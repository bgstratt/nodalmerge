//! G6 integration test — capability token expiry mid-session.
//!
//! Scenario: a locked room (`auth_key` set). Client joins with a
//! valid token whose `expiry` is 2 seconds in the future. The server
//! must accept the hello, complete the handshake, and then disconnect
//! the peer with WS close code `4002 token expired` shortly after the
//! 2-second mark — not drop it silently and not keep the session open.
//!
//! The second test covers the positive-control case: the server does
//! **not** close a peer whose token is valid for 60 s within the first
//! 3 s of the session. Guards against a future refactor that accidentally
//! fires the G6 timer prematurely.
//!
//! Same caveat as `rate_limit.rs`: we do not install the Prometheus
//! recorder (monopoly belongs to `metrics_endpoint.rs`). The behaviour
//! under test — `Close{4002}` after expiry — is observable without it.

use std::sync::Arc;
use std::time::Duration;

use axum::{routing::get, Router};
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use nodalmerge_core::RoomToken;
use nodalmerge_server::room::Rooms;
use nodalmerge_server::store::{NoPersistence, SharedPersistence};
use nodalmerge_server::ws_handler;
use tokio_tungstenite::tungstenite::Message as TMessage;

/// Spawn a server with a single locked room `locked-room` whose auth
/// key is the returned `SigningKey` (test controls it so it can mint
/// valid tokens).
async fn spawn_locked_server() -> (std::net::SocketAddr, Rooms, SigningKey) {
    let server_key = SigningKey::from_bytes(&[7u8; 32]);
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let rooms = Rooms::new(server_key, persistence, 512, 0, 0);

    let room_key = SigningKey::from_bytes(&[0xABu8; 32]);
    let room = rooms.get_or_create("locked-room").await;
    *room.auth_key.write().await = Some(room_key.verifying_key());

    let app = Router::new()
        .route("/ws/:room_id", get(ws_handler::handler))
        .with_state(rooms.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, rooms, room_key)
}

fn hex_lower(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 0x0f) as usize] as char);
    }
    s
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Build the JSON envelope the handler expects in `hello.token`.
fn token_json(tok: &RoomToken) -> serde_json::Value {
    serde_json::json!({
        "peer_pubkey": hex_lower(&tok.peer_pubkey),
        "expiry": tok.expiry_secs,
        "caps": tok.capabilities,
        "sig": hex_lower(&tok.signature),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_token_closes_with_4002() {
    let (addr, _rooms, room_key) = spawn_locked_server().await;

    let peer_sk = SigningKey::from_bytes(&[0x11u8; 32]);
    let peer_pk = peer_sk.verifying_key().to_bytes();
    let peer_pk_hex = hex_lower(&peer_pk);

    // Token expires 2 s from now — long enough to handshake, short
    // enough to keep the test fast.
    let expiry = now_secs() + 2;
    let token = RoomToken::sign("locked-room", &peer_pk, expiry, &[], &room_key);

    let url = format!("ws://{addr}/ws/locked-room");
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut ws_sink, mut ws_stream) = ws.split();

    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": peer_pk_hex,
        "frontier": [],
        "token": token_json(&token),
    })
    .to_string();
    ws_sink
        .send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    // The server should send welcome, then later close with 4002.
    // Budget: token expires in 2 s; poll up to 6 s total.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    let mut saw_welcome = false;
    let mut close_code: Option<u16> = None;
    while tokio::time::Instant::now() < deadline && close_code.is_none() {
        match tokio::time::timeout(Duration::from_millis(500), ws_stream.next()).await {
            Ok(Some(Ok(TMessage::Text(t)))) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                if v["type"] == "welcome" {
                    saw_welcome = true;
                }
                if v["type"] == "error" {
                    panic!("unexpected server error during G6 happy-path: {t}");
                }
            }
            Ok(Some(Ok(TMessage::Close(Some(cf))))) => {
                close_code = Some(u16::from(cf.code));
            }
            Ok(Some(Ok(_))) | Ok(None) | Err(_) => continue,
            Ok(Some(Err(e))) => panic!("stream error: {e}"),
        }
    }

    assert!(
        saw_welcome,
        "handshake did not complete before token expired"
    );
    assert_eq!(
        close_code,
        Some(4002),
        "expected 4002 token expired close; got {close_code:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_token_does_not_close_prematurely() {
    let (addr, _rooms, room_key) = spawn_locked_server().await;

    let peer_sk = SigningKey::from_bytes(&[0x22u8; 32]);
    let peer_pk = peer_sk.verifying_key().to_bytes();
    let peer_pk_hex = hex_lower(&peer_pk);

    let expiry = now_secs() + 60;
    let token = RoomToken::sign("locked-room", &peer_pk, expiry, &[], &room_key);

    let url = format!("ws://{addr}/ws/locked-room");
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut ws_sink, mut ws_stream) = ws.split();

    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": peer_pk_hex,
        "frontier": [],
        "token": token_json(&token),
    })
    .to_string();
    ws_sink
        .send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    // Poll for 3 s. Must see a welcome; must NOT see any close frame.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut saw_welcome = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(300), ws_stream.next()).await {
            Ok(Some(Ok(TMessage::Text(t)))) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                if v["type"] == "welcome" {
                    saw_welcome = true;
                }
            }
            Ok(Some(Ok(TMessage::Close(Some(cf))))) => {
                panic!(
                    "server closed prematurely: code={} reason={:?}",
                    u16::from(cf.code),
                    cf.reason
                );
            }
            Ok(Some(Ok(_))) | Ok(None) | Err(_) => continue,
            Ok(Some(Err(e))) => panic!("stream error: {e}"),
        }
    }
    assert!(saw_welcome, "did not receive welcome within 3s");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expiring_duplicate_session_does_not_emit_early_peer_left() {
    let (addr, _rooms, room_key) = spawn_locked_server().await;

    let peer_sk = SigningKey::from_bytes(&[0x33u8; 32]);
    let peer_pk = peer_sk.verifying_key().to_bytes();
    let peer_pk_hex = hex_lower(&peer_pk);

    // Session A: short-lived token.
    let short_token = RoomToken::sign("locked-room", &peer_pk, now_secs() + 2, &[], &room_key);
    let url_a = format!("ws://{addr}/ws/locked-room");
    let (ws_a, _resp) = tokio_tungstenite::connect_async(url_a)
        .await
        .expect("connect short-lived session");
    let (mut sink_a, mut stream_a) = ws_a.split();
    let hello_a = serde_json::json!({
        "type": "hello",
        "pubkey": peer_pk_hex,
        "frontier": [],
        "token": token_json(&short_token),
    })
    .to_string();
    sink_a
        .send(TMessage::Text(hello_a.into()))
        .await
        .expect("send hello short-lived session");

    let deadline_a = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut saw_welcome_a = false;
    while tokio::time::Instant::now() < deadline_a && !saw_welcome_a {
        match tokio::time::timeout(Duration::from_millis(300), stream_a.next()).await {
            Ok(Some(Ok(TMessage::Text(t)))) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                if v["type"] == "welcome" {
                    saw_welcome_a = true;
                }
            }
            Ok(Some(Ok(_))) | Ok(None) | Err(_) => continue,
            Ok(Some(Err(e))) => panic!("stream A error waiting for welcome: {e}"),
        }
    }
    assert!(saw_welcome_a, "session A did not receive welcome");

    // Session B: longer-lived token with same pubkey.
    let long_token = RoomToken::sign("locked-room", &peer_pk, now_secs() + 30, &[], &room_key);
    let url_b = format!("ws://{addr}/ws/locked-room");
    let (ws_b, _resp) = tokio_tungstenite::connect_async(url_b)
        .await
        .expect("connect long-lived session");
    let (mut sink_b, mut stream_b) = ws_b.split();
    let hello_b = serde_json::json!({
        "type": "hello",
        "pubkey": peer_pk_hex,
        "frontier": [],
        "token": token_json(&long_token),
    })
    .to_string();
    sink_b
        .send(TMessage::Text(hello_b.into()))
        .await
        .expect("send hello long-lived session");

    let deadline_b = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut saw_welcome_b = false;
    while tokio::time::Instant::now() < deadline_b && !saw_welcome_b {
        match tokio::time::timeout(Duration::from_millis(300), stream_b.next()).await {
            Ok(Some(Ok(TMessage::Text(t)))) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                if v["type"] == "welcome" {
                    saw_welcome_b = true;
                }
            }
            Ok(Some(Ok(_))) | Ok(None) | Err(_) => continue,
            Ok(Some(Err(e))) => panic!("stream B error waiting for welcome: {e}"),
        }
    }
    assert!(saw_welcome_b, "session B did not receive welcome");

    // Wait for session A to expire and close with 4002.
    let close_a = tokio::time::timeout(Duration::from_secs(6), async {
        while let Some(msg) = stream_a.next().await {
            match msg {
                Ok(TMessage::Close(Some(cf))) => return Some(u16::from(cf.code)),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
        None
    })
    .await
    .expect("timed out waiting for session A close");
    assert_eq!(close_a, Some(4002), "session A should close on token expiry");

    // Session B should remain connected and must not receive peer-left yet.
    let early_left = tokio::time::timeout(Duration::from_millis(800), stream_b.next()).await;
    if let Ok(Some(Ok(TMessage::Text(t)))) = early_left {
        let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
        assert_ne!(
            v.get("type").and_then(|x| x.as_str()),
            Some("peer-left"),
            "peer-left emitted while duplicate long-lived session still connected"
        );
    }

    sink_b
        .send(TMessage::Close(None))
        .await
        .expect("close long-lived session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presence_rejects_ttl_without_now_unix_ms() {
    let (addr, _rooms, room_key) = spawn_locked_server().await;

    let peer_sk = SigningKey::from_bytes(&[0x44u8; 32]);
    let peer_pk = peer_sk.verifying_key().to_bytes();
    let peer_pk_hex = hex_lower(&peer_pk);
    let token = RoomToken::sign("locked-room", &peer_pk, now_secs() + 60, &[], &room_key);

    let url = format!("ws://{addr}/ws/locked-room");
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": peer_pk_hex,
        "frontier": [],
        "token": token_json(&token),
    })
    .to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut saw_welcome = false;
    while tokio::time::Instant::now() < deadline && !saw_welcome {
        match tokio::time::timeout(Duration::from_millis(300), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(t)))) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                if v["type"] == "welcome" {
                    saw_welcome = true;
                }
            }
            Ok(Some(Ok(_))) | Ok(None) | Err(_) => continue,
            Ok(Some(Err(e))) => panic!("stream error waiting for welcome: {e}"),
        }
    }
    assert!(saw_welcome, "did not receive welcome");

    let invalid_presence = serde_json::json!({
        "type": "presence",
        "data": { "state": "online" },
        "ttl_ms": 1000
    })
    .to_string();
    sink.send(TMessage::Text(invalid_presence.into()))
        .await
        .expect("send invalid presence");

    let rejection = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(msg) = stream.next().await {
            match msg {
                Ok(TMessage::Text(t)) => {
                    let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                    if v.get("type").and_then(|x| x.as_str()) == Some("error") {
                        return Some(v);
                    }
                }
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
        None
    })
    .await
    .expect("timed out waiting for rejection")
    .expect("missing error envelope");

    assert_eq!(
        rejection.get("msg").and_then(|x| x.as_str()),
        Some("reject.presence_lease_invalid:ttl_ms_requires_now_unix_ms")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presence_rejects_now_without_ttl_ms() {
    let (addr, _rooms, room_key) = spawn_locked_server().await;

    let peer_sk = SigningKey::from_bytes(&[0x55u8; 32]);
    let peer_pk = peer_sk.verifying_key().to_bytes();
    let peer_pk_hex = hex_lower(&peer_pk);
    let token = RoomToken::sign("locked-room", &peer_pk, now_secs() + 60, &[], &room_key);

    let url = format!("ws://{addr}/ws/locked-room");
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": peer_pk_hex,
        "frontier": [],
        "token": token_json(&token),
    })
    .to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut saw_welcome = false;
    while tokio::time::Instant::now() < deadline && !saw_welcome {
        match tokio::time::timeout(Duration::from_millis(300), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(t)))) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                if v["type"] == "welcome" {
                    saw_welcome = true;
                }
            }
            Ok(Some(Ok(_))) | Ok(None) | Err(_) => continue,
            Ok(Some(Err(e))) => panic!("stream error waiting for welcome: {e}"),
        }
    }
    assert!(saw_welcome, "did not receive welcome");

    let invalid_presence = serde_json::json!({
        "type": "presence",
        "data": { "state": "online" },
        "now_unix_ms": 1000
    })
    .to_string();
    sink.send(TMessage::Text(invalid_presence.into()))
        .await
        .expect("send invalid presence");

    let rejection = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(msg) = stream.next().await {
            match msg {
                Ok(TMessage::Text(t)) => {
                    let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                    if v.get("type").and_then(|x| x.as_str()) == Some("error") {
                        return Some(v);
                    }
                }
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
        None
    })
    .await
    .expect("timed out waiting for rejection")
    .expect("missing error envelope");

    assert_eq!(
        rejection.get("msg").and_then(|x| x.as_str()),
        Some("reject.presence_lease_invalid:now_unix_ms_requires_ttl_ms")
    );
}
