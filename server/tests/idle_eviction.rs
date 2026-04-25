//! Integration test for idle-room eviction (F4 follow-up).
//!
//! Covers three properties:
//! 1. A durable room with zero connected peers past its idle timeout is
//!    swept from the registry.
//! 2. Hydration after eviction: a subsequent `get_or_create` rebuilds the
//!    room's DAG from persistence.
//! 3. In-memory rooms (NoPersistence) are never evicted, even when idle —
//!    that would be data loss.

use std::sync::Arc;
use std::time::{Duration, Instant};

use activesync_core::{MapOp, Op, StateGraph};
use activesync_server::room::{import_nodes, Rooms};
use activesync_server::store::{DirPersistence, NoPersistence, SharedPersistence};
use ed25519_dalek::SigningKey;

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("activesync-idle-{tag}-{nanos}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn make_node(sk: &SigningKey, key: &str, val: &[u8]) -> activesync_core::SyncNode {
    let mut g = StateGraph::new();
    let id = g.apply_local(sk, 0, vec![Op::Map(MapOp::Set {
        key: key.into(), value: val.to_vec(),
    })]).unwrap();
    g.get_nodes(&[id]).into_iter().next().unwrap().clone()
}

#[tokio::test]
async fn durable_idle_room_is_evicted_and_rehydrates() {
    let dir = tmpdir("evict");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let server_key = SigningKey::from_bytes(&[0x11u8; 32]);
    let rooms = Rooms::new(server_key, Arc::clone(&persistence), 512, 0, 0);

    // Create a room, connect a peer, persist a node, disconnect.
    {
        let room = rooms.get_or_create("r1").await;
        room.register_peer("peer-a".into()).await;
        let sk = SigningKey::from_bytes(&[0x22u8; 32]);
        let node = make_node(&sk, "hello", b"world");
        let (accepted, errs) = import_nodes(&room, vec![node]).await;
        assert_eq!(accepted, 1);
        assert!(errs.is_empty());
        room.deregister_peer("peer-a").await;
        // Drop our handle so Arc::strong_count drops to 1 (registry only).
    }

    // Sweep with zero timeout -> should evict immediately.
    let evicted = rooms.sweep_idle(Duration::ZERO, Instant::now()).await;
    assert_eq!(evicted, vec!["r1".to_string()]);

    // Rejoining hydrates from disk.
    let room = rooms.get_or_create("r1").await;
    let graph = room.graph.read().await;
    let state: std::collections::HashMap<String, Vec<u8>> = graph.resolve().into_iter().collect();
    assert_eq!(state.get("hello").map(|v| v.as_slice()), Some(b"world".as_slice()));
    drop(graph);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn in_memory_rooms_are_never_evicted() {
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let server_key = SigningKey::from_bytes(&[0x33u8; 32]);
    let rooms = Rooms::new(server_key, persistence, 512, 0, 0);

    {
        let room = rooms.get_or_create("r2").await;
        room.register_peer("peer-b".into()).await;
        room.deregister_peer("peer-b").await;
    }

    let evicted = rooms.sweep_idle(Duration::ZERO, Instant::now()).await;
    assert!(evicted.is_empty(), "NoPersistence rooms must not be evicted (data loss)");
}

#[tokio::test]
async fn connected_room_is_not_evicted() {
    let dir = tmpdir("connected");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let server_key = SigningKey::from_bytes(&[0x44u8; 32]);
    let rooms = Rooms::new(server_key, Arc::clone(&persistence), 512, 0, 0);

    let room = rooms.get_or_create("r3").await;
    room.register_peer("peer-c".into()).await;
    // Still connected; also we're holding an extra Arc -> strong_count > 1.

    let evicted = rooms.sweep_idle(Duration::ZERO, Instant::now()).await;
    assert!(evicted.is_empty(), "room with a live peer must not be evicted");

    room.deregister_peer("peer-c").await;
    drop(room);
    // Now only the registry holds it and peers is empty -> eligible.
    let evicted = rooms.sweep_idle(Duration::ZERO, Instant::now()).await;
    assert_eq!(evicted, vec!["r3".to_string()]);

    let _ = std::fs::remove_dir_all(&dir);
}
