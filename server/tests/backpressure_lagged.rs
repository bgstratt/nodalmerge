//! G1 integration test — lagged slow-consumer disconnect.
//!
//! Scenario: a peer completes the handshake, then stops draining its WS. We
//! flood the room's broadcast channel with more messages than `--broadcast-
//! capacity` (here: 2) before yielding, so the handler's `bcast_rx` wakes
//! into `RecvError::Lagged`. The server must:
//!   - log a warning with the lagged count,
//!   - send a WS Close frame with code `4001 resync required`,
//!   - break out of the main loop so cleanup runs (peer count returns to 0).
//!
//! This test does NOT install the global Prometheus recorder — that would
//! clash with `metrics_endpoint.rs`, which has a one-install-per-process
//! monopoly. `metrics::counter!` calls are no-ops when no recorder is
//! registered, so the behavioral assertions (close code + peer deregister)
//! cover the observable G1 contract on their own.

use std::sync::Arc;
use std::time::Duration;

use axum::{routing::get, Router};
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use nodalmerge_server::room::Rooms;
use nodalmerge_server::store::{NoPersistence, SharedPersistence};
use nodalmerge_server::ws_handler;
use tokio_tungstenite::tungstenite::Message as TMessage;

/// Spawn an axum server bound to an ephemeral loopback port with the given
/// broadcast capacity. Returns `(addr, rooms)` so the test can both connect
/// a client AND drive `room.tx.send` directly.
async fn spawn_server(broadcast_capacity: usize) -> (std::net::SocketAddr, Rooms) {
    let server_key = SigningKey::from_bytes(&[7u8; 32]);
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let rooms = Rooms::new(server_key, persistence, broadcast_capacity, 0, 0);

    let app = Router::new()
        .route("/ws/:room_id", get(ws_handler::handler))
        .with_state(rooms.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // Tiny grace so the listener is polling before we try to connect.
    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, rooms)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lagged_broadcast_closes_with_4001() {
    let (addr, rooms) = spawn_server(2).await;

    // --- client connects ----------------------------------------------------
    let url = format!("ws://{addr}/ws/laggy");
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut ws_sink, mut ws_stream) = ws.split();

    // Minimal hello — open room, no IBF, no subscription filter.
    let pubkey_hex = "aa".repeat(32);
    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": pubkey_hex,
        "frontier": [],
    })
    .to_string();
    ws_sink
        .send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    // Read welcome (+ peer-joined broadcast) — drain until we see welcome.
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
            Ok(Some(Err(e))) => panic!("unexpected ws error: {e}"),
        }
    }
    assert!(saw_welcome, "never received welcome");

    // --- flood the broadcast channel ---------------------------------------
    //
    // `broadcast::Sender::send` is synchronous and non-yielding. Running it
    // in a tight loop without any `.await` means the handler task cannot
    // interleave — it stays parked in `bcast_rx.recv().await`. With capacity=2
    // the ring buffer keeps only the last 2 messages; when the handler is
    // eventually polled, its cursor is thousands of slots behind and recv
    // returns `Err(Lagged(n))`, which must trigger the 4001 close.
    let room = rooms.get_or_create("laggy").await;
    for i in 0..5000u32 {
        let env = serde_json::json!({
            "type": "flood",
            "from": "other-peer",
            "n": i,
        })
        .to_string();
        let _ = room.tx.send(env);
    }

    // --- observe the close frame -------------------------------------------
    //
    // The server task wakes, sees Lagged, sends Close(4001), and breaks.
    // Our client stream should deliver that frame within a couple seconds.
    let close_code = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(msg) = ws_stream.next().await {
            match msg {
                Ok(TMessage::Close(Some(frame))) => return Some(u16::from(frame.code)),
                Ok(TMessage::Close(None)) => return Some(0),
                Ok(_) => continue, // drain normal messages
                Err(_) => return None,
            }
        }
        None
    })
    .await
    .expect("timed out waiting for close frame")
    .expect("stream ended without a close frame");

    assert_eq!(
        close_code, 4001,
        "expected 4001 resync required, got {close_code}"
    );

    // Cleanup should have run: peer count returns to 0.
    // Give the server task a brief moment to deregister after the close frame.
    let peers_empty = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if room.connected_peers.read().await.is_empty() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or(false);
    assert!(peers_empty, "peer never got deregistered after 4001 close");
}
