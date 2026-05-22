//! Workstream A parity harness (slice 1): hello + catchup.
//!
//! This harness runs a golden scenario through two execution paths:
//! - baseline: current server websocket adapter behavior
//! - shadow: candidate extraction path (currently mapped to baseline)
//!
//! As host-core extraction lands, replace `run_shadow_path` with the migrated
//! adapter path and keep the fixture/trace comparison unchanged.

use std::sync::Arc;
use std::time::Duration;

use activesync_core::{BlobStore, MapOp, Op, RoomToken, StateGraph};
use activesync_server::room::{import_nodes, Rooms};
use activesync_server::store::{NoPersistence, SharedPersistence};
use activesync_server::ws_handler;
use axum::{routing::get, Router};
use base64::Engine as _;
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message as TMessage;

#[derive(Debug, Deserialize)]
struct GoldenFixture {
    name: String,
    room_id: String,
    #[serde(default = "default_scenario")]
    scenario: String,
    expected_sequence: Vec<String>,
    #[serde(default)]
    expected_pack_count: usize,
    #[serde(default)]
    expected_close_code: Option<u16>,
    #[serde(default)]
    token_ttl_secs: Option<u64>,
    #[serde(default)]
    client_supports_ibf: bool,
    #[serde(default)]
    client_supports_mst: bool,
    #[serde(default)]
    include_client_ibf_empty: bool,
    #[serde(default)]
    expected_welcome_has_mst_root: Option<bool>,
    #[serde(default)]
    expected_blob_pack_count: Option<usize>,
    #[serde(default)]
    expected_upload_denied_reason: Option<String>,
    #[serde(default)]
    expected_has_snapshot_pack: Option<bool>,
    #[serde(default)]
    expected_has_compact_ack: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CanonicalTrace {
    sequence: Vec<String>,
    pack_count: usize,
    close_code: Option<u16>,
    welcome_has_mst_root: Option<bool>,
    blob_pack_count: Option<usize>,
    upload_denied_reason: Option<String>,
    has_snapshot_pack: Option<bool>,
    has_compact_ack: Option<bool>,
}

fn default_scenario() -> String {
    "hello_catchup".to_string()
}

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn load_fixture(name: &str) -> GoldenFixture {
    let p = fixture_path(name);
    let s = std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", p.display()));
    serde_json::from_str(&s)
        .unwrap_or_else(|e| panic!("invalid fixture JSON {}: {e}", p.display()))
}

fn make_map_set_node(sk: &SigningKey, key: &str, value: &[u8]) -> activesync_core::SyncNode {
    let mut g = StateGraph::new();
    let id = g
        .apply_local(
            sk,
            0,
            vec![Op::Map(MapOp::Set {
                key: key.to_string(),
                value: value.to_vec(),
            })],
        )
        .unwrap();
    g.get_nodes(&[id]).into_iter().next().unwrap().clone()
}

async fn spawn_server() -> (std::net::SocketAddr, Rooms) {
    let server_key = SigningKey::from_bytes(&[0x51u8; 32]);
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let rooms = Rooms::new(server_key, persistence, 512, 0, 0);

    let app = Router::new()
        .route("/ws/:room_id", get(ws_handler::handler))
        .with_state(rooms.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, rooms)
}

async fn spawn_locked_server(room_id: &str) -> (std::net::SocketAddr, Rooms, SigningKey) {
    let (addr, rooms) = spawn_server().await;
    let room_key = SigningKey::from_bytes(&[0xABu8; 32]);
    let room = rooms.get_or_create(room_id).await;
    *room.auth_key.write().await = Some(room_key.verifying_key());
    (addr, rooms, room_key)
}

async fn run_current_path(fx: &GoldenFixture) -> CanonicalTrace {
    match fx.scenario.as_str() {
        "hello_catchup" => run_current_hello_catchup(fx).await,
        "token_expiry" => run_current_token_expiry(fx).await,
        "blob_flow" => run_current_blob_flow(fx).await,
        "tick_compaction" => run_current_tick_compaction(fx).await,
        other => panic!("unknown fixture scenario: {other}"),
    }
}

async fn run_current_hello_catchup(fx: &GoldenFixture) -> CanonicalTrace {
    let (addr, rooms) = spawn_server().await;

    let room = rooms.get_or_create(&fx.room_id).await;
    let author = SigningKey::from_bytes(&[0x22u8; 32]);
    let node = make_map_set_node(&author, "world/greeting", b"hello");
    let (accepted, _, errs) = import_nodes(&room, vec![node]).await;
    assert_eq!(accepted, 1);
    assert!(errs.is_empty());

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url).await.expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x31u8; 32]).verifying_key().to_bytes();
    let peer_hex = hex_lower(&peer);

    let mut hello = serde_json::json!({
        "type": "hello",
        "pubkey": peer_hex,
        "frontier": [],
        "caps": {
            "supports_ibf": fx.client_supports_ibf,
            "supports_mst": fx.client_supports_mst
        },
        "subscribe": ["**"]
    });
    if fx.include_client_ibf_empty {
        let ibf_b64 = base64::engine::general_purpose::STANDARD
            .encode(activesync_core::Ibf::from_ids(&[]).encode());
        hello["ibf"] = serde_json::json!(ibf_b64);
    }
    let hello = hello.to_string();
    sink.send(TMessage::Text(hello.into())).await.expect("send hello");

    let mut sequence = Vec::new();
    let mut pack_count = 0usize;
    let mut welcome_has_mst_root = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    while tokio::time::Instant::now() < deadline {
        let next = tokio::time::timeout(Duration::from_millis(500), stream.next()).await;
        match next {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let Some(ty) = v.get("type").and_then(|t| t.as_str()) else {
                    continue;
                };
                match ty {
                    "welcome" => {
                        sequence.push("welcome".to_string());
                        welcome_has_mst_root = Some(
                            v.get("mst_root")
                                .and_then(|x| x.as_str())
                                .map(|s| !s.is_empty())
                                .unwrap_or(false),
                        );
                    }
                    "pack" => {
                        sequence.push("pack".to_string());
                        pack_count = decode_pack_count(&v);
                        break;
                    }
                    _ => {
                        // Ignore non-parity events in this first harness slice.
                    }
                }
            }
            Ok(Some(Ok(TMessage::Close(Some(cf))))) => {
                return CanonicalTrace {
                    sequence,
                    pack_count,
                    close_code: Some(u16::from(cf.code)),
                    welcome_has_mst_root,
                    blob_pack_count: None,
                    upload_denied_reason: None,
                    has_snapshot_pack: None,
                    has_compact_ack: None,
                };
            }
            _ => continue,
        }
    }

    CanonicalTrace {
        sequence,
        pack_count,
        close_code: None,
        welcome_has_mst_root,
        blob_pack_count: None,
        upload_denied_reason: None,
        has_snapshot_pack: None,
        has_compact_ack: None,
    }
}

async fn run_current_token_expiry(fx: &GoldenFixture) -> CanonicalTrace {
    let (addr, _rooms, room_key) = spawn_locked_server(&fx.room_id).await;

    let peer = SigningKey::from_bytes(&[0x41u8; 32]).verifying_key().to_bytes();
    let peer_hex = hex_lower(&peer);
    let ttl = fx.token_ttl_secs.unwrap_or(2);
    let token = RoomToken::sign(&fx.room_id, &peer, now_secs() + ttl, &[], &room_key);

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url).await.expect("connect");
    let (mut sink, mut stream) = ws.split();

    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": peer_hex,
        "frontier": [],
        "token": token_json(&token)
    })
    .to_string();
    sink.send(TMessage::Text(hello.into())).await.expect("send hello");

    let mut sequence = Vec::new();
    let mut welcome_has_mst_root = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    let mut close_code = None;

    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(ty) = v.get("type").and_then(|t| t.as_str()) {
                    if ty == "welcome" {
                        sequence.push("welcome".to_string());
                        welcome_has_mst_root = Some(
                            v.get("mst_root")
                                .and_then(|x| x.as_str())
                                .map(|s| !s.is_empty())
                                .unwrap_or(false),
                        );
                    }
                    if ty == "error" {
                        panic!("unexpected error in token-expiry scenario: {text}");
                    }
                }
            }
            Ok(Some(Ok(TMessage::Close(Some(cf))))) => {
                sequence.push("close".to_string());
                close_code = Some(u16::from(cf.code));
                break;
            }
            _ => continue,
        }
    }

    CanonicalTrace {
        sequence,
        pack_count: 0,
        close_code,
        welcome_has_mst_root,
        blob_pack_count: None,
        upload_denied_reason: None,
        has_snapshot_pack: None,
        has_compact_ack: None,
    }
}

async fn run_current_blob_flow(fx: &GoldenFixture) -> CanonicalTrace {
    let (addr, rooms) = spawn_server().await;
    let room = rooms.get_or_create(&fx.room_id).await;

    let seeded_blob = b"parity-blob-bytes".to_vec();
    let seeded_hash = activesync_core::Hash::of(&seeded_blob);
    room.blobs.write().await.put(seeded_blob);

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url).await.expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x61u8; 32]).verifying_key().to_bytes();
    let peer_hex = hex_lower(&peer);

    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": peer_hex,
        "frontier": [],
        "caps": {
            "supports_direct_blob_io": true,
            "supports_ibf": false,
            "supports_mst": false
        },
        "subscribe": ["**"]
    })
    .to_string();
    sink.send(TMessage::Text(hello.into())).await.expect("send hello");

    let mut sequence = Vec::new();
    let mut welcome_has_mst_root = None;

    // Wait for welcome first.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("type").and_then(|t| t.as_str()) == Some("welcome") {
                    sequence.push("welcome".to_string());
                    welcome_has_mst_root = Some(
                        v.get("mst_root")
                            .and_then(|x| x.as_str())
                            .map(|s| !s.is_empty())
                            .unwrap_or(false),
                    );
                    break;
                }
            }
            _ => continue,
        }
    }

    // request-upload should be denied on NoPersistence.
    let upload_req = serde_json::json!({
        "type": "request-upload",
        "hash": seeded_hash.to_hex(),
        "size": 16,
        "content_type": "application/octet-stream"
    })
    .to_string();
    sink.send(TMessage::Text(upload_req.into())).await.expect("send request-upload");

    let mut upload_denied_reason = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("type").and_then(|t| t.as_str()) == Some("upload-denied") {
                    sequence.push("upload-denied".to_string());
                    upload_denied_reason = v.get("reason").and_then(|x| x.as_str()).map(str::to_string);
                    break;
                }
            }
            _ => continue,
        }
    }

    let blob_req = serde_json::json!({
        "type": "blob-request",
        "hashes": [seeded_hash.to_hex()]
    })
    .to_string();
    sink.send(TMessage::Text(blob_req.into())).await.expect("send blob-request");

    let mut blob_pack_count = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("type").and_then(|t| t.as_str()) == Some("blob-pack") {
                    sequence.push("blob-pack".to_string());
                    let n = v
                        .get("blobs")
                        .and_then(|b| b.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);
                    blob_pack_count = Some(n);
                    break;
                }
            }
            _ => continue,
        }
    }

    CanonicalTrace {
        sequence,
        pack_count: 0,
        close_code: None,
        welcome_has_mst_root,
        blob_pack_count,
        upload_denied_reason,
        has_snapshot_pack: None,
        has_compact_ack: None,
    }
}

async fn run_current_tick_compaction(fx: &GoldenFixture) -> CanonicalTrace {
    let (addr, rooms) = spawn_server().await;
    let room = rooms.get_or_create(&fx.room_id).await;

    // Ensure the room has at least one node before compaction.
    let author = SigningKey::from_bytes(&[0x72u8; 32]);
    let node = make_map_set_node(&author, "world/seed", b"v1");
    let (accepted, _, errs) = import_nodes(&room, vec![node]).await;
    assert_eq!(accepted, 1);
    assert!(errs.is_empty());

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url).await.expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x73u8; 32]).verifying_key().to_bytes();
    let peer_hex = hex_lower(&peer);
    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": peer_hex,
        "frontier": [],
        "subscribe": ["**"]
    })
    .to_string();
    sink.send(TMessage::Text(hello.into())).await.expect("send hello");

    let mut sequence = Vec::new();
    let mut got_welcome = false;
    let mut has_snapshot_pack = false;
    let mut has_compact_ack = false;

    // Wait for welcome.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("type").and_then(|t| t.as_str()) == Some("welcome") {
                    sequence.push("welcome".to_string());
                    got_welcome = true;
                    break;
                }
            }
            _ => continue,
        }
    }
    assert!(got_welcome, "tick/compaction scenario did not receive welcome");

    // Start tick.
    let start_tick = serde_json::json!({
        "type": "start-tick",
        "interval_ms": 25,
        "intent_prefix": "intent/"
    })
    .to_string();
    sink.send(TMessage::Text(start_tick.into())).await.expect("send start-tick");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("type").and_then(|t| t.as_str()) == Some("tick-started") {
                    sequence.push("tick-started".to_string());
                    break;
                }
            }
            _ => continue,
        }
    }

    // Stop tick.
    let stop_tick = serde_json::json!({"type": "stop-tick"}).to_string();
    sink.send(TMessage::Text(stop_tick.into())).await.expect("send stop-tick");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("type").and_then(|t| t.as_str()) == Some("tick-stopped") {
                    sequence.push("tick-stopped".to_string());
                    break;
                }
            }
            _ => continue,
        }
    }

    // Trigger compaction and wait for both broadcast+ack in any order.
    let compact = serde_json::json!({"type": "compact-room"}).to_string();
    sink.send(TMessage::Text(compact.into())).await.expect("send compact-room");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                match v.get("type").and_then(|t| t.as_str()) {
                    Some("snapshot-pack") => has_snapshot_pack = true,
                    Some("compact-ack") => has_compact_ack = true,
                    _ => {}
                }
                if has_snapshot_pack && has_compact_ack {
                    break;
                }
            }
            _ => continue,
        }
    }

    CanonicalTrace {
        sequence,
        pack_count: 0,
        close_code: None,
        welcome_has_mst_root: None,
        blob_pack_count: None,
        upload_denied_reason: None,
        has_snapshot_pack: Some(has_snapshot_pack),
        has_compact_ack: Some(has_compact_ack),
    }
}

async fn run_shadow_path(fx: &GoldenFixture) -> CanonicalTrace {
    // Compatibility mode: shadow path is the current adapter until host-core
    // extraction replaces this execution path.
    run_current_path(fx).await
}

fn decode_pack_count(v: &serde_json::Value) -> usize {
    let Some(nodes_b64) = v.get("nodes").and_then(|n| n.as_str()) else {
        return 0;
    };
    let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(nodes_b64) else {
        return 0;
    };
    let Ok(nodes) = activesync_core::unpack_nodes(&raw) else {
        return 0;
    };
    nodes.len()
}

fn token_json(tok: &RoomToken) -> serde_json::Value {
    serde_json::json!({
        "peer_pubkey": hex_lower(&tok.peer_pubkey),
        "expiry": tok.expiry_secs,
        "caps": tok.capabilities,
        "sig": hex_lower(&tok.signature),
    })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
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

fn assert_trace_against_fixture(baseline: &CanonicalTrace, fx: &GoldenFixture) {
    assert_eq!(baseline.sequence, fx.expected_sequence, "message sequence mismatch");
    assert_eq!(baseline.pack_count, fx.expected_pack_count, "pack node count mismatch");
    assert_eq!(baseline.close_code, fx.expected_close_code, "close code mismatch");
    if let Some(expected) = fx.expected_blob_pack_count {
        assert_eq!(baseline.blob_pack_count, Some(expected), "blob-pack count mismatch");
    }
    if let Some(expected) = &fx.expected_upload_denied_reason {
        assert_eq!(baseline.upload_denied_reason.as_deref(), Some(expected.as_str()), "upload-denied reason mismatch");
    }
    if let Some(expected) = fx.expected_welcome_has_mst_root {
        assert_eq!(
            baseline.welcome_has_mst_root,
            Some(expected),
            "mst_root presence mismatch"
        );
    }
    if let Some(expected) = fx.expected_has_snapshot_pack {
        assert_eq!(baseline.has_snapshot_pack, Some(expected), "snapshot-pack presence mismatch");
    }
    if let Some(expected) = fx.expected_has_compact_ack {
        assert_eq!(baseline.has_compact_ack, Some(expected), "compact-ack presence mismatch");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_hello_catchup_single_node_fixture() {
    let fx = load_fixture("hello_catchup_single_node.json");
    assert_eq!(fx.name, "hello_catchup_single_node");

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    // Dual-path parity assertion.
    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_token_expiry_close_fixture() {
    let fx = load_fixture("token_expiry_close_4002.json");
    assert_eq!(fx.name, "token_expiry_close_4002");

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_ibf_mst_negotiation_fixture() {
    let fx = load_fixture("ibf_mst_negotiation_single_node.json");
    assert_eq!(fx.name, "ibf_mst_negotiation_single_node");

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_blob_flow_request_upload_and_fetch_fixture() {
    let fx = load_fixture("blob_flow_request_upload_and_fetch.json");
    assert_eq!(fx.name, "blob_flow_request_upload_and_fetch");

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_tick_compaction_control_fixture() {
    let fx = load_fixture("tick_compaction_start_stop_snapshot.json");
    assert_eq!(fx.name, "tick_compaction_start_stop_snapshot");

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}

