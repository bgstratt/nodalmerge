//! Slice 2.4 (`blob-cas-remediation.md`, latent) — `blob-request`'s
//! bytes-over-WS fallthrough must fall back to `persistence.get_blob` on an
//! in-memory miss.
//!
//! Pre-2.4, the fallthrough path (`ws_handler.rs`'s `BlobRequest` arm, past
//! the F6 redirect split) served ONLY `room.blobs` — the room's in-memory
//! `MemoryBlobStore` — with no durable-store fallback. A peer that did not
//! negotiate `supports_direct_blob_io` (or whose backend has no presigned
//! URL for the hash) asking for a hash that is durable but was never
//! relayed into *this* room process's memory (e.g. after a restart, or a
//! hash a sibling process persisted) got nothing back: an empty `blob-pack`
//! naming the hash in `requested` but not in `blobs`.
//!
//! Two scenarios, matching the two `BlobPersistence::get_blob` contracts:
//! - `DirPersistence`: `get_blob` returns real, BLAKE3-verified-on-read
//!   bytes (slice 3.2) for anything durably stored. The fallback must serve
//!   them, in the same `BlobPackEntry` shape as an in-memory hit.
//! - A non-hydrating backend (`S3BlobStore`'s shape, ported here by
//!   `NonHydratingBackend` per slice 0.2's shared fake): `get_blob` is
//!   `None` BY CONTRACT (slice 2.2 pinned "S3 never hydrates"). The
//!   fallback is therefore a structural no-op there — pinned explicitly so
//!   a future change can't accidentally start hydrating file payloads
//!   through this path.
//!
//! Harness: reuses the real axum + `tokio_tungstenite` WS integration
//! pattern already established in `host_migration_parity.rs` /
//! `rate_limit.rs` (`ws_handler::handler` has no unit seam — it's a
//! receive-loop shaped function — so a real socket is the only way to
//! drive this specific match arm in-process).

use std::sync::Arc;
use std::time::Duration;

use axum::{routing::get, Router};
use base64::Engine as _;
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use nodalmerge_blobstore_conformance::NonHydratingBackend;
use nodalmerge_core::{BlobStore, Hash};
use nodalmerge_server::room::Rooms;
use nodalmerge_server::store::{Composite, DirPersistence, NoPersistence, SharedPersistence};
use nodalmerge_server::ws_handler;
use tokio_tungstenite::tungstenite::Message as TMessage;

async fn spawn_server(persistence: SharedPersistence) -> (std::net::SocketAddr, Rooms) {
    let server_key = SigningKey::from_bytes(&[0x9Cu8; 32]);
    let rooms = Rooms::new(server_key, persistence, 512, 0, 0);

    let app = Router::new()
        .route("/ws/:room_id", get(ws_handler::handler))
        .with_state(rooms.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let addr = listener
        .local_addr()
        .expect("listener should expose local addr");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("blob-request-fallback server should run");
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, rooms)
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

/// Connect, complete `hello`/`welcome` (unlocked room, no caps — this test
/// is specifically about a peer that did NOT negotiate direct blob IO, so
/// the fallthrough bucket is guaranteed to hold the whole request), send a
/// `blob-request` for `hashes`, and return the resulting `blob-pack`
/// envelope (or `None` if none arrived within the deadline).
async fn request_blobs_over_ws(
    addr: std::net::SocketAddr,
    room_id: &str,
    hashes: &[String],
) -> Option<serde_json::Value> {
    let url = format!("ws://{addr}/ws/{room_id}");
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x9Du8; 32])
        .verifying_key()
        .to_bytes();
    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": hex_lower(&peer),
        "frontier": [],
    })
    .to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut saw_welcome = false;
    while tokio::time::Instant::now() < deadline && !saw_welcome {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
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

    let blob_req = serde_json::json!({
        "type": "blob-request",
        "hashes": hashes,
    })
    .to_string();
    sink.send(TMessage::Text(blob_req.into()))
        .await
        .expect("send blob-request");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(t)))) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                if v.get("type").and_then(|t| t.as_str()) == Some("blob-pack") {
                    return Some(v);
                }
            }
            Ok(Some(Ok(_))) | Ok(None) | Err(_) => continue,
            Ok(Some(Err(e))) => panic!("stream error waiting for blob-pack: {e}"),
        }
    }
    None
}

/// RED before the 2.4 fix, GREEN after: a blob durably stored via
/// `persistence.persist_blob` directly (bypassing `room.blobs` entirely —
/// nothing ever put it in the room's in-memory store, and it is not
/// referenced by any node so room-open hydration doesn't load it either)
/// must still be served over the bytes-over-WS fallthrough.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blob_request_falls_back_to_dir_persistence_on_in_memory_miss() {
    let tmp_root = std::env::temp_dir().join(format!(
        "nodalmerge-blob-request-fallback-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_root).expect("temp persistence root should be created");

    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&tmp_root).expect("dir persistence should open"));

    let bytes = b"slice-2.4-durable-but-not-relayed".to_vec();
    let hash = Hash::of(&bytes);
    persistence
        .persist_blob(&hash, &bytes)
        .expect("dir persist_blob should succeed");

    let room_id = "blob-fallback-dir-room";
    let (addr, rooms) = spawn_server(persistence).await;
    let room = rooms.get_or_create(room_id).await;

    // Sanity: the blob genuinely is NOT in the room's in-memory store before
    // the request. If this ever fails, the test isn't exercising the miss
    // path it claims to.
    assert!(
        !room.blobs.read().await.contains(&hash),
        "test setup bug: blob must be absent from the in-memory store"
    );

    let pack = request_blobs_over_ws(addr, room_id, &[hash.to_hex()])
        .await
        .expect("expected a blob-pack reply within 5s");

    let blobs = pack["blobs"].as_array().expect("blobs should be an array");
    assert_eq!(
        blobs.len(),
        1,
        "expected the durably-stored blob to be served via the persistence \
         fallback; pre-2.4 this array is empty because only room.blobs (the \
         in-memory store) is consulted: {pack}"
    );
    assert_eq!(blobs[0]["hash"], hash.to_hex());
    let got_data = blobs[0]["data"]
        .as_str()
        .expect("blob entry should carry base64 data");
    let got_bytes = base64::engine::general_purpose::STANDARD
        .decode(got_data)
        .expect("data should be valid base64");
    assert_eq!(
        got_bytes, bytes,
        "served bytes must match what was durably persisted"
    );

    let requested = pack["requested"]
        .as_array()
        .expect("requested should be an array");
    assert_eq!(requested, &[serde_json::json!(hash.to_hex())]);

    // The in-memory store must NOT have been warmed by the fallback:
    // `MemoryBlobStore` (`core/crdt/src/storage.rs`) is an unbounded
    // `HashMap` with no eviction, so warming it on every persistence
    // fallback would risk pulling the whole durable CAS into this
    // process's RAM for a busy relay room. Only genuine uploads
    // (`BlobUpload`) and this room's own referenced-blob hydrate-on-open
    // are allowed to populate it.
    assert!(
        !room.blobs.read().await.contains(&hash),
        "the persistence-fallback path must not warm the in-memory relay \
         cache with bytes it only borrowed from the durable store"
    );

    let _ = std::fs::remove_dir_all(&tmp_root);
}

/// Structural pin, not a RED/GREEN case: on a backend whose `get_blob`
/// returns `None` by contract (the `S3BlobStore` shape — slice 2.2 pinned
/// "S3 never hydrates"), the persistence fallback is a no-op. This must be
/// true both before AND after the 2.4 fix — it is not a gap the fix closes,
/// it is the backend's own offloading policy (see the F6 redirect path and
/// the delegate/direct presign negotiation for how a capable client gets
/// these bytes instead).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blob_request_fallback_is_a_structural_no_op_on_a_non_hydrating_backend() {
    let backend = NonHydratingBackend::default();
    let bytes = b"present-in-the-bucket-never-hydrated".to_vec();
    let hash = Hash::of(&bytes);
    // `mark_present` (no bytes) is exactly the shape a real S3 upload
    // leaves: the object exists, but this fake — like the real
    // `S3BlobStore` — never reads bytes back through `get_blob`.
    backend.mark_present(hash);

    // `SharedPersistence` needs both halves; `NonHydratingBackend` only
    // stands in for the blob half (same pairing `blob_relay_nonhydrating_
    // backend.rs` uses).
    let persistence: SharedPersistence = Arc::new(Composite::new(NoPersistence, backend));

    let room_id = "blob-fallback-nonhydrating-room";
    let (addr, _rooms) = spawn_server(persistence).await;

    let pack = request_blobs_over_ws(addr, room_id, &[hash.to_hex()])
        .await
        .expect("expected a blob-pack reply within 5s");

    let blobs = pack["blobs"].as_array().expect("blobs should be an array");
    assert!(
        blobs.is_empty(),
        "a non-hydrating backend's get_blob is None by contract (slice \
         2.2); the fallback must stay a structural no-op here, not attempt \
         to hydrate or presign: {pack}"
    );
    let requested = pack["requested"]
        .as_array()
        .expect("requested should be an array");
    assert_eq!(requested, &[serde_json::json!(hash.to_hex())]);
}
