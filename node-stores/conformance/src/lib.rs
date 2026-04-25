//! F7 — shared conformance suite for `NodePersistence` adapters.
//!
//! Every adapter must pass [`run_all`] against a freshly-isolated namespace.
//! The scenarios are derived from `server/tests/persistence.rs` and codify
//! behavior the server depends on: idempotent inserts, hydration ordering,
//! per-room isolation, batch semantics.
//!
//! ## Usage
//!
//! ```ignore
//! let store = MyAdapter::connect(...)?;
//! activesync_nodestore_conformance::run_all(&store, "test-room-1");
//! ```
//!
//! All scenarios use a single `room_id` namespace passed by the caller, so
//! tests can isolate themselves by naming (e.g. `format!("conf-{nanos}")`).

use activesync_core::{MapOp, Op, StateGraph, SyncNode};
use activesync_server::store::NodePersistence;
use ed25519_dalek::SigningKey;

fn make_node(sk: &SigningKey, key: &str, val: &[u8]) -> SyncNode {
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

/// Run every conformance scenario under the given `room_id`. Panics on
/// the first violation. Use a fresh `room_id` per call to avoid bleed
/// between concurrent test runs.
pub fn run_all(store: &dyn NodePersistence, room_id: &str) {
    nodes_durable_reports_true(store);
    empty_room_returns_empty(store, &format!("{room_id}-empty"));
    persist_then_load_round_trips(store, &format!("{room_id}-rt"));
    duplicate_persist_is_idempotent(store, &format!("{room_id}-dup"));
    batch_persist_round_trips(store, &format!("{room_id}-batch"));
    rooms_are_isolated(store, &format!("{room_id}-iso"));
    hydration_order_matches_insertion(store, &format!("{room_id}-order"));
}

fn nodes_durable_reports_true(store: &dyn NodePersistence) {
    assert!(
        store.nodes_durable(),
        "F7 adapters must report nodes_durable() == true"
    );
}

fn empty_room_returns_empty(store: &dyn NodePersistence, room: &str) {
    let loaded = store.load_room_nodes(room);
    assert!(loaded.is_empty(), "expected empty room, got {}", loaded.len());
}

fn persist_then_load_round_trips(store: &dyn NodePersistence, room: &str) {
    let sk = SigningKey::from_bytes(&[0x11u8; 32]);
    let n = make_node(&sk, "k", b"v");
    store.persist_node(room, &n);
    let loaded = store.load_room_nodes(room);
    assert_eq!(loaded.len(), 1, "expected 1 node, got {}", loaded.len());
    assert_eq!(loaded[0].id, n.id, "loaded node id mismatch");
    assert_eq!(loaded[0].signature.0, n.signature.0, "signature drift");
}

fn duplicate_persist_is_idempotent(store: &dyn NodePersistence, room: &str) {
    let sk = SigningKey::from_bytes(&[0x22u8; 32]);
    let n = make_node(&sk, "k", b"v");
    store.persist_node(room, &n);
    store.persist_node(room, &n);
    store.persist_node(room, &n);
    let loaded = store.load_room_nodes(room);
    assert_eq!(loaded.len(), 1, "duplicate persist must be idempotent");
    assert_eq!(loaded[0].id, n.id);
}

fn batch_persist_round_trips(store: &dyn NodePersistence, room: &str) {
    let sk = SigningKey::from_bytes(&[0x33u8; 32]);
    let nodes: Vec<SyncNode> = (0..5)
        .map(|i| make_node(&sk, &format!("k{i}"), &[i as u8]))
        .collect();
    let refs: Vec<&SyncNode> = nodes.iter().collect();
    store.persist_nodes(room, &refs);
    let loaded = store.load_room_nodes(room);
    assert_eq!(loaded.len(), 5, "expected 5 nodes from batch");
    let loaded_ids: std::collections::HashSet<_> = loaded.iter().map(|n| n.id).collect();
    for n in &nodes {
        assert!(loaded_ids.contains(&n.id), "batch missing node {:?}", n.id);
    }

    // Re-batching the same nodes must remain idempotent.
    store.persist_nodes(room, &refs);
    let loaded2 = store.load_room_nodes(room);
    assert_eq!(loaded2.len(), 5, "batch persist must be idempotent");
}

fn rooms_are_isolated(store: &dyn NodePersistence, room: &str) {
    let sk = SigningKey::from_bytes(&[0x44u8; 32]);
    let a = format!("{room}-a");
    let b = format!("{room}-b");
    let na = make_node(&sk, "ka", b"va");
    let nb = make_node(&sk, "kb", b"vb");
    store.persist_node(&a, &na);
    store.persist_node(&b, &nb);

    let la = store.load_room_nodes(&a);
    let lb = store.load_room_nodes(&b);
    assert_eq!(la.len(), 1);
    assert_eq!(lb.len(), 1);
    assert_eq!(la[0].id, na.id);
    assert_eq!(lb[0].id, nb.id);
    assert_ne!(la[0].id, lb[0].id, "rooms leaked into each other");
}

fn hydration_order_matches_insertion(store: &dyn NodePersistence, room: &str) {
    // Persist single nodes in a known order. The adapter must hydrate them
    // in the same insertion order so DAG parent edges resolve.
    let sk = SigningKey::from_bytes(&[0x55u8; 32]);
    let mut inserted_ids = Vec::new();
    for i in 0..8 {
        let n = make_node(&sk, &format!("ord{i}"), &[i as u8]);
        inserted_ids.push(n.id);
        store.persist_node(room, &n);
    }
    let loaded = store.load_room_nodes(room);
    let loaded_ids: Vec<_> = loaded.iter().map(|n| n.id).collect();
    assert_eq!(
        loaded_ids, inserted_ids,
        "hydration order must match insertion order"
    );
}
