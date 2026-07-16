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
//! nodalmerge_nodestore_conformance::run_all(&store, "test-room-1");
//! ```
//!
//! All scenarios use a single `room_id` namespace passed by the caller, so
//! tests can isolate themselves by naming (e.g. `format!("conf-{nanos}")`).

use nodalmerge_core::{MapOp, Op, StateGraph, SyncNode};
use nodalmerge_server::store::NodePersistence;
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
    known_room_ids_is_superset_of_written_rooms(store, &format!("{room_id}-enum"));
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

/// blob-cas-remediation.md CI follow-up (raised by slice 1.1, finding #1) —
/// `known_room_ids()` is now safety-critical: the global blob GC sweep
/// trusts it completely whenever `can_enumerate_rooms() == true`, which
/// `PostgresNodeStore` and `MongoNodeStore` both assert. Every prior F7
/// scenario in this file writes into one namespaced room and never looks at
/// `known_room_ids()`/`can_enumerate_rooms()` at all, so a regression to
/// either query (e.g. a `DISTINCT room_id` scan silently returning an empty
/// or partial set) would ship with a fully green conformance suite and
/// resurrect finding #1's permanent blob deletion — now with the fail-closed
/// guard vouching for it.
///
/// `known_room_ids()` is store-global, not scoped to a single `room_id` — a
/// persistent DB or a run sharing the store with other scenarios will have
/// other rooms present. So this asserts a **superset**, never an exact set:
/// write into two distinct namespaced rooms and confirm both come back.
fn known_room_ids_is_superset_of_written_rooms(store: &dyn NodePersistence, room: &str) {
    assert!(
        store.can_enumerate_rooms(),
        "F7 adapters must report can_enumerate_rooms() == true"
    );

    let sk = SigningKey::from_bytes(&[0x66u8; 32]);
    let room_x = format!("{room}-x");
    let room_y = format!("{room}-y");
    let nx = make_node(&sk, "kx", b"vx");
    let ny = make_node(&sk, "ky", b"vy");
    store.persist_node(&room_x, &nx);
    store.persist_node(&room_y, &ny);

    // Round-trip both rooms first so a failure here (rather than the
    // enumeration assertions below) points straight at ordinary
    // persist/load, not at known_room_ids().
    assert_eq!(store.load_room_nodes(&room_x).len(), 1, "room {room_x} round-trip failed");
    assert_eq!(store.load_room_nodes(&room_y).len(), 1, "room {room_y} round-trip failed");

    let known: std::collections::HashSet<String> = store.known_room_ids().into_iter().collect();
    assert!(
        known.contains(&room_x),
        "known_room_ids() is missing room {room_x}, which was just persisted to — \
         enumeration is incomplete and the global blob GC sweep would treat this \
         room's blobs as unreferenced"
    );
    assert!(
        known.contains(&room_y),
        "known_room_ids() is missing room {room_y}, which was just persisted to — \
         enumeration is incomplete and the global blob GC sweep would treat this \
         room's blobs as unreferenced"
    );
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
