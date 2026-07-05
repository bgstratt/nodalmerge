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

use axum::{routing::get, Router};
use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use futures_util::{SinkExt, StreamExt};
use nodalmerge_core::{
    BlobStore, MapOp, Op, Policy, PolicyDefault, PolicyRule, RoomToken, StateGraph,
};
use nodalmerge_server::lineage::{snapshot_parent_checkpoint, snapshot_room_canonical_hash};
use nodalmerge_server::room::{import_nodes, Rooms};
use nodalmerge_server::store::{DirPersistence, NoPersistence, SharedPersistence};
use nodalmerge_server::ws_handler;
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
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("invalid fixture JSON {}: {e}", p.display()))
}

fn make_map_set_node(sk: &SigningKey, key: &str, value: &[u8]) -> nodalmerge_core::SyncNode {
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

async fn spawn_durable_locked_server(
    room_id: &str,
) -> (std::net::SocketAddr, Rooms, SigningKey, std::path::PathBuf) {
    let tmp_root = std::env::temp_dir().join(format!(
        "nodalmerge-archive-parity-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_root).expect("temp persistence root should be created");

    let server_key = SigningKey::from_bytes(&[0x51u8; 32]);
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&tmp_root).expect("dir persistence should open"));
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
            .expect("archive parity server should run");
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let room_key = SigningKey::from_bytes(&[0xABu8; 32]);
    let target_room = rooms.get_or_create(room_id).await;
    *target_room.auth_key.write().await = Some(room_key.verifying_key());

    (addr, rooms, room_key, tmp_root)
}

async fn run_current_path(fx: &GoldenFixture) -> CanonicalTrace {
    match fx.scenario.as_str() {
        "hello_catchup" => run_current_hello_catchup(fx).await,
        "token_expiry" => run_current_token_expiry(fx).await,
        "blob_flow" => run_current_blob_flow(fx).await,
        "tick_compaction" => run_current_tick_compaction(fx).await,
        "archive_flow" => run_current_archive_flow(fx).await,
        "archive_export_roundtrip_file" => run_current_archive_export_roundtrip_file(fx).await,
        "archive_positive_policy_timeline_transition_non_zero" => {
            run_current_archive_positive_policy_timeline_transition_non_zero(fx).await
        }
        "archive_negative_unsupported_format" => {
            run_current_archive_negative_unsupported_format(fx).await
        }
        "archive_negative_signature_invalid" => {
            run_current_archive_negative_signature_invalid(fx).await
        }
        "archive_negative_payload_digest_policy_invalid" => {
            run_current_archive_negative_payload_digest_policy_invalid(fx).await
        }
        "archive_negative_compatibility_window_unsupported" => {
            run_current_archive_negative_compatibility_window_unsupported(fx).await
        }
        "archive_negative_compatibility_window_no_overlap" => {
            run_current_archive_negative_compatibility_window_no_overlap(fx).await
        }
        "archive_positive_compatibility_window_edge_overlap_lower" => {
            run_current_archive_positive_compatibility_window_edge_overlap_lower(fx).await
        }
        "archive_negative_policy_timeline_hash_mismatch" => {
            run_current_archive_negative_policy_timeline_hash_mismatch(fx).await
        }
        "archive_negative_policy_timeline_cutover_mismatch" => {
            run_current_archive_negative_policy_timeline_cutover_mismatch(fx).await
        }
        "archive_negative_checkpoint_not_found_external" => {
            run_current_archive_negative_checkpoint_not_found_external(fx).await
        }
        "topology_promotion_workflow" => run_current_topology_promotion_workflow(fx).await,
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
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x31u8; 32])
        .verifying_key()
        .to_bytes();
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
            .encode(nodalmerge_core::Ibf::from_ids(&[]).encode());
        hello["ibf"] = serde_json::json!(ibf_b64);
    }
    let hello = hello.to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

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

    let peer = SigningKey::from_bytes(&[0x41u8; 32])
        .verifying_key()
        .to_bytes();
    let peer_hex = hex_lower(&peer);
    let ttl = fx.token_ttl_secs.unwrap_or(2);
    let token = RoomToken::sign(&fx.room_id, &peer, now_secs() + ttl, &[], &room_key);

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": peer_hex,
        "frontier": [],
        "token": token_json(&token)
    })
    .to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

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
    let seeded_hash = nodalmerge_core::Hash::of(&seeded_blob);
    room.blobs.write().await.put(seeded_blob);

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x61u8; 32])
        .verifying_key()
        .to_bytes();
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
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

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
    sink.send(TMessage::Text(upload_req.into()))
        .await
        .expect("send request-upload");

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
                    upload_denied_reason =
                        v.get("reason").and_then(|x| x.as_str()).map(str::to_string);
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
    sink.send(TMessage::Text(blob_req.into()))
        .await
        .expect("send blob-request");

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
    let (addr, rooms, room_key) = spawn_locked_server(&fx.room_id).await;
    let room = rooms.get_or_create(&fx.room_id).await;

    // Ensure the room has at least one node before compaction.
    let author = SigningKey::from_bytes(&[0x72u8; 32]);
    let node = make_map_set_node(&author, "world/seed", b"v1");
    let (accepted, _, errs) = import_nodes(&room, vec![node]).await;
    assert_eq!(accepted, 1);
    assert!(errs.is_empty());

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x73u8; 32])
        .verifying_key()
        .to_bytes();
    let peer_hex = hex_lower(&peer);
    let token = RoomToken::sign(
        &fx.room_id,
        &peer,
        now_secs() + 300,
        &["tick.admin".to_string()],
        &room_key,
    );
    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": peer_hex,
        "frontier": [],
        "token": token_json(&token),
        "subscribe": ["**"]
    })
    .to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

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
    assert!(
        got_welcome,
        "tick/compaction scenario did not receive welcome"
    );

    // Start tick.
    let start_tick = serde_json::json!({
        "type": "start-tick",
        "interval_ms": 25,
        "intent_prefix": "intent/"
    })
    .to_string();
    sink.send(TMessage::Text(start_tick.into()))
        .await
        .expect("send start-tick");

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
    sink.send(TMessage::Text(stop_tick.into()))
        .await
        .expect("send stop-tick");

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
    sink.send(TMessage::Text(compact.into()))
        .await
        .expect("send compact-room");

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

async fn run_current_archive_flow(fx: &GoldenFixture) -> CanonicalTrace {
    let (addr, rooms, room_key, _tmp_root) = spawn_durable_locked_server(&fx.room_id).await;

    let source_room_id = format!("{}-source", fx.room_id);
    let source_room = rooms.get_or_create(&source_room_id).await;
    let source_author = SigningKey::from_bytes(&[0x7Au8; 32]);
    let source_node = make_map_set_node(&source_author, "world/archive-key", b"archive-value");
    let (accepted, _, errs) = import_nodes(&source_room, vec![source_node]).await;
    assert_eq!(accepted, 1);
    assert!(errs.is_empty());

    let source_blob = b"archive-flow-blob".to_vec();
    let source_blob_hash = nodalmerge_core::Hash::of(&source_blob);
    source_room.blobs.write().await.put(source_blob.clone());
    source_room
        .persistence
        .persist_blob(&source_room.room_id, &source_blob_hash, &source_blob);

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    // Use the server key identity (spawn_server uses [0x51;32]) so control-plane
    // authorization is deterministic without relying on capability profile expansion.
    let peer = SigningKey::from_bytes(&[0x51u8; 32])
        .verifying_key()
        .to_bytes();
    let peer_hex = hex_lower(&peer);
    let token = RoomToken::sign(&fx.room_id, &peer, now_secs() + 300, &[], &room_key);

    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": peer_hex,
        "frontier": [],
        "token": token_json(&token),
        "subscribe": ["**"]
    })
    .to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    let mut sequence = Vec::new();
    let mut describe_checkpoint = None;

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
                    break;
                }
            }
            _ => continue,
        }
    }

    let archive_ref = format!("room://{source_room_id}");

    let describe = serde_json::json!({
        "type": "archive.describe",
        "archive_ref": archive_ref
    })
    .to_string();
    sink.send(TMessage::Text(describe.into()))
        .await
        .expect("send archive.describe");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("type").and_then(|t| t.as_str()) == Some("archive.describe.result") {
                    sequence.push("archive.describe.result".to_string());
                    describe_checkpoint = v.get("checkpoint").cloned();
                    break;
                }
                if v.get("type").and_then(|t| t.as_str()) == Some("error") {
                    panic!("archive.describe produced error envelope: {text}");
                }
            }
            _ => continue,
        }
    }

    let validate = serde_json::json!({
        "type": "archive.validate",
        "archive_ref": format!("room://{source_room_id}"),
        "mode": "full_integrity"
    })
    .to_string();
    sink.send(TMessage::Text(validate.into()))
        .await
        .expect("send archive.validate");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("type").and_then(|t| t.as_str()) == Some("archive.validate.result") {
                    sequence.push("archive.validate.result".to_string());
                    break;
                }
                if v.get("type").and_then(|t| t.as_str()) == Some("error") {
                    panic!("archive.validate produced error envelope: {text}");
                }
            }
            _ => continue,
        }
    }

    let import = serde_json::json!({
        "type": "archive.import",
        "archive_ref": format!("room://{source_room_id}"),
        "import_mode": "full_apply",
        "expected_checkpoint": describe_checkpoint
    })
    .to_string();
    sink.send(TMessage::Text(import.into()))
        .await
        .expect("send archive.import");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("type").and_then(|t| t.as_str()) == Some("archive.import.completed") {
                    sequence.push("archive.import.completed".to_string());
                    break;
                }
                if v.get("type").and_then(|t| t.as_str()) == Some("error") {
                    panic!("archive.import produced error envelope: {text}");
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
        has_snapshot_pack: None,
        has_compact_ack: None,
    }
}

async fn run_current_archive_export_roundtrip_file(fx: &GoldenFixture) -> CanonicalTrace {
    let (addr, rooms, room_key, tmp_root) = spawn_durable_locked_server(&fx.room_id).await;

    let source_room_id = format!("{}-source-export", fx.room_id);
    let source_room = rooms.get_or_create(&source_room_id).await;
    let source_author = SigningKey::from_bytes(&[0x6Au8; 32]);
    let source_node = make_map_set_node(&source_author, "world/export-key", b"export-value");
    let (accepted, _, errs) = import_nodes(&source_room, vec![source_node]).await;
    assert_eq!(accepted, 1);
    assert!(errs.is_empty());

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x51u8; 32])
        .verifying_key()
        .to_bytes();
    let peer_hex = hex_lower(&peer);
    let token = RoomToken::sign(&fx.room_id, &peer, now_secs() + 300, &[], &room_key);

    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": peer_hex,
        "frontier": [],
        "token": token_json(&token),
        "subscribe": ["**"]
    })
    .to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    let mut sequence = Vec::new();

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
                    break;
                }
            }
            _ => continue,
        }
    }

    let manifest_path = tmp_root.join("archives").join("roundtrip-export.json");
    let archive_ref = format!("file://{}", manifest_path.display());

    let export = serde_json::json!({
        "type": "archive.export",
        "source_room": source_room_id,
        "archive_ref": archive_ref,
    })
    .to_string();
    sink.send(TMessage::Text(export.into()))
        .await
        .expect("send archive.export");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("type").and_then(|t| t.as_str()) == Some("archive.export.result") {
                    sequence.push("archive.export.result".to_string());
                    assert_eq!(
                        v.get("compatibility_window")
                            .and_then(|cw| cw.get("min_supported"))
                            .and_then(|m| m.as_str()),
                        Some("1")
                    );
                    assert_eq!(
                        v.get("compatibility_window")
                            .and_then(|cw| cw.get("max_supported"))
                            .and_then(|m| m.as_str()),
                        Some("2")
                    );
                    assert_eq!(
                        v.get("payload_digest_policy").and_then(|p| p.as_str()),
                        Some("strict_sha256_v1")
                    );
                    assert!(v
                        .get("policy_timeline_hash")
                        .and_then(|h| h.as_str())
                        .map(|h| !h.is_empty())
                        .unwrap_or(false));
                    assert_eq!(
                        v.get("policy_timeline_cutover_lamport")
                            .and_then(|c| c.as_u64()),
                        Some(0)
                    );
                    assert_eq!(
                        v.get("policy_timeline_transition_cutovers")
                            .and_then(|c| c.as_array())
                            .cloned(),
                        Some(vec![serde_json::json!(0)])
                    );
                    break;
                }
                if v.get("type").and_then(|t| t.as_str()) == Some("error") {
                    panic!("archive.export produced error envelope: {text}");
                }
            }
            _ => continue,
        }
    }

    let import = serde_json::json!({
        "type": "archive.import",
        "archive_ref": format!("file://{}", manifest_path.display()),
        "import_mode": "full_apply",
    })
    .to_string();
    sink.send(TMessage::Text(import.into()))
        .await
        .expect("send archive.import");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("type").and_then(|t| t.as_str()) == Some("archive.import.completed") {
                    sequence.push("archive.import.completed".to_string());
                    break;
                }
                if v.get("type").and_then(|t| t.as_str()) == Some("error") {
                    panic!("archive.import produced error envelope: {text}");
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
        has_snapshot_pack: None,
        has_compact_ack: None,
    }
}

async fn run_current_archive_positive_policy_timeline_transition_non_zero(
    fx: &GoldenFixture,
) -> CanonicalTrace {
    let (addr, rooms, room_key, tmp_root) = spawn_durable_locked_server(&fx.room_id).await;
    let target_room = rooms.get_or_create(&fx.room_id).await;

    let source_room_id = format!("{}-source-policy-transition", fx.room_id);
    let source_room = rooms.get_or_create(&source_room_id).await;
    let source_author = SigningKey::from_bytes(&[0x76u8; 32]);
    let source_node = make_map_set_node(&source_author, "world/export-key", b"export-value");
    let (accepted, _, errs) = import_nodes(&source_room, vec![source_node]).await;
    assert_eq!(accepted, 1);
    assert!(errs.is_empty());

    target_room
        .set_policy(Policy {
            rules: vec![PolicyRule {
                path_glob: "world/**".to_string(),
                can_write: vec![],
                can_read: vec![],
                can_derive: vec![],
            }],
            default: PolicyDefault::DenyAll,
        })
        .await;
    target_room
        .set_policy(Policy {
            rules: vec![PolicyRule {
                path_glob: "world/**".to_string(),
                can_write: vec![source_author.verifying_key().to_bytes()],
                can_read: vec![],
                can_derive: vec![],
            }],
            default: PolicyDefault::AllowAll,
        })
        .await;

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x51u8; 32])
        .verifying_key()
        .to_bytes();
    let peer_hex = hex_lower(&peer);
    let token = RoomToken::sign(&fx.room_id, &peer, now_secs() + 300, &[], &room_key);

    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": peer_hex,
        "frontier": [],
        "token": token_json(&token),
        "subscribe": ["**"]
    })
    .to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    let mut sequence = Vec::new();

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
                    break;
                }
            }
            _ => continue,
        }
    }

    let manifest_path = tmp_root
        .join("archives")
        .join("policy-transition-export.json");
    let archive_ref = format!("file://{}", manifest_path.display());

    let export = serde_json::json!({
        "type": "archive.export",
        "source_room": source_room_id,
        "archive_ref": archive_ref,
    })
    .to_string();
    sink.send(TMessage::Text(export.into()))
        .await
        .expect("send archive.export");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("type").and_then(|t| t.as_str()) == Some("archive.export.result") {
                    sequence.push("archive.export.result".to_string());
                    assert_eq!(
                        v.get("policy_timeline_cutover_lamport")
                            .and_then(|c| c.as_u64()),
                        Some(2)
                    );
                    assert_eq!(
                        v.get("policy_timeline_transition_cutovers")
                            .and_then(|c| c.as_array())
                            .cloned(),
                        Some(vec![
                            serde_json::json!(0),
                            serde_json::json!(1),
                            serde_json::json!(2),
                        ])
                    );
                    break;
                }
                if v.get("type").and_then(|t| t.as_str()) == Some("error") {
                    panic!("archive.export produced error envelope: {text}");
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
        has_snapshot_pack: None,
        has_compact_ack: None,
    }
}

fn write_signed_external_manifest(
    path: &std::path::Path,
    source_room: &str,
    signer: &SigningKey,
    tamper_signature: bool,
) {
    let default_policy_timeline =
        nodalmerge_server::archive_export::policy_timeline_metadata_for_policy(&Policy::default());
    write_signed_external_manifest_with_policy(
        path,
        source_room,
        signer,
        tamper_signature,
        "1",
        "2",
        "strict_sha256_v1",
        &default_policy_timeline.hash_hex,
        default_policy_timeline.cutover_lamport,
    );
}

fn write_signed_external_manifest_with_policy(
    path: &std::path::Path,
    source_room: &str,
    signer: &SigningKey,
    tamper_signature: bool,
    min_supported: &str,
    max_supported: &str,
    payload_digest_policy: &str,
    policy_timeline_hash: &str,
    policy_timeline_cutover_lamport: u64,
) {
    let policy_timeline_transition_cutovers = if policy_timeline_cutover_lamport == 0 {
        vec![0]
    } else {
        vec![0, policy_timeline_cutover_lamport]
    };
    let payload = format!(
        "format_version={}|source_room={}|min_supported={}|max_supported={}|payload_digest_policy={}|policy_timeline_hash={}|policy_timeline_cutover_lamport={}|policy_timeline_transition_cutovers={}",
        "1",
        source_room,
        min_supported,
        max_supported,
        payload_digest_policy,
        policy_timeline_hash,
        policy_timeline_cutover_lamport,
        policy_timeline_transition_cutovers
            .iter()
            .map(u64::to_string)
            .collect::<Vec<String>>()
            .join(","),
    );
    let mut sig = signer.sign(payload.as_bytes()).to_bytes();
    if tamper_signature {
        sig[0] ^= 0x01;
    }

    let manifest = serde_json::json!({
        "format_version": "1",
        "source_room": source_room,
        "compatibility_window": {
            "min_supported": min_supported,
            "max_supported": max_supported
        },
        "payload_digest_policy": payload_digest_policy,
        "policy_timeline_hash": policy_timeline_hash,
        "policy_timeline_cutover_lamport": policy_timeline_cutover_lamport,
        "policy_timeline_transition_cutovers": policy_timeline_transition_cutovers,
        "signature": {
            "public_key": hex_lower(&signer.verifying_key().to_bytes()),
            "signature": hex_lower(&sig),
        }
    });

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("manifest parent directory should exist");
    }
    std::fs::write(path, manifest.to_string()).expect("manifest should be written");
}

async fn run_current_archive_negative_unsupported_format(fx: &GoldenFixture) -> CanonicalTrace {
    let (addr, _rooms, room_key, _tmp_root) = spawn_durable_locked_server(&fx.room_id).await;

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x51u8; 32])
        .verifying_key()
        .to_bytes();
    let token = RoomToken::sign(&fx.room_id, &peer, now_secs() + 300, &[], &room_key);
    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": hex_lower(&peer),
        "frontier": [],
        "token": token_json(&token),
        "subscribe": ["**"]
    })
    .to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    let mut sequence = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("welcome") {
                    sequence.push("welcome".to_string());
                    break;
                }
            }
            _ => continue,
        }
    }

    let validate = serde_json::json!({
        "type": "archive.validate",
        "archive_ref": "ftp://unsupported/archive.nmar",
        "mode": "full_integrity"
    })
    .to_string();
    sink.send(TMessage::Text(validate.into()))
        .await
        .expect("send archive.validate");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("archive.validate.rejected") {
                    sequence.push("archive.validate.rejected".to_string());
                    assert_eq!(
                        v.get("reason_class").and_then(|r| r.as_str()),
                        Some("reject.archive_unsupported_format")
                    );
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
        has_snapshot_pack: None,
        has_compact_ack: None,
    }
}

async fn run_current_archive_negative_signature_invalid(fx: &GoldenFixture) -> CanonicalTrace {
    let (addr, rooms, room_key, tmp_root) = spawn_durable_locked_server(&fx.room_id).await;
    let source_room_id = format!("{}-source-signature", fx.room_id);
    let source_room = rooms.get_or_create(&source_room_id).await;
    let source_author = SigningKey::from_bytes(&[0x7Cu8; 32]);
    let source_node = make_map_set_node(&source_author, "world/a", b"1");
    let _ = import_nodes(&source_room, vec![source_node]).await;

    let signer = SigningKey::from_bytes(&[0x41u8; 32]);
    let manifest_path = tmp_root.join("archives").join("invalid-sig.json");
    write_signed_external_manifest(&manifest_path, &source_room_id, &signer, true);

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x51u8; 32])
        .verifying_key()
        .to_bytes();
    let token = RoomToken::sign(&fx.room_id, &peer, now_secs() + 300, &[], &room_key);
    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": hex_lower(&peer),
        "frontier": [],
        "token": token_json(&token),
        "subscribe": ["**"]
    })
    .to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    let mut sequence = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("welcome") {
                    sequence.push("welcome".to_string());
                    break;
                }
            }
            _ => continue,
        }
    }

    let validate = serde_json::json!({
        "type": "archive.validate",
        "archive_ref": format!("file://{}", manifest_path.display()),
        "mode": "full_integrity"
    })
    .to_string();
    sink.send(TMessage::Text(validate.into()))
        .await
        .expect("send archive.validate");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("archive.validate.rejected") {
                    sequence.push("archive.validate.rejected".to_string());
                    assert_eq!(
                        v.get("reason_class").and_then(|r| r.as_str()),
                        Some("reject.archive_signature_invalid")
                    );
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
        has_snapshot_pack: None,
        has_compact_ack: None,
    }
}

async fn run_current_archive_negative_payload_digest_policy_invalid(
    fx: &GoldenFixture,
) -> CanonicalTrace {
    let (addr, rooms, room_key, tmp_root) = spawn_durable_locked_server(&fx.room_id).await;
    let source_room_id = format!("{}-source-policy", fx.room_id);
    let source_room = rooms.get_or_create(&source_room_id).await;
    let source_author = SigningKey::from_bytes(&[0x7Eu8; 32]);
    let source_node = make_map_set_node(&source_author, "world/a", b"1");
    let _ = import_nodes(&source_room, vec![source_node]).await;

    let signer = SigningKey::from_bytes(&[0x43u8; 32]);
    let manifest_path = tmp_root.join("archives").join("invalid-policy.json");
    write_signed_external_manifest_with_policy(
        &manifest_path,
        &source_room_id,
        &signer,
        false,
        "1",
        "2",
        "legacy_md5_v0",
        &nodalmerge_server::archive_export::policy_timeline_metadata_for_policy(&Policy::default())
            .hash_hex,
        0,
    );

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x51u8; 32])
        .verifying_key()
        .to_bytes();
    let token = RoomToken::sign(&fx.room_id, &peer, now_secs() + 300, &[], &room_key);
    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": hex_lower(&peer),
        "frontier": [],
        "token": token_json(&token),
        "subscribe": ["**"]
    })
    .to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    let mut sequence = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("welcome") {
                    sequence.push("welcome".to_string());
                    break;
                }
            }
            _ => continue,
        }
    }

    let validate = serde_json::json!({
        "type": "archive.validate",
        "archive_ref": format!("file://{}", manifest_path.display()),
        "mode": "full_integrity"
    })
    .to_string();
    sink.send(TMessage::Text(validate.into()))
        .await
        .expect("send archive.validate");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("archive.validate.rejected") {
                    sequence.push("archive.validate.rejected".to_string());
                    assert_eq!(
                        v.get("reason_class").and_then(|r| r.as_str()),
                        Some("reject.archive_policy_timeline_mismatch")
                    );
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
        has_snapshot_pack: None,
        has_compact_ack: None,
    }
}

async fn run_current_archive_negative_compatibility_window_unsupported(
    fx: &GoldenFixture,
) -> CanonicalTrace {
    let (addr, rooms, room_key, tmp_root) = spawn_durable_locked_server(&fx.room_id).await;
    let source_room_id = format!("{}-source-compat", fx.room_id);
    let source_room = rooms.get_or_create(&source_room_id).await;
    let source_author = SigningKey::from_bytes(&[0x7Fu8; 32]);
    let source_node = make_map_set_node(&source_author, "world/a", b"1");
    let _ = import_nodes(&source_room, vec![source_node]).await;

    let signer = SigningKey::from_bytes(&[0x44u8; 32]);
    let manifest_path = tmp_root.join("archives").join("unsupported-window.json");
    write_signed_external_manifest_with_policy(
        &manifest_path,
        &source_room_id,
        &signer,
        false,
        "2",
        "2",
        "strict_sha256_v1",
        &nodalmerge_server::archive_export::policy_timeline_metadata_for_policy(&Policy::default())
            .hash_hex,
        0,
    );

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x51u8; 32])
        .verifying_key()
        .to_bytes();
    let token = RoomToken::sign(&fx.room_id, &peer, now_secs() + 300, &[], &room_key);
    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": hex_lower(&peer),
        "frontier": [],
        "token": token_json(&token),
        "subscribe": ["**"]
    })
    .to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    let mut sequence = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("welcome") {
                    sequence.push("welcome".to_string());
                    break;
                }
            }
            _ => continue,
        }
    }

    let validate = serde_json::json!({
        "type": "archive.validate",
        "archive_ref": format!("file://{}", manifest_path.display()),
        "mode": "full_integrity"
    })
    .to_string();
    sink.send(TMessage::Text(validate.into()))
        .await
        .expect("send archive.validate");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("archive.validate.rejected") {
                    sequence.push("archive.validate.rejected".to_string());
                    assert_eq!(
                        v.get("reason_class").and_then(|r| r.as_str()),
                        Some("reject.archive_unsupported_format")
                    );
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
        has_snapshot_pack: None,
        has_compact_ack: None,
    }
}

async fn run_current_archive_negative_compatibility_window_no_overlap(
    fx: &GoldenFixture,
) -> CanonicalTrace {
    let (addr, rooms, room_key, tmp_root) = spawn_durable_locked_server(&fx.room_id).await;
    let source_room_id = format!("{}-source-compat-no-overlap", fx.room_id);
    let source_room = rooms.get_or_create(&source_room_id).await;
    let source_author = SigningKey::from_bytes(&[0x70u8; 32]);
    let source_node = make_map_set_node(&source_author, "world/a", b"1");
    let _ = import_nodes(&source_room, vec![source_node]).await;

    let signer = SigningKey::from_bytes(&[0x71u8; 32]);
    let manifest_path = tmp_root.join("archives").join("no-overlap-window.json");
    write_signed_external_manifest_with_policy(
        &manifest_path,
        &source_room_id,
        &signer,
        false,
        "3",
        "4",
        "strict_sha256_v1",
        &nodalmerge_server::archive_export::policy_timeline_metadata_for_policy(&Policy::default())
            .hash_hex,
        0,
    );

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x51u8; 32])
        .verifying_key()
        .to_bytes();
    let token = RoomToken::sign(&fx.room_id, &peer, now_secs() + 300, &[], &room_key);
    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": hex_lower(&peer),
        "frontier": [],
        "token": token_json(&token),
        "subscribe": ["**"]
    })
    .to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    let mut sequence = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("welcome") {
                    sequence.push("welcome".to_string());
                    break;
                }
            }
            _ => continue,
        }
    }

    let validate = serde_json::json!({
        "type": "archive.validate",
        "archive_ref": format!("file://{}", manifest_path.display()),
        "mode": "full_integrity"
    })
    .to_string();
    sink.send(TMessage::Text(validate.into()))
        .await
        .expect("send archive.validate");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("archive.validate.rejected") {
                    sequence.push("archive.validate.rejected".to_string());
                    assert_eq!(
                        v.get("reason_class").and_then(|r| r.as_str()),
                        Some("reject.archive_unsupported_format")
                    );
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
        has_snapshot_pack: None,
        has_compact_ack: None,
    }
}

async fn run_current_archive_positive_compatibility_window_edge_overlap_lower(
    fx: &GoldenFixture,
) -> CanonicalTrace {
    let (addr, rooms, room_key, tmp_root) = spawn_durable_locked_server(&fx.room_id).await;
    let source_room_id = format!("{}-source-compat-edge-lower", fx.room_id);
    let source_room = rooms.get_or_create(&source_room_id).await;
    let source_author = SigningKey::from_bytes(&[0x72u8; 32]);
    let source_node = make_map_set_node(&source_author, "world/a", b"1");
    let _ = import_nodes(&source_room, vec![source_node]).await;

    let signer = SigningKey::from_bytes(&[0x73u8; 32]);
    let manifest_path = tmp_root
        .join("archives")
        .join("edge-overlap-lower-window.json");
    write_signed_external_manifest_with_policy(
        &manifest_path,
        &source_room_id,
        &signer,
        false,
        "0",
        "1",
        "strict_sha256_v1",
        &nodalmerge_server::archive_export::policy_timeline_metadata_for_policy(&Policy::default())
            .hash_hex,
        0,
    );

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x51u8; 32])
        .verifying_key()
        .to_bytes();
    let token = RoomToken::sign(&fx.room_id, &peer, now_secs() + 300, &[], &room_key);
    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": hex_lower(&peer),
        "frontier": [],
        "token": token_json(&token),
        "subscribe": ["**"]
    })
    .to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    let mut sequence = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("welcome") {
                    sequence.push("welcome".to_string());
                    break;
                }
            }
            _ => continue,
        }
    }

    let validate = serde_json::json!({
        "type": "archive.validate",
        "archive_ref": format!("file://{}", manifest_path.display()),
        "mode": "full_integrity"
    })
    .to_string();
    sink.send(TMessage::Text(validate.into()))
        .await
        .expect("send archive.validate");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("archive.validate.result") {
                    sequence.push("archive.validate.result".to_string());
                    assert_eq!(v.get("accepted").and_then(|a| a.as_bool()), Some(true));
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
        has_snapshot_pack: None,
        has_compact_ack: None,
    }
}

async fn run_current_archive_negative_policy_timeline_hash_mismatch(
    fx: &GoldenFixture,
) -> CanonicalTrace {
    let (addr, rooms, room_key, tmp_root) = spawn_durable_locked_server(&fx.room_id).await;
    let target_room = rooms.get_or_create(&fx.room_id).await;
    target_room
        .set_policy(Policy {
            rules: vec![PolicyRule {
                path_glob: "world/**".to_string(),
                can_write: vec![],
                can_read: vec![],
                can_derive: vec![],
            }],
            default: PolicyDefault::DenyAll,
        })
        .await;

    let source_room_id = format!("{}-source-policy-hash", fx.room_id);
    let source_room = rooms.get_or_create(&source_room_id).await;
    let source_author = SigningKey::from_bytes(&[0x45u8; 32]);
    let source_node = make_map_set_node(&source_author, "world/a", b"1");
    let _ = import_nodes(&source_room, vec![source_node]).await;

    let signer = SigningKey::from_bytes(&[0x46u8; 32]);
    let manifest_path = tmp_root.join("archives").join("policy-hash-mismatch.json");
    let default_policy_timeline =
        nodalmerge_server::archive_export::policy_timeline_metadata_for_policy(&Policy::default());
    write_signed_external_manifest_with_policy(
        &manifest_path,
        &source_room_id,
        &signer,
        false,
        "1",
        "2",
        "strict_sha256_v1",
        &default_policy_timeline.hash_hex,
        default_policy_timeline.cutover_lamport,
    );

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x51u8; 32])
        .verifying_key()
        .to_bytes();
    let token = RoomToken::sign(&fx.room_id, &peer, now_secs() + 300, &[], &room_key);
    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": hex_lower(&peer),
        "frontier": [],
        "token": token_json(&token),
        "subscribe": ["**"]
    })
    .to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    let mut sequence = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("welcome") {
                    sequence.push("welcome".to_string());
                    break;
                }
            }
            _ => continue,
        }
    }

    let validate = serde_json::json!({
        "type": "archive.validate",
        "archive_ref": format!("file://{}", manifest_path.display()),
        "mode": "full_integrity"
    })
    .to_string();
    sink.send(TMessage::Text(validate.into()))
        .await
        .expect("send archive.validate");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("archive.validate.rejected") {
                    sequence.push("archive.validate.rejected".to_string());
                    assert_eq!(
                        v.get("reason_class").and_then(|r| r.as_str()),
                        Some("reject.archive_policy_timeline_mismatch")
                    );
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
        has_snapshot_pack: None,
        has_compact_ack: None,
    }
}

async fn run_current_archive_negative_policy_timeline_cutover_mismatch(
    fx: &GoldenFixture,
) -> CanonicalTrace {
    let (addr, rooms, room_key, tmp_root) = spawn_durable_locked_server(&fx.room_id).await;

    let source_room_id = format!("{}-source-policy-cutover", fx.room_id);
    let source_room = rooms.get_or_create(&source_room_id).await;
    let source_author = SigningKey::from_bytes(&[0x47u8; 32]);
    let source_node = make_map_set_node(&source_author, "world/a", b"1");
    let _ = import_nodes(&source_room, vec![source_node]).await;

    let signer = SigningKey::from_bytes(&[0x48u8; 32]);
    let manifest_path = tmp_root
        .join("archives")
        .join("policy-cutover-mismatch.json");
    let default_policy_timeline =
        nodalmerge_server::archive_export::policy_timeline_metadata_for_policy(&Policy::default());
    write_signed_external_manifest_with_policy(
        &manifest_path,
        &source_room_id,
        &signer,
        false,
        "1",
        "2",
        "strict_sha256_v1",
        &default_policy_timeline.hash_hex,
        default_policy_timeline.cutover_lamport + 7,
    );

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x51u8; 32])
        .verifying_key()
        .to_bytes();
    let token = RoomToken::sign(&fx.room_id, &peer, now_secs() + 300, &[], &room_key);
    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": hex_lower(&peer),
        "frontier": [],
        "token": token_json(&token),
        "subscribe": ["**"]
    })
    .to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    let mut sequence = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("welcome") {
                    sequence.push("welcome".to_string());
                    break;
                }
            }
            _ => continue,
        }
    }

    let validate = serde_json::json!({
        "type": "archive.validate",
        "archive_ref": format!("file://{}", manifest_path.display()),
        "mode": "full_integrity"
    })
    .to_string();
    sink.send(TMessage::Text(validate.into()))
        .await
        .expect("send archive.validate");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("archive.validate.rejected") {
                    sequence.push("archive.validate.rejected".to_string());
                    assert_eq!(
                        v.get("reason_class").and_then(|r| r.as_str()),
                        Some("reject.archive_policy_timeline_mismatch")
                    );
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
        has_snapshot_pack: None,
        has_compact_ack: None,
    }
}

async fn run_current_archive_negative_checkpoint_not_found_external(
    fx: &GoldenFixture,
) -> CanonicalTrace {
    let (addr, _rooms, room_key, tmp_root) = spawn_durable_locked_server(&fx.room_id).await;

    let signer = SigningKey::from_bytes(&[0x42u8; 32]);
    let object_root = tmp_root.join("object-root");
    let manifest_path = object_root.join("bucket-a").join("missing.json");
    write_signed_external_manifest(&manifest_path, "missing-room", &signer, false);
    std::env::set_var(
        "NODALMERGE_ARCHIVE_OBJECT_ROOT",
        object_root.display().to_string(),
    );

    let url = format!("ws://{addr}/ws/{}", fx.room_id);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0x51u8; 32])
        .verifying_key()
        .to_bytes();
    let token = RoomToken::sign(&fx.room_id, &peer, now_secs() + 300, &[], &room_key);
    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": hex_lower(&peer),
        "frontier": [],
        "token": token_json(&token),
        "subscribe": ["**"]
    })
    .to_string();
    sink.send(TMessage::Text(hello.into()))
        .await
        .expect("send hello");

    let mut sequence = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("welcome") {
                    sequence.push("welcome".to_string());
                    break;
                }
            }
            _ => continue,
        }
    }

    let validate = serde_json::json!({
        "type": "archive.validate",
        "archive_ref": "object://bucket-a/missing.json",
        "mode": "full_integrity"
    })
    .to_string();
    sink.send(TMessage::Text(validate.into()))
        .await
        .expect("send archive.validate");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if v.get("type").and_then(|t| t.as_str()) == Some("archive.validate.rejected") {
                    sequence.push("archive.validate.rejected".to_string());
                    assert_eq!(
                        v.get("reason_class").and_then(|r| r.as_str()),
                        Some("reject.archive_checkpoint_not_found")
                    );
                    break;
                }
            }
            _ => continue,
        }
    }

    std::env::remove_var("NODALMERGE_ARCHIVE_OBJECT_ROOT");

    CanonicalTrace {
        sequence,
        pack_count: 0,
        close_code: None,
        welcome_has_mst_root: None,
        blob_pack_count: None,
        upload_denied_reason: None,
        has_snapshot_pack: None,
        has_compact_ack: None,
    }
}

async fn topology_drain_until_type(
    stream: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    expected_type: &str,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value =
                    serde_json::from_str(&text).expect("topology parity frame should be json");
                if v.get("type").and_then(|t| t.as_str()) == Some(expected_type) {
                    return;
                }
                if v.get("type").and_then(|t| t.as_str()) == Some("error") {
                    panic!("topology parity unexpected error: {text}");
                }
            }
            _ => continue,
        }
    }
    panic!("timed out waiting for {expected_type}");
}

/// Golden path: propose → validate → apply on parent room WS (topology.admin).
async fn run_current_topology_promotion_workflow(fx: &GoldenFixture) -> CanonicalTrace {
    let parent_id = fx.room_id.as_str();
    let child_id = format!("{parent_id}-child");
    let (addr, rooms, room_key) = spawn_locked_server(parent_id).await;

    let author = SigningKey::from_bytes(&[0xC1u8; 32]);
    let parent = rooms.get_or_create(parent_id).await;
    import_nodes(
        &parent,
        vec![make_map_set_node(&author, "world/topo-parent", b"p")],
    )
    .await;
    let checkpoint = snapshot_parent_checkpoint(&parent).await.unwrap();
    rooms
        .create_child_room(
            parent_id,
            &child_id,
            checkpoint,
            "parity-task".to_string(),
            "parity-mgr".to_string(),
            "promotion-based".to_string(),
        )
        .await
        .expect("create child for topology parity");

    let child = rooms.get_or_create(&child_id).await;
    import_nodes(
        &child,
        vec![make_map_set_node(&author, "world/topo-child", b"c")],
    )
    .await;
    let child_hash = snapshot_room_canonical_hash(&child).await.unwrap();

    let url = format!("ws://{addr}/ws/{parent_id}");
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    let peer = SigningKey::from_bytes(&[0xC2u8; 32]);
    let token = RoomToken::sign(
        parent_id,
        &peer.verifying_key().to_bytes(),
        now_secs() + 300,
        &["topology.admin".to_string()],
        &room_key,
    );
    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": hex_lower(&peer.verifying_key().to_bytes()),
        "frontier": [],
        "token": token_json(&token),
    });
    sink.send(TMessage::Text(hello.to_string().into()))
        .await
        .expect("hello");

    let mut sequence = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(TMessage::Text(text)))) => {
                let v: serde_json::Value =
                    serde_json::from_str(&text).expect("topology parity frame should be json");
                if v.get("type").and_then(|t| t.as_str()) == Some("welcome") {
                    sequence.push("welcome".to_string());
                    break;
                }
            }
            _ => continue,
        }
    }
    assert!(
        sequence.iter().any(|s| s == "welcome"),
        "topology parity never received welcome"
    );

    let propose = serde_json::json!({
        "type": "topology.propose-promotion",
        "parent_room_id": parent_id,
        "child_room_id": child_id,
        "child_checkpoint_hash": child_hash,
        "payload_ref": "artifact://parity",
        "idempotency_key": "prop-parity",
    });
    sink.send(TMessage::Text(propose.to_string().into()))
        .await
        .expect("propose");
    topology_drain_until_type(&mut stream, "topology.propose-promotion.completed").await;
    sequence.push("topology.propose-promotion.completed".to_string());

    let validate = serde_json::json!({
        "type": "topology.validate-promotion",
        "proposal_id": "prop-parity",
    });
    sink.send(TMessage::Text(validate.to_string().into()))
        .await
        .expect("validate");
    topology_drain_until_type(&mut stream, "topology.validate-promotion.completed").await;
    sequence.push("topology.validate-promotion.completed".to_string());

    let apply = serde_json::json!({
        "type": "topology.apply-promotion",
        "proposal_id": "prop-parity",
    });
    sink.send(TMessage::Text(apply.to_string().into()))
        .await
        .expect("apply");
    topology_drain_until_type(&mut stream, "topology.apply-promotion.completed").await;
    sequence.push("topology.apply-promotion.completed".to_string());

    CanonicalTrace {
        sequence,
        pack_count: 0,
        close_code: None,
        welcome_has_mst_root: None,
        blob_pack_count: None,
        upload_denied_reason: None,
        has_snapshot_pack: None,
        has_compact_ack: None,
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
    let Ok(nodes) = nodalmerge_core::unpack_nodes(&raw) else {
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
    assert_eq!(
        baseline.sequence, fx.expected_sequence,
        "message sequence mismatch"
    );
    assert_eq!(
        baseline.pack_count, fx.expected_pack_count,
        "pack node count mismatch"
    );
    assert_eq!(
        baseline.close_code, fx.expected_close_code,
        "close code mismatch"
    );
    if let Some(expected) = fx.expected_blob_pack_count {
        assert_eq!(
            baseline.blob_pack_count,
            Some(expected),
            "blob-pack count mismatch"
        );
    }
    if let Some(expected) = &fx.expected_upload_denied_reason {
        assert_eq!(
            baseline.upload_denied_reason.as_deref(),
            Some(expected.as_str()),
            "upload-denied reason mismatch"
        );
    }
    if let Some(expected) = fx.expected_welcome_has_mst_root {
        assert_eq!(
            baseline.welcome_has_mst_root,
            Some(expected),
            "mst_root presence mismatch"
        );
    }
    if let Some(expected) = fx.expected_has_snapshot_pack {
        assert_eq!(
            baseline.has_snapshot_pack,
            Some(expected),
            "snapshot-pack presence mismatch"
        );
    }
    if let Some(expected) = fx.expected_has_compact_ack {
        assert_eq!(
            baseline.has_compact_ack,
            Some(expected),
            "compact-ack presence mismatch"
        );
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_archive_runtime_adapter_fixture() {
    let fx = load_fixture("archive_runtime_adapter_room_ref.json");
    assert_eq!(fx.name, "archive_runtime_adapter_room_ref");

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_archive_export_runtime_fixture() {
    let fx = load_fixture("archive_runtime_export_file_ref.json");
    assert_eq!(fx.name, "archive_runtime_export_file_ref");

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_archive_positive_policy_timeline_transition_non_zero_fixture() {
    let fx = load_fixture("archive_positive_policy_timeline_transition_non_zero.json");
    assert_eq!(
        fx.name,
        "archive_positive_policy_timeline_transition_non_zero"
    );

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_archive_negative_unsupported_format_fixture() {
    let fx = load_fixture("archive_negative_unsupported_format.json");
    assert_eq!(fx.name, "archive_negative_unsupported_format");

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_archive_negative_signature_invalid_fixture() {
    let fx = load_fixture("archive_negative_signature_invalid.json");
    assert_eq!(fx.name, "archive_negative_signature_invalid");

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_archive_negative_payload_digest_policy_invalid_fixture() {
    let fx = load_fixture("archive_negative_payload_digest_policy_invalid.json");
    assert_eq!(fx.name, "archive_negative_payload_digest_policy_invalid");

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_archive_negative_compatibility_window_unsupported_fixture() {
    let fx = load_fixture("archive_negative_compatibility_window_unsupported.json");
    assert_eq!(fx.name, "archive_negative_compatibility_window_unsupported");

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_archive_negative_compatibility_window_no_overlap_fixture() {
    let fx = load_fixture("archive_negative_compatibility_window_no_overlap.json");
    assert_eq!(fx.name, "archive_negative_compatibility_window_no_overlap");

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_archive_positive_compatibility_window_edge_overlap_lower_fixture() {
    let fx = load_fixture("archive_positive_compatibility_window_edge_overlap_lower.json");
    assert_eq!(
        fx.name,
        "archive_positive_compatibility_window_edge_overlap_lower"
    );

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_archive_negative_policy_timeline_hash_mismatch_fixture() {
    let fx = load_fixture("archive_negative_policy_timeline_hash_mismatch.json");
    assert_eq!(fx.name, "archive_negative_policy_timeline_hash_mismatch");

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_archive_negative_policy_timeline_cutover_mismatch_fixture() {
    let fx = load_fixture("archive_negative_policy_timeline_cutover_mismatch.json");
    assert_eq!(fx.name, "archive_negative_policy_timeline_cutover_mismatch");

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_archive_negative_checkpoint_not_found_external_fixture() {
    let fx = load_fixture("archive_negative_checkpoint_not_found_external.json");
    assert_eq!(fx.name, "archive_negative_checkpoint_not_found_external");

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_topology_promotion_workflow_fixture() {
    let fx = load_fixture("topology_promotion_workflow.json");
    assert_eq!(fx.name, "topology_promotion_workflow");

    let baseline = run_current_path(&fx).await;
    let shadow = run_shadow_path(&fx).await;

    assert_eq!(baseline, shadow, "baseline and shadow traces diverged");
    assert_trace_against_fixture(&baseline, &fx);
}
