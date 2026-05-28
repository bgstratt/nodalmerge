//! CLI-ARCHIVE-001: archive describe over WebSocket (locked room + archive.read token).

use std::sync::Arc;
use std::time::Duration;

use axum::{routing::get, Router};
use ed25519_dalek::SigningKey;
use nodalmerge_cli::{hex_lower, run_archive_command, ArchiveCommand, TopologyGlobalOpts};
use nodalmerge_core::{MapOp, Op, RoomToken, StateGraph};
use nodalmerge_server::room::{import_nodes, Rooms};
use nodalmerge_server::store::{DirPersistence, SharedPersistence};
use nodalmerge_server::ws_handler;
use tempfile::TempDir;

fn make_map_set_node(sk: &SigningKey, key: &str, val: &[u8]) -> nodalmerge_core::SyncNode {
    let mut g = StateGraph::new();
    let id = g
        .apply_local(
            sk,
            0,
            vec![Op::Map(MapOp::Set {
                key: key.into(),
                value: val.to_vec(),
            })],
        )
        .expect("apply_local");
    g.get_nodes(&[id]).into_iter().next().unwrap().clone()
}

async fn spawn_server() -> (std::net::SocketAddr, Rooms, SigningKey, TempDir) {
    let data_dir = TempDir::new().expect("tempdir");
    let server_key = SigningKey::from_bytes(&[0xE1u8; 32]);
    let persistence: SharedPersistence = Arc::new(
        DirPersistence::open(data_dir.path()).expect("dir persistence"),
    );
    let rooms = Rooms::new(server_key.clone(), persistence, 512, 0, 0);
    let app = Router::new()
        .route("/ws/:room_id", get(ws_handler::handler))
        .with_state(rooms.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, rooms, server_key, data_dir)
}

fn capability_token(
    room_id: &str,
    peer_sk: &SigningKey,
    room_key: &SigningKey,
    caps: &[&str],
) -> String {
    let capabilities: Vec<String> = caps.iter().map(|c| (*c).to_string()).collect();
    let tok = RoomToken::sign(
        room_id,
        &peer_sk.verifying_key().to_bytes(),
        now_secs() + 3600,
        &capabilities,
        room_key,
    );
    serde_json::json!({
        "peer_pubkey": hex_lower(&tok.peer_pubkey),
        "expiry": tok.expiry_secs,
        "caps": tok.capabilities,
        "sig": hex_lower(&tok.signature),
    })
    .to_string()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// CLI-ARCHIVE-001: describe `room://` ref with archive.read capability.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_archive_001_describe_room_ref() {
    let room_id = "cli-archive-001";
    let (addr, rooms, _, _data_dir) = spawn_server().await;

    let room_key = SigningKey::from_bytes(&[0xE2u8; 32]);
    let parent = rooms.get_or_create(room_id).await;
    *parent.auth_key.write().await = Some(room_key.verifying_key());

    let author = SigningKey::from_bytes(&[0xE3u8; 32]);
    let node = make_map_set_node(&author, "world/archive", b"cli");
    import_nodes(&parent, vec![node.clone()]).await;
    parent
        .persistence
        .persist_nodes(room_id, &[&node]);

    let peer_sk = SigningKey::from_bytes(&[0xE4u8; 32]);
    let token_json = capability_token(room_id, &peer_sk, &room_key, &["archive.read"]);

    let globals = TopologyGlobalOpts {
        server: Some(format!("ws://{addr}/ws/{room_id}")),
        room: Some(room_id.to_string()),
        token_json: Some(token_json),
        timeout_secs: 15,
        peer_seed: [0xE4u8; 32],
    };

    let result = run_archive_command(
        &globals,
        ArchiveCommand::Describe {
            archive_ref: format!("room://{room_id}"),
        },
    )
    .await
    .expect("archive describe");

    assert_eq!(
        result.get("type").and_then(|v| v.as_str()),
        Some("archive.describe.result")
    );
}
