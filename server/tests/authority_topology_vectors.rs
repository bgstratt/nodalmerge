//! AUTH-ROOM-* topology / lineage vectors (Wave 2 Phase B).

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{routing::get, Router};
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use nodalmerge_core::{canonical_hash, replay};
use nodalmerge_core::{MapOp, Op, PromotionReasonClass, RoomToken, StateGraph};
use nodalmerge_server::lineage::{snapshot_parent_checkpoint, snapshot_room_canonical_hash};
use nodalmerge_server::promotion::{
    process_topology_apply_promotion, process_topology_propose_promotion,
    process_topology_validate_promotion,
};
use nodalmerge_server::room::{import_nodes, Rooms};
use nodalmerge_server::store::{DirPersistence, NoPersistence, SharedPersistence};
use nodalmerge_server::ws_handler;
use tokio_tungstenite::tungstenite::Message as TMessage;

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
    let server_key = SigningKey::from_bytes(&[0x91u8; 32]);
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

async fn lock_room(rooms: &Rooms, room_id: &str) -> SigningKey {
    let room_key = SigningKey::from_bytes(&[0xABu8; 32]);
    let room = rooms.get_or_create(room_id).await;
    *room.auth_key.write().await = Some(room_key.verifying_key());
    room_key
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

fn token_json(tok: &RoomToken) -> serde_json::Value {
    serde_json::json!({
        "peer_pubkey": hex_lower(&tok.peer_pubkey),
        "expiry": tok.expiry_secs,
        "caps": tok.capabilities,
        "sig": hex_lower(&tok.signature),
    })
}

/// AUTH-ROOM-002 (shape): child creation rejects stale parent checkpoint hash.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_room_002_child_rejects_parent_checkpoint_mismatch() {
    let parent_id = "auth-parent-002";
    let (_addr, rooms, _server_key) = spawn_server().await;

    let author = SigningKey::from_bytes(&[0x92u8; 32]);
    let node = make_map_set_node(&author, "world/x", b"1");
    let parent = rooms.get_or_create(parent_id).await;
    import_nodes(&parent, vec![node]).await;

    let bad_checkpoint = nodalmerge_core::ParentCheckpoint {
        frontier: vec!["seq:1".to_string()],
        canonical_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .to_string(),
        policy_timeline_hash: None,
    };

    let err = rooms
        .create_child_room(
            parent_id,
            "auth-child-002",
            bad_checkpoint,
            "task-a".to_string(),
            "test".to_string(),
            "reference-only".to_string(),
        )
        .await
        .expect_err("must reject");

    assert_eq!(
        err.reason_class,
        nodalmerge_core::LineageReasonClass::ParentCheckpointMismatch
    );
}

/// Lineage metadata on child rooms + list-children (Rooms API).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_room_lineage_create_and_list_via_rooms_api() {
    let parent_id = "auth-parent-api";
    let child_id = "auth-child-api";
    let (_addr, rooms, _) = spawn_server().await;

    let author = SigningKey::from_bytes(&[0x94u8; 32]);
    let parent = rooms.get_or_create(parent_id).await;
    import_nodes(&parent, vec![make_map_set_node(&author, "world/y", b"2")]).await;

    let checkpoint = snapshot_parent_checkpoint(&parent).await.expect("snapshot");
    let created = rooms
        .create_child_room(
            parent_id,
            child_id,
            checkpoint,
            "worker-task".to_string(),
            "manager-1".to_string(),
            "promotion-based".to_string(),
        )
        .await
        .expect("create child");

    assert_eq!(created.child_room_id, child_id);
    let child = rooms.get_or_create(child_id).await;
    let lineage = child.lineage.read().await.clone().expect("lineage");
    assert_eq!(lineage.parent_room_id, parent_id);

    let listed = rooms.list_children(parent_id).await.expect("list");
    assert_eq!(listed.children.len(), 1);
}

/// Lineage children index cap keeps only the most recent child ids in-memory.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_room_lineage_children_index_cap_enforced() {
    let parent_id = "auth-parent-cap";
    let server_key = SigningKey::from_bytes(&[0x98u8; 32]);
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let rooms = Rooms::new_with_topology_limits(
        server_key,
        persistence,
        512,
        0,
        0,
        4,
        32,
        Some(2),
    );

    let author = SigningKey::from_bytes(&[0x95u8; 32]);
    let parent = rooms.get_or_create(parent_id).await;
    import_nodes(&parent, vec![make_map_set_node(&author, "world/cap", b"1")]).await;
    let checkpoint = snapshot_parent_checkpoint(&parent).await.expect("snapshot");

    for child in ["auth-child-cap-1", "auth-child-cap-2", "auth-child-cap-3"] {
        rooms
            .create_child_room(
                parent_id,
                child,
                checkpoint.clone(),
                "worker-task".to_string(),
                "manager-1".to_string(),
                "promotion-based".to_string(),
            )
            .await
            .expect("create child");
    }

    let listed = rooms.list_children(parent_id).await.expect("list");
    let ids: Vec<String> = listed.children.iter().map(|c| c.child_room_id.clone()).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&"auth-child-cap-2".to_string()));
    assert!(ids.contains(&"auth-child-cap-3".to_string()));
}

/// WS path: topology.create-child with topology.admin capability token.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_room_topology_create_child_ws_with_admin_cap() {
    let parent_id = "auth-parent-ws";
    let child_id = "auth-child-ws";
    let (addr, rooms, _) = spawn_server().await;
    let room_key = lock_room(&rooms, parent_id).await;

    let author = SigningKey::from_bytes(&[0x96u8; 32]);
    let parent = rooms.get_or_create(parent_id).await;
    import_nodes(&parent, vec![make_map_set_node(&author, "world/z", b"3")]).await;
    let checkpoint = snapshot_parent_checkpoint(&parent).await.expect("snapshot");

    let peer_sk = SigningKey::from_bytes(&[0x97u8; 32]);
    let peer_pk = peer_sk.verifying_key().to_bytes();
    let token = RoomToken::sign(
        parent_id,
        &peer_pk,
        now_secs() + 3600,
        &["topology.admin".to_string()],
        &room_key,
    );

    let url = format!("ws://{addr}/ws/{parent_id}");
    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();

    sink.send(TMessage::Text(
        serde_json::json!({
            "type": "hello",
            "pubkey": hex_lower(&peer_pk),
            "frontier": [],
            "token": token_json(&token),
            "subscribe": ["**"]
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();

    drain_until_welcome(&mut stream).await;

    sink.send(TMessage::Text(
        serde_json::json!({
            "type": "topology.create-child",
            "parent_room_id": parent_id,
            "child_room_id": child_id,
            "child_purpose": "ws-task",
            "created_by": "operator",
            "promotion_policy_id": "reference-only",
            "parent_checkpoint": checkpoint,
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();

    let mut completed = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(TMessage::Text(t)))) =
            tokio::time::timeout(Duration::from_millis(500), stream.next()).await
        {
            if t.contains("topology.create-child.completed") {
                completed = true;
                break;
            }
            if t.contains("reject.") {
                panic!("unexpected reject: {t}");
            }
        }
    }
    assert!(
        completed,
        "expected topology.create-child.completed over WS"
    );
}

/// AUTH-ROOM-003: propose → validate → apply yields stable parent canonical hash.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_room_003_promotion_happy_path_via_rooms_api() {
    let parent_id = "auth-parent-promo-003";
    let child_id = "auth-child-promo-003";
    let (_addr, rooms, server_key) = spawn_server().await;

    let author = SigningKey::from_bytes(&[0xA1u8; 32]);
    let parent = rooms.get_or_create(parent_id).await;
    import_nodes(
        &parent,
        vec![make_map_set_node(&author, "world/p", b"parent")],
    )
    .await;
    let parent_hash_before = snapshot_room_canonical_hash(&parent).await.unwrap();

    let checkpoint = snapshot_parent_checkpoint(&parent).await.unwrap();
    rooms
        .create_child_room(
            parent_id,
            child_id,
            checkpoint,
            "promo-task".to_string(),
            "mgr".to_string(),
            "promotion-based".to_string(),
        )
        .await
        .unwrap();

    let child = rooms.get_or_create(child_id).await;
    import_nodes(
        &child,
        vec![make_map_set_node(&author, "world/c", b"child-outcome")],
    )
    .await;
    let child_hash = snapshot_room_canonical_hash(&child).await.unwrap();

    let propose_msg = serde_json::json!({
        "parent_room_id": parent_id,
        "child_room_id": child_id,
        "child_checkpoint_hash": child_hash,
        "payload_ref": "artifact://run03/outcome",
        "idempotency_key": "prop-003",
    });
    let proposed = process_topology_propose_promotion(&rooms, &propose_msg)
        .await
        .expect("propose");
    assert_eq!(proposed.proposal_id, "prop-003");

    let validated = process_topology_validate_promotion(
        &rooms,
        &serde_json::json!({ "proposal_id": proposed.proposal_id }),
    )
    .await
    .expect("validate");

    let applied = process_topology_apply_promotion(
        &rooms,
        &server_key,
        &serde_json::json!({ "proposal_id": validated.proposal_id }),
    )
    .await
    .expect("apply");

    assert_ne!(applied.parent_new_canonical_hash, parent_hash_before);

    let applied_again = process_topology_apply_promotion(
        &rooms,
        &server_key,
        &serde_json::json!({ "proposal_id": validated.proposal_id }),
    )
    .await
    .expect("apply idempotent");
    assert_eq!(
        applied_again.parent_new_canonical_hash,
        applied.parent_new_canonical_hash
    );
}

/// AUTH-ROOM-004: stale child checkpoint rejected at propose.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_room_004_promotion_rejects_child_checkpoint_mismatch() {
    let parent_id = "auth-parent-promo-004";
    let child_id = "auth-child-promo-004";
    let (_addr, rooms, _) = spawn_server().await;

    let author = SigningKey::from_bytes(&[0xA2u8; 32]);
    let parent = rooms.get_or_create(parent_id).await;
    import_nodes(&parent, vec![make_map_set_node(&author, "world/p4", b"x")]).await;
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
    import_nodes(&child, vec![make_map_set_node(&author, "world/c4", b"z")]).await;

    let err = process_topology_propose_promotion(
        &rooms,
        &serde_json::json!({
            "parent_room_id": parent_id,
            "child_room_id": child_id,
            "child_checkpoint_hash": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "payload_ref": "artifact://bad",
        }),
    )
    .await
    .expect_err("must reject");

    assert_eq!(
        err.reason_class,
        nodalmerge_core::PromotionReasonClass::ChildCheckpointMismatch
    );
}

/// AUTH-ROOM-004: reference-only policy denies promotion propose.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_room_004_promotion_rejects_reference_only_policy() {
    let parent_id = "auth-parent-promo-004b";
    let child_id = "auth-child-promo-004b";
    let (_addr, rooms, _) = spawn_server().await;

    let author = SigningKey::from_bytes(&[0xA3u8; 32]);
    let parent = rooms.get_or_create(parent_id).await;
    import_nodes(&parent, vec![make_map_set_node(&author, "world/p4b", b"y")]).await;
    let checkpoint = snapshot_parent_checkpoint(&parent).await.unwrap();
    rooms
        .create_child_room(
            parent_id,
            child_id,
            checkpoint,
            "t".to_string(),
            "mgr".to_string(),
            "reference-only".to_string(),
        )
        .await
        .unwrap();

    let child = rooms.get_or_create(child_id).await;
    import_nodes(&child, vec![make_map_set_node(&author, "world/c4b", b"z")]).await;
    let child_hash = snapshot_room_canonical_hash(&child).await.unwrap();

    let err = process_topology_propose_promotion(
        &rooms,
        &serde_json::json!({
            "parent_room_id": parent_id,
            "child_room_id": child_id,
            "child_checkpoint_hash": child_hash,
            "payload_ref": "artifact://ref-only",
        }),
    )
    .await
    .expect_err("reference-only must deny");

    assert_eq!(
        err.reason_class,
        nodalmerge_core::PromotionReasonClass::PolicyDenied
    );
}

/// AUTH-ROOM-005: parent replay retains promotion audit artifact after apply.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_room_005_parent_replay_includes_promotion_audit() {
    let parent_id = "auth-parent-promo-005";
    let child_id = "auth-child-promo-005";
    let (_addr, rooms, server_key) = spawn_server().await;

    let author = SigningKey::from_bytes(&[0xA4u8; 32]);
    let parent = rooms.get_or_create(parent_id).await;
    import_nodes(&parent, vec![make_map_set_node(&author, "world/p5", b"p")]).await;
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
    import_nodes(&child, vec![make_map_set_node(&author, "world/c5", b"c")]).await;
    let child_hash = snapshot_room_canonical_hash(&child).await.unwrap();

    let proposed = process_topology_propose_promotion(
        &rooms,
        &serde_json::json!({
            "parent_room_id": parent_id,
            "child_room_id": child_id,
            "child_checkpoint_hash": child_hash,
            "payload_ref": "artifact://replay",
            "idempotency_key": "prop-005",
        }),
    )
    .await
    .unwrap();
    process_topology_validate_promotion(
        &rooms,
        &serde_json::json!({ "proposal_id": proposed.proposal_id }),
    )
    .await
    .unwrap();
    let applied = process_topology_apply_promotion(
        &rooms,
        &server_key,
        &serde_json::json!({ "proposal_id": proposed.proposal_id }),
    )
    .await
    .unwrap();

    let parent = rooms.get_or_create(parent_id).await;
    let nodes: Vec<nodalmerge_core::SyncNode> = {
        let graph = parent.graph.read().await;
        let ids = graph.all_node_ids();
        graph.get_nodes(&ids).into_iter().cloned().collect()
    };
    let replayed = replay(&nodes, None).unwrap();
    assert!(replayed.map.contains_key(&applied.audit_key));
    assert_eq!(
        canonical_hash(&replayed.map).to_hex(),
        applied.parent_new_canonical_hash
    );
}

async fn wait_for_canonical_hash(room: &nodalmerge_server::room::Room, expected: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if snapshot_room_canonical_hash(room).await.ok().as_deref() == Some(expected) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("room did not hydrate to expected canonical hash within 5s");
}

/// AUTH-ROOM-006: validated promotion survives simulated server restart when `--store` is enabled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_room_006_promotion_survives_store_restart() {
    let parent_id = "auth-parent-promo-006";
    let child_id = "auth-child-promo-006";
    let server_key = SigningKey::from_bytes(&[0xA5u8; 32]);

    let dir = std::env::temp_dir().join(format!(
        "nodalmerge-auth-006-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&dir).expect("dir persistence"));

    let rooms_before = Rooms::new(server_key.clone(), Arc::clone(&persistence), 512, 0, 0);

    let author = SigningKey::from_bytes(&[0xA6u8; 32]);
    let parent = rooms_before.get_or_create(parent_id).await;
    import_nodes(&parent, vec![make_map_set_node(&author, "world/p6", b"p")]).await;
    let parent_hash = snapshot_room_canonical_hash(&parent).await.unwrap();
    let checkpoint = snapshot_parent_checkpoint(&parent).await.unwrap();
    rooms_before
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

    let child = rooms_before.get_or_create(child_id).await;
    import_nodes(&child, vec![make_map_set_node(&author, "world/c6", b"c")]).await;
    let child_hash = snapshot_room_canonical_hash(&child).await.unwrap();

    let proposed = process_topology_propose_promotion(
        &rooms_before,
        &serde_json::json!({
            "parent_room_id": parent_id,
            "child_room_id": child_id,
            "child_checkpoint_hash": child_hash,
            "payload_ref": "artifact://restart",
            "idempotency_key": "prop-006",
        }),
    )
    .await
    .unwrap();

    process_topology_validate_promotion(
        &rooms_before,
        &serde_json::json!({ "proposal_id": proposed.proposal_id }),
    )
    .await
    .unwrap();

    drop(rooms_before);

    // Simulated restart: new Rooms registry, same on-disk store + promotion DB.
    let rooms_after = Rooms::new(server_key.clone(), persistence, 512, 0, 0);

    let parent_after = rooms_after.get_or_create(parent_id).await;
    let child_after = rooms_after.get_or_create(child_id).await;
    wait_for_canonical_hash(&parent_after, &parent_hash).await;
    wait_for_canonical_hash(&child_after, &child_hash).await;

    let applied = process_topology_apply_promotion(
        &rooms_after,
        &server_key,
        &serde_json::json!({ "proposal_id": proposed.proposal_id }),
    )
    .await
    .expect("apply after restart");

    assert_ne!(applied.parent_new_canonical_hash, parent_hash);
    assert!(applied.audit_key.contains("prop-006"));

    let _ = std::fs::remove_dir_all(&dir);
}

/// AUTH-ROOM-006 (lineage durability): room lineage metadata survives restart
/// with durable store and remains usable for describe/list/propose workflows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_room_006_lineage_survives_store_restart() {
    let parent_id = "auth-parent-lineage-006";
    let child_id = "auth-child-lineage-006";
    let server_key = SigningKey::from_bytes(&[0xB1u8; 32]);

    let dir = std::env::temp_dir().join(format!(
        "nodalmerge-auth-lineage-006-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&dir).expect("dir persistence"));

    let rooms_before = Rooms::new(server_key.clone(), Arc::clone(&persistence), 512, 0, 0);

    let author = SigningKey::from_bytes(&[0xB2u8; 32]);
    let parent = rooms_before.get_or_create(parent_id).await;
    import_nodes(&parent, vec![make_map_set_node(&author, "world/p6l", b"p")]).await;
    let checkpoint = snapshot_parent_checkpoint(&parent).await.unwrap();
    rooms_before
        .create_child_room(
            parent_id,
            child_id,
            checkpoint,
            "lineage-test".to_string(),
            "mgr".to_string(),
            "promotion-based".to_string(),
        )
        .await
        .unwrap();

    let child = rooms_before.get_or_create(child_id).await;
    import_nodes(&child, vec![make_map_set_node(&author, "world/c6l", b"c")]).await;
    let child_hash = snapshot_room_canonical_hash(&child).await.unwrap();
    drop(rooms_before);

    let rooms_after = Rooms::new(server_key.clone(), persistence, 512, 0, 0);
    let parent_after = rooms_after.get_or_create(parent_id).await;
    let child_after = rooms_after.get_or_create(child_id).await;
    wait_for_canonical_hash(&child_after, &child_hash).await;

    let described = rooms_after
        .describe_lineage(child_id)
        .await
        .expect("describe-lineage after restart");
    let lineage = described.lineage.expect("lineage exists");
    assert_eq!(lineage.parent_room_id, parent_id);
    assert_eq!(lineage.child_purpose, "lineage-test");

    let listed = rooms_after
        .list_children(parent_id)
        .await
        .expect("list-children after restart");
    assert!(listed.children.iter().any(|c| c.child_room_id == child_id));

    let proposed = process_topology_propose_promotion(
        &rooms_after,
        &serde_json::json!({
            "parent_room_id": parent_id,
            "child_room_id": child_id,
            "child_checkpoint_hash": child_hash,
            "payload_ref": "artifact://lineage-restart",
            "idempotency_key": "prop-006-lineage",
        }),
    )
    .await
    .expect("propose after restart uses durable lineage");
    assert_eq!(proposed.child_room_id, child_id);
    assert_eq!(proposed.parent_room_id, parent_id);

    drop(parent_after);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Phase E baseline: large room-family create/list pressure with retention cap and
/// promotion flow still succeeding under high child cardinality.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_room_phasee_large_family_baseline_and_policy_tuning() {
    let parent_id = "auth-parent-phasee-large-family";
    let server_key = SigningKey::from_bytes(&[0xC1u8; 32]);
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let rooms = Rooms::new_with_topology_limits(
        server_key.clone(),
        persistence,
        512,
        0,
        0,
        4,    // NODALMERGE_TOPOLOGY_PROMOTION_MAX_INFLIGHT equivalent
        256,  // NODALMERGE_TOPOLOGY_PROMOTION_MAX_QUEUE equivalent
        Some(128), // NODALMERGE_LINEAGE_CHILDREN_INDEX_MAX equivalent
    );

    let author = SigningKey::from_bytes(&[0xC2u8; 32]);
    let parent = rooms.get_or_create(parent_id).await;
    import_nodes(
        &parent,
        vec![make_map_set_node(&author, "world/phasee-root", b"ready")],
    )
    .await;
    let parent_hash_before = snapshot_room_canonical_hash(&parent).await.unwrap();
    let checkpoint = snapshot_parent_checkpoint(&parent).await.unwrap();

    const TOTAL_CHILDREN: usize = 600;
    const INDEX_CAP: usize = 128;
    let create_start = Instant::now();
    for i in 0..TOTAL_CHILDREN {
        let child_id = format!("auth-child-phasee-{i:04}");
        rooms
            .create_child_room(
                parent_id,
                &child_id,
                checkpoint.clone(),
                "phasee-load".to_string(),
                "phasee-runner".to_string(),
                "promotion-based".to_string(),
            )
            .await
            .expect("create child");
    }
    let create_elapsed = create_start.elapsed();

    let list_start = Instant::now();
    let listed = rooms.list_children(parent_id).await.expect("list children");
    let list_elapsed = list_start.elapsed();

    let listed_ids: Vec<String> = listed
        .children
        .iter()
        .map(|child| child.child_room_id.clone())
        .collect();
    assert_eq!(listed_ids.len(), INDEX_CAP);

    // Retention policy should keep the newest child ids under pressure.
    for i in (TOTAL_CHILDREN - INDEX_CAP)..TOTAL_CHILDREN {
        let expected = format!("auth-child-phasee-{i:04}");
        assert!(listed_ids.contains(&expected), "missing retained child {expected}");
    }

    // Promotion flow remains functional even after large-family cardinality load.
    let promoted_child_id = format!("auth-child-phasee-{:04}", TOTAL_CHILDREN - 1);
    let promoted_child = rooms.get_or_create(&promoted_child_id).await;
    import_nodes(
        &promoted_child,
        vec![make_map_set_node(
            &author,
            "world/phasee-promotion",
            b"child-outcome",
        )],
    )
    .await;
    let promoted_child_hash = snapshot_room_canonical_hash(&promoted_child).await.unwrap();

    let proposed = process_topology_propose_promotion(
        &rooms,
        &serde_json::json!({
            "parent_room_id": parent_id,
            "child_room_id": promoted_child_id,
            "child_checkpoint_hash": promoted_child_hash,
            "payload_ref": "artifact://phasee/large-family/run01",
            "idempotency_key": "prop-phasee-large-family",
        }),
    )
    .await
    .expect("propose promotion under large-family load");
    process_topology_validate_promotion(
        &rooms,
        &serde_json::json!({ "proposal_id": proposed.proposal_id }),
    )
    .await
    .expect("validate proposal");
    let applied = process_topology_apply_promotion(
        &rooms,
        &server_key,
        &serde_json::json!({ "proposal_id": proposed.proposal_id }),
    )
    .await
    .expect("apply proposal");
    assert_ne!(applied.parent_new_canonical_hash, parent_hash_before);

    eprintln!(
        "phasee_large_family_baseline create_ms={} list_ms={} total_children={} cap={} retained={}",
        create_elapsed.as_millis(),
        list_elapsed.as_millis(),
        TOTAL_CHILDREN,
        INDEX_CAP,
        listed_ids.len()
    );
}

/// FSE-10.A: two validated promotions against the same parent applied concurrently.
/// Exactly one apply should succeed; the other must reject as stale-parent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_room_fse10a_multi_promotion_concurrency_single_winner() {
    let parent_id = "auth-parent-fse10a-concurrency";
    let child_a_id = "auth-child-fse10a-a";
    let child_b_id = "auth-child-fse10a-b";
    let server_key = SigningKey::from_bytes(&[0xD1u8; 32]);
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let rooms = Rooms::new_with_topology_limits(
        server_key.clone(),
        persistence,
        512,
        0,
        0,
        1,  // force fair-queue contention on promotion operations
        32, // allow bounded waiters
        Some(256),
    );

    let author = SigningKey::from_bytes(&[0xD2u8; 32]);
    let parent = rooms.get_or_create(parent_id).await;
    import_nodes(
        &parent,
        vec![make_map_set_node(&author, "world/fse10a-root", b"ready")],
    )
    .await;
    let checkpoint = snapshot_parent_checkpoint(&parent).await.unwrap();

    for child_id in [child_a_id, child_b_id] {
        rooms
            .create_child_room(
                parent_id,
                child_id,
                checkpoint.clone(),
                "fse10a-load".to_string(),
                "fse10a-runner".to_string(),
                "promotion-based".to_string(),
            )
            .await
            .expect("create child");
    }

    let child_a = rooms.get_or_create(child_a_id).await;
    import_nodes(
        &child_a,
        vec![make_map_set_node(&author, "world/fse10a/a", b"outcome-a")],
    )
    .await;
    let child_a_hash = snapshot_room_canonical_hash(&child_a).await.unwrap();

    let child_b = rooms.get_or_create(child_b_id).await;
    import_nodes(
        &child_b,
        vec![make_map_set_node(&author, "world/fse10a/b", b"outcome-b")],
    )
    .await;
    let child_b_hash = snapshot_room_canonical_hash(&child_b).await.unwrap();

    let proposed_a = process_topology_propose_promotion(
        &rooms,
        &serde_json::json!({
            "parent_room_id": parent_id,
            "child_room_id": child_a_id,
            "child_checkpoint_hash": child_a_hash,
            "payload_ref": "artifact://fse10a/a",
            "idempotency_key": "prop-fse10a-a",
        }),
    )
    .await
    .expect("propose A");
    let proposed_b = process_topology_propose_promotion(
        &rooms,
        &serde_json::json!({
            "parent_room_id": parent_id,
            "child_room_id": child_b_id,
            "child_checkpoint_hash": child_b_hash,
            "payload_ref": "artifact://fse10a/b",
            "idempotency_key": "prop-fse10a-b",
        }),
    )
    .await
    .expect("propose B");

    process_topology_validate_promotion(
        &rooms,
        &serde_json::json!({ "proposal_id": proposed_a.proposal_id }),
    )
    .await
    .expect("validate A");
    process_topology_validate_promotion(
        &rooms,
        &serde_json::json!({ "proposal_id": proposed_b.proposal_id }),
    )
    .await
    .expect("validate B");

    let apply_msg_a = serde_json::json!({ "proposal_id": proposed_a.proposal_id });
    let apply_msg_b = serde_json::json!({ "proposal_id": proposed_b.proposal_id });

    let (apply_a, apply_b) = tokio::join!(
        process_topology_apply_promotion(
            &rooms,
            &server_key,
            &apply_msg_a,
        ),
        process_topology_apply_promotion(
            &rooms,
            &server_key,
            &apply_msg_b,
        ),
    );

    let outcomes = vec![apply_a, apply_b];
    let success_count = outcomes.iter().filter(|r| r.is_ok()).count();
    let stale_parent_reject_count = outcomes
        .iter()
        .filter_map(|r| r.as_ref().err())
        .filter(|rej| rej.reason_class == PromotionReasonClass::StaleParent)
        .count();

    assert_eq!(
        success_count, 1,
        "exactly one concurrent promotion apply should succeed"
    );
    assert_eq!(
        stale_parent_reject_count, 1,
        "losing concurrent promotion apply should reject with stale-parent"
    );
}

async fn drain_until_welcome(
    stream: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(TMessage::Text(t)))) =
            tokio::time::timeout(Duration::from_millis(300), stream.next()).await
        {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                if v.get("type").and_then(|x| x.as_str()) == Some("welcome") {
                    return;
                }
            }
        }
    }
    panic!("never received welcome");
}
