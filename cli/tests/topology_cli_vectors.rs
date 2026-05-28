//! CLI-TOPOLOGY-* vectors: frozen §7a commands over WebSocket against nodalmerge-server.

use std::sync::Arc;
use std::time::Duration;

use axum::{routing::get, Router};
use ed25519_dalek::SigningKey;
use nodalmerge_cli::{hex_lower, run_topology_command, TopologyCommand, TopologyGlobalOpts};
use nodalmerge_core::{MapOp, Op, RoomToken, StateGraph};
use nodalmerge_server::lineage::{snapshot_parent_checkpoint, snapshot_room_canonical_hash};
use nodalmerge_server::room::{import_nodes, Rooms};
use nodalmerge_server::store::{NoPersistence, SharedPersistence};
use nodalmerge_server::ws_handler;
use tempfile::NamedTempFile;

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

async fn spawn_server() -> (std::net::SocketAddr, Rooms, SigningKey) {
    let server_key = SigningKey::from_bytes(&[0xD1u8; 32]);
    let persistence: SharedPersistence = Arc::new(NoPersistence);
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
    (addr, rooms, server_key)
}

fn topology_token(room_id: &str, peer_sk: &SigningKey, room_key: &SigningKey) -> String {
    let tok = RoomToken::sign(
        room_id,
        &peer_sk.verifying_key().to_bytes(),
        now_secs() + 3600,
        &["topology.admin".to_string()],
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

fn globals(
    addr: std::net::SocketAddr,
    parent_id: &str,
    token_json: String,
    peer_seed: [u8; 32],
) -> TopologyGlobalOpts {
    TopologyGlobalOpts {
        server: Some(format!("ws://{addr}/ws/{parent_id}")),
        room: Some(parent_id.to_string()),
        token_json: Some(token_json),
        timeout_secs: 15,
        peer_seed,
    }
}

/// CLI-TOPOLOGY-001: create-child + list-children + show-lineage over WS CLI.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_topology_001_lineage_commands() {
    let parent_id = "cli-parent-001";
    let child_id = "cli-child-001";
    let (addr, rooms, _) = spawn_server().await;

    let room_key = SigningKey::from_bytes(&[0xD2u8; 32]);
    let parent = rooms.get_or_create(parent_id).await;
    *parent.auth_key.write().await = Some(room_key.verifying_key());

    let author = SigningKey::from_bytes(&[0xD3u8; 32]);
    import_nodes(
        &parent,
        vec![make_map_set_node(&author, "world/cli", b"1")],
    )
    .await;
    let checkpoint = snapshot_parent_checkpoint(&parent).await.unwrap();

    let cp_file = NamedTempFile::new().unwrap();
    std::fs::write(
        cp_file.path(),
        serde_json::to_string(&checkpoint).unwrap(),
    )
    .unwrap();

    let peer_sk = SigningKey::from_bytes(&[0xD4u8; 32]);
    let g = globals(
        addr,
        parent_id,
        topology_token(parent_id, &peer_sk, &room_key),
        [0xD4u8; 32],
    );

    let created = run_topology_command(
        &g,
        TopologyCommand::CreateChild {
            parent_room: parent_id.to_string(),
            child_room: child_id.to_string(),
            purpose: "cli-task".to_string(),
            policy: "promotion-based".to_string(),
            created_by: Some("cli-test".to_string()),
            parent_checkpoint_file: cp_file.path().to_path_buf(),
        },
    )
    .await
    .expect("create-child");
    assert_eq!(
        created.get("type").and_then(|v| v.as_str()),
        Some("topology.create-child.completed")
    );

    let listed = run_topology_command(
        &g,
        TopologyCommand::ListChildren {
            parent_room: parent_id.to_string(),
        },
    )
    .await
    .expect("list-children");
    let children = listed.get("children").and_then(|v| v.as_array()).unwrap();
    assert_eq!(children.len(), 1);

    let lineage = run_topology_command(
        &g,
        TopologyCommand::ShowLineage {
            room: child_id.to_string(),
        },
    )
    .await
    .expect("show-lineage");
    assert_eq!(
        lineage.get("type").and_then(|v| v.as_str()),
        Some("topology.describe-lineage.result")
    );
}

/// CLI-TOPOLOGY-002: propose → validate → apply promotion via CLI (maps AUTH-ROOM-003).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_topology_002_promotion_workflow() {
    let parent_id = "cli-parent-002";
    let child_id = "cli-child-002";
    let (addr, rooms, _) = spawn_server().await;

    let room_key = SigningKey::from_bytes(&[0xD5u8; 32]);
    let parent = rooms.get_or_create(parent_id).await;
    *parent.auth_key.write().await = Some(room_key.verifying_key());

    let author = SigningKey::from_bytes(&[0xD6u8; 32]);
    import_nodes(
        &parent,
        vec![make_map_set_node(&author, "world/p", b"p")],
    )
    .await;
    let checkpoint = snapshot_parent_checkpoint(&parent).await.unwrap();
    rooms
        .create_child_room(
            parent_id,
            child_id,
            checkpoint,
            "t".to_string(),
            "mgr".to_string(),
            "promotion-based".to_string(),
        )
        .await
        .unwrap();

    let child = rooms.get_or_create(child_id).await;
    import_nodes(
        &child,
        vec![make_map_set_node(&author, "world/c", b"c")],
    )
    .await;
    let child_hash = snapshot_room_canonical_hash(&child).await.unwrap();

    let peer_sk = SigningKey::from_bytes(&[0xD7u8; 32]);
    let g = globals(
        addr,
        parent_id,
        topology_token(parent_id, &peer_sk, &room_key),
        [0xD7u8; 32],
    );

    let proposed = run_topology_command(
        &g,
        TopologyCommand::ProposePromotion {
            parent_room: parent_id.to_string(),
            child_room: child_id.to_string(),
            child_checkpoint: child_hash,
            payload_ref: "artifact://cli/run02".to_string(),
            idempotency_key: Some("cli-prop-002".to_string()),
        },
    )
    .await
    .expect("propose");
    let proposal_id = proposed
        .get("proposal_id")
        .and_then(|v| v.as_str())
        .unwrap();

    let validated = run_topology_command(
        &g,
        TopologyCommand::ValidatePromotion {
            proposal_id: proposal_id.to_string(),
        },
    )
    .await
    .expect("validate");
    assert_eq!(
        validated.get("type").and_then(|v| v.as_str()),
        Some("topology.validate-promotion.completed")
    );

    let applied = run_topology_command(
        &g,
        TopologyCommand::ApplyPromotion {
            proposal_id: proposal_id.to_string(),
        },
    )
    .await
    .expect("apply");
    assert_eq!(
        applied.get("type").and_then(|v| v.as_str()),
        Some("topology.apply-promotion.completed")
    );
    assert!(applied
        .get("parent_new_canonical_hash")
        .and_then(|v| v.as_str())
        .is_some());
}
