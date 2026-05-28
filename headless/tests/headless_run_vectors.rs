//! HEADLESS-RUN-* conformance vectors for the headless worker slice.

use std::sync::Arc;
use std::time::Duration;

use axum::{routing::get, Router};
use ed25519_dalek::SigningKey;
use nodalmerge_core::{MapOp, Op, StateGraph};
use nodalmerge_headless::{run_worker_session, WorkerConfig};
use nodalmerge_runtime_local::PersistBackendKind;
use nodalmerge_server::room::{import_nodes, Rooms};
use nodalmerge_server::store::{NoPersistence, SharedPersistence};
use nodalmerge_server::ws_handler;

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

async fn spawn_server() -> (std::net::SocketAddr, Rooms) {
    let server_key = SigningKey::from_bytes(&[0x81u8; 32]);
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

/// HEADLESS-RUN-001: headless worker completes WS handshake and persists catch-up packs locally.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_run_001_worker_handshake_and_local_persist() {
    let room_id = "headless-run-001";
    let (addr, rooms) = spawn_server().await;

    let author = SigningKey::from_bytes(&[0x82u8; 32]);
    let node = make_map_set_node(&author, "world/greeting", b"hello-headless");
    let room = rooms.get_or_create(room_id).await;
    let (accepted, _, errs) = import_nodes(&room, vec![node]).await;
    assert_eq!(accepted, 1);
    assert!(errs.is_empty());

    let url = format!("ws://{addr}/ws/{room_id}");
    let report = run_worker_session(WorkerConfig {
        server_ws_url: url,
        room_id: room_id.to_string(),
        backend: PersistBackendKind::Memory,
        run_for: Duration::from_secs(5),
        negotiate_ibf: true,
        negotiate_mst: true,
    })
    .await
    .expect("worker session");

    assert!(report.saw_welcome, "HEADLESS-RUN-001: must receive welcome");
    assert!(
        report.nodes_persisted_total >= 1,
        "HEADLESS-RUN-001: expected at least one node in peer-local log"
    );
    assert!(!report.canonical_hash_hex.is_empty());
}

/// HEADLESS-RUN-002 (partial): file backend survives reopen with same canonical hash.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_run_002_file_backend_reopen_preserves_canonical_hash() {
    let room_id = "headless-run-002";
    let (addr, rooms) = spawn_server().await;

    let author = SigningKey::from_bytes(&[0x83u8; 32]);
    let node = make_map_set_node(&author, "world/persist", b"fs-headless");
    let room = rooms.get_or_create(room_id).await;
    import_nodes(&room, vec![node]).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let data_dir = dir.path().to_path_buf();
    let url = format!("ws://{addr}/ws/{room_id}");

    let report = run_worker_session(WorkerConfig {
        server_ws_url: url.clone(),
        room_id: room_id.to_string(),
        backend: PersistBackendKind::File {
            data_dir: data_dir.clone(),
        },
        run_for: Duration::from_secs(5),
        negotiate_ibf: true,
        negotiate_mst: true,
    })
    .await
    .expect("first session");

    assert!(report.saw_welcome);
    let hash_first = report.canonical_hash_hex.clone();

    let report2 = run_worker_session(WorkerConfig {
        server_ws_url: url,
        room_id: room_id.to_string(),
        backend: PersistBackendKind::File { data_dir },
        run_for: Duration::from_secs(2),
        negotiate_ibf: true,
        negotiate_mst: true,
    })
    .await
    .expect("second session");

    assert_eq!(
        report2.canonical_hash_hex, hash_first,
        "HEADLESS-RUN-002: canonical hash must match across worker restart with same data_dir"
    );
}

/// HEADLESS-RUN-003: durable file backend restarts, then catches up new server nodes (IBF/MST/catchup).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_run_003_file_restart_catches_up_server_append() {
    let room_id = "headless-run-003";
    let (addr, rooms) = spawn_server().await;

    let author = SigningKey::from_bytes(&[0x84u8; 32]);
    let node_a = make_map_set_node(&author, "world/a", b"first");
    let room = rooms.get_or_create(room_id).await;
    import_nodes(&room, vec![node_a]).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let data_dir = dir.path().to_path_buf();
    let url = format!("ws://{addr}/ws/{room_id}");

    let report1 = run_worker_session(WorkerConfig {
        server_ws_url: url.clone(),
        room_id: room_id.to_string(),
        backend: PersistBackendKind::File {
            data_dir: data_dir.clone(),
        },
        run_for: Duration::from_secs(5),
        negotiate_ibf: true,
        negotiate_mst: true,
    })
    .await
    .expect("first session");
    assert!(report1.saw_welcome);
    assert!(report1.nodes_persisted_total >= 1);

    let node_b = make_map_set_node(&author, "world/b", b"second");
    import_nodes(&room, vec![node_b]).await;

    let report2 = run_worker_session(WorkerConfig {
        server_ws_url: url,
        room_id: room_id.to_string(),
        backend: PersistBackendKind::File { data_dir },
        run_for: Duration::from_secs(5),
        negotiate_ibf: true,
        negotiate_mst: true,
    })
    .await
    .expect("restart session");

    assert!(report2.saw_welcome);
    assert!(
        report2.nodes_persisted_total >= 2,
        "HEADLESS-RUN-003: restart must persist server-appended node"
    );
    assert_ne!(
        report2.canonical_hash_hex, report1.canonical_hash_hex,
        "HEADLESS-RUN-003: canonical hash must change after new server op"
    );
}

/// HEADLESS-RUN-004: composite peer-local backend (write-through file) over WS.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_run_004_composite_backend_sync() {
    let room_id = "headless-run-004";
    let (addr, rooms) = spawn_server().await;

    let author = SigningKey::from_bytes(&[0x85u8; 32]);
    let node = make_map_set_node(&author, "world/composite", b"composite-backend");
    let room = rooms.get_or_create(room_id).await;
    import_nodes(&room, vec![node]).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("ws://{addr}/ws/{room_id}");

    let report = run_worker_session(WorkerConfig {
        server_ws_url: url,
        room_id: room_id.to_string(),
        backend: PersistBackendKind::Composite {
            data_dir: dir.path().to_path_buf(),
        },
        run_for: Duration::from_secs(5),
        negotiate_ibf: true,
        negotiate_mst: true,
    })
    .await
    .expect("composite session");

    assert!(report.saw_welcome);
    assert!(report.nodes_persisted_total >= 1);
}
