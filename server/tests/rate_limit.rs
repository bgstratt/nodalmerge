//! G3 integration test — per-peer rate limiting.
//!
//! Scenario: server is started with `peer_rate_nodes = 2` (nodes/sec).
//! A peer completes the handshake and then pushes a single pack of 5
//! distinct signed nodes. That's larger than the 1-second burst (2) for
//! the node-count limiter, so `check_n(5)` returns `InsufficientCapacity`
//! and the server must close the WS with code `4008 rate limit exceeded`.
//!
//! We intentionally DO NOT install the global Prometheus recorder here —
//! that collides with `metrics_endpoint.rs`, which has the one-install
//! monopoly for the process. `metrics::counter!` calls are no-ops when
//! no recorder is registered, so the behavioural assertions (close code
//! arriving, session torn down) still cover the observable G3 contract.

use std::sync::Arc;
use std::time::Duration;

use activesync_core::{MapOp, Op, StateGraph, SyncNode, pack_nodes};
use activesync_server::room::Rooms;
use activesync_server::store::{NoPersistence, SharedPersistence};
use activesync_server::ws_handler;
use axum::{routing::get, Router};
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as TMessage;

async fn spawn_server(peer_rate_nodes: u32, peer_rate_bytes: u32) -> std::net::SocketAddr {
    let server_key = SigningKey::from_bytes(&[9u8; 32]);
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    // Broadcast capacity doesn't matter for this test; 512 is the prod default.
    let rooms = Rooms::new(server_key, persistence, 512, peer_rate_nodes, peer_rate_bytes);

    let app = Router::new()
        .route("/ws/:room_id", get(ws_handler::handler))
        .with_state(rooms);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

/// Build a single signed `SyncNode` with the given key/value. Uses a
/// fresh `StateGraph` so each call produces a node whose parent set is
/// empty — fine for this test because we only need the pack to decode;
/// server-side import may reject the nodes later (we close before that
/// codepath on the failure case, and we don't assert on accept counts).
fn make_node(sk: &SigningKey, key: &str, val: &[u8]) -> SyncNode {
    let mut g = StateGraph::new();
    let id = g
        .apply_local(sk, 0, vec![Op::Map(MapOp::Set { key: key.into(), value: val.to_vec() })])
        .unwrap();
    g.get_nodes(&[id]).into_iter().next().unwrap().clone()
}

fn base64_encode(data: &[u8]) -> String {
    // Copy of ws_handler::base64_encode's behaviour (standard alphabet, pad).
    const ALPHA: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    let mut i = 0;
    while i + 3 <= data.len() {
        let a = data[i] as u32;
        let b = data[i + 1] as u32;
        let c = data[i + 2] as u32;
        let n = (a << 16) | (b << 8) | c;
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char);
        out.push(ALPHA[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = data.len() - i;
    if rem == 1 {
        let a = data[i] as u32;
        let n = a << 16;
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let a = data[i] as u32;
        let b = data[i + 1] as u32;
        let n = (a << 16) | (b << 8);
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char);
        out.push('=');
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_pack_closes_with_4008() {
    let addr = spawn_server(/* nodes/s */ 2, /* bytes/s */ 0).await;

    let url = format!("ws://{addr}/ws/ratey");
    let (ws, _resp) = tokio_tungstenite::connect_async(url).await.expect("connect");
    let (mut ws_sink, mut ws_stream) = ws.split();

    let peer_sk = SigningKey::from_bytes(&[0xAB; 32]);
    let peer_pk_hex = hex_lower(&peer_sk.verifying_key().to_bytes());

    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": peer_pk_hex,
        "frontier": [],
    })
    .to_string();
    ws_sink.send(TMessage::Text(hello.into())).await.expect("send hello");

    // Drain to welcome.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut saw_welcome = false;
    while tokio::time::Instant::now() < deadline && !saw_welcome {
        match tokio::time::timeout(Duration::from_millis(500), ws_stream.next()).await {
            Ok(Some(Ok(TMessage::Text(t)))) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                if v["type"] == "welcome" {
                    saw_welcome = true;
                }
            }
            Ok(Some(Ok(_))) | Ok(None) | Err(_) => continue,
            Ok(Some(Err(e))) => panic!("stream error: {e}"),
        }
    }
    assert!(saw_welcome, "did not receive welcome within 5s");

    // Build a pack of 5 distinct signed nodes — exceeds burst=2.
    let nodes: Vec<SyncNode> = (0..5u8)
        .map(|i| make_node(&peer_sk, &format!("k{i}"), &[i]))
        .collect();
    let refs: Vec<&SyncNode> = nodes.iter().collect();
    let pack_b64 = base64_encode(&pack_nodes(&refs));
    let env = serde_json::json!({ "type": "pack", "nodes": pack_b64 }).to_string();
    ws_sink.send(TMessage::Text(env.into())).await.expect("send pack");

    // Assert Close{4008} within 5 s.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut close_code: Option<u16> = None;
    while tokio::time::Instant::now() < deadline && close_code.is_none() {
        match tokio::time::timeout(Duration::from_millis(500), ws_stream.next()).await {
            Ok(Some(Ok(TMessage::Close(Some(cf))))) => {
                close_code = Some(u16::from(cf.code));
            }
            Ok(Some(Ok(_))) => continue,
            Ok(None) | Err(_) => continue,
            Ok(Some(Err(e))) => panic!("stream error waiting for close: {e}"),
        }
    }
    assert_eq!(
        close_code,
        Some(4008),
        "expected 4008 rate-limit close; got {close_code:?}"
    );
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
