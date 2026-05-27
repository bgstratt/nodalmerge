use std::collections::BTreeMap;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use nodalmerge_core::{Hash, MapOp, Op, StateGraph, canonical_hash};
use nodalmerge_server::room::{Room, import_nodes};
use nodalmerge_server::store::{NoPersistence, SharedPersistence};

fn build_set_nodes(sk: &SigningKey, writes: &[(&str, &str)]) -> Vec<nodalmerge_core::SyncNode> {
    let mut g = StateGraph::new();
    let mut out = Vec::with_capacity(writes.len());
    for (k, v) in writes {
        let id = g
            .apply_local(
                sk,
                0,
                vec![Op::Map(MapOp::Set {
                    key: (*k).to_string(),
                    value: v.as_bytes().to_vec(),
                })],
            )
            .expect("apply_local should succeed");
        let n = g
            .get_nodes(&[id])
            .into_iter()
            .next()
            .expect("node must exist")
            .clone();
        out.push(n);
    }
    out
}

fn canonical_lane_hash(state: &std::collections::HashMap<String, Vec<u8>>) -> Hash {
    let filtered: BTreeMap<String, Vec<u8>> = state
        .iter()
        .filter(|(k, _)| !k.starts_with("intent/"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    canonical_hash(&filtered)
}

#[tokio::test]
async fn spec_auth_001_optimistic_visible_before_authority() {
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let room = Room::new("spec-auth-001".to_string(), Arc::clone(&persistence), 512);
    let signer = SigningKey::from_bytes(&[0x60u8; 32]);

    // Wave 0 stub: an intent write should be visible in speculative read
    // surfaces before any canonical authority write is emitted.
    let intent_nodes = build_set_nodes(&signer, &[("intent/p1/move", "{\"dx\":1,\"dy\":0}")]);
    let (accepted, _, errs) = import_nodes(&room, intent_nodes).await;
    assert_eq!(accepted, 1);
    assert!(errs.is_empty());

    let state = room.graph.read().await.resolve();
    assert_eq!(
        state.get("intent/p1/move").map(|v| v.as_slice()),
        Some(b"{\"dx\":1,\"dy\":0}".as_slice()),
        "SPEC-AUTH-001: optimistic intent should be visible before authority response"
    );
    assert!(
        !state.contains_key("world/p1/pos"),
        "SPEC-AUTH-001: canonical key should not appear before authority write"
    );
}

#[tokio::test]
async fn spec_auth_002_accepted_converges() {
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let room_a = Room::new("spec-auth-002-a".to_string(), Arc::clone(&persistence), 512);
    let room_b = Room::new("spec-auth-002-b".to_string(), Arc::clone(&persistence), 512);
    let signer = SigningKey::from_bytes(&[0x61u8; 32]);

    let nodes = build_set_nodes(
        &signer,
        &[
            ("intent/p1/move", "{\"dx\":1,\"dy\":0}"),
            ("world/p1/pos", "{\"x\":10,\"y\":20}"),
        ],
    );

    let (accepted_a, _, errs_a) = import_nodes(&room_a, nodes.clone()).await;
    let (accepted_b, _, errs_b) = import_nodes(&room_b, nodes).await;

    assert_eq!(accepted_a, 2);
    assert_eq!(accepted_b, 2);
    assert!(errs_a.is_empty());
    assert!(errs_b.is_empty());

    let state_a = room_a.graph.read().await.resolve();
    let state_b = room_b.graph.read().await.resolve();

    assert_eq!(
        state_a.get("world/p1/pos").map(|v| v.as_slice()),
        Some(b"{\"x\":10,\"y\":20}".as_slice())
    );
    assert_eq!(
        state_b.get("world/p1/pos").map(|v| v.as_slice()),
        Some(b"{\"x\":10,\"y\":20}".as_slice())
    );
    assert_eq!(
        canonical_lane_hash(&state_a),
        canonical_lane_hash(&state_b),
        "SPEC-AUTH-002: canonical-lane state did not converge across peers"
    );
}

#[tokio::test]
async fn spec_auth_005_disposition_idempotent_reconnect_safe() {
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let room = Room::new("spec-auth-005".to_string(), Arc::clone(&persistence), 512);
    let signer = SigningKey::from_bytes(&[0x62u8; 32]);

    // Build a parent-child pair for the same intent status key.
    let chain = build_set_nodes(
        &signer,
        &[
            ("intent_status/intent-1", "pending"),
            ("intent_status/intent-1", "accepted"),
        ],
    );
    let pending = chain[0].clone();
    let accepted = chain[1].clone();

    // Reconnect/replay order safety: child arrives before parent.
    let (accepted_count, _, errs) = import_nodes(&room, vec![accepted.clone(), pending]).await;
    assert_eq!(accepted_count, 2, "SPEC-AUTH-005: out-of-order replay should converge");
    assert!(errs.is_empty(), "SPEC-AUTH-005: out-of-order replay should not leave unresolved parents");

    // Idempotence: replay duplicate accepted node after convergence.
    let (dup_accepted, _, dup_errs) = import_nodes(&room, vec![accepted]).await;
    assert_eq!(dup_accepted, 0, "SPEC-AUTH-005: duplicate replay should be idempotent");
    assert!(dup_errs.is_empty(), "SPEC-AUTH-005: duplicate replay should not produce errors");

    let state = room.graph.read().await.resolve();
    assert_eq!(
        state.get("intent_status/intent-1").map(|v| v.as_slice()),
        Some(b"accepted".as_slice()),
        "SPEC-AUTH-005: final intent disposition should remain accepted"
    );
}
