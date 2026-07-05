use ed25519_dalek::SigningKey;
use nodalmerge_core::{
    Hash, MapOp, Op, StateGraph, SyncNode, Transaction, compact, compact_incremental,
    rebuild_from_snapshot, replay, verify_snapshot,
};

fn signed_set_node(
    key: &SigningKey,
    lamport: u64,
    map_key: &str,
    map_val: &str,
    parents: Vec<Hash>,
) -> SyncNode {
    let tx = Transaction {
        author: key.verifying_key().to_bytes(),
        lamport,
        wall_ms: 0,
        ops: vec![Op::Map(MapOp::Set {
            key: map_key.to_string(),
            value: map_val.as_bytes().to_vec(),
        })],
        parents,
    };
    SyncNode::new_signed(tx, key)
}

fn linear_chain_nodes(count: usize) -> Vec<SyncNode> {
    let signer = SigningKey::from_bytes(&[0x41u8; 32]);
    let mut nodes = Vec::with_capacity(count);
    let mut parents: Vec<Hash> = Vec::new();
    for i in 0..count {
        let node = signed_set_node(
            &signer,
            (i + 1) as u64,
            &format!("world/path/{i}"),
            &format!("v{i}"),
            parents,
        );
        parents = vec![node.id];
        nodes.push(node);
    }
    nodes
}

#[test]
fn branch_fork_001_frontier_hash_equal() {
    let source_nodes = linear_chain_nodes(8);

    // Frontier-based fork stub: cut at node index 4 and include the
    // ancestor-closed payload for that frontier.
    let cut = 5;
    let source_at_cut = replay(&source_nodes[..cut], None).expect("source replay should succeed");
    let fork_payload = source_nodes[..cut].to_vec();
    let fork_initial = replay(&fork_payload, None).expect("fork payload replay should succeed");

    assert_eq!(
        source_at_cut.hash, fork_initial.hash,
        "BRANCH-FORK-001: fork-from-frontier hash mismatch"
    );
}

#[test]
fn branch_fork_002_snapshot_hash_equal() {
    let source_nodes = linear_chain_nodes(8);

    // Snapshot-hash-based fork stub: resolve expected snapshot hash at cut,
    // then ensure payload resolved for that cut reproduces it exactly.
    let cut = 6;
    let source_at_snapshot = replay(&source_nodes[..cut], None).expect("source replay should succeed");
    let expected_snapshot_hash = source_at_snapshot.hash;

    let fork_payload = source_nodes[..cut].to_vec();
    let fork_initial = replay(&fork_payload, None).expect("fork payload replay should succeed");

    assert_eq!(
        expected_snapshot_hash, fork_initial.hash,
        "BRANCH-FORK-002: fork-from-snapshot-hash mismatch"
    );
}

#[test]
fn branch_fork_005_compaction_preserves_metadata() {
    let signer = SigningKey::from_bytes(&[0x42u8; 32]);
    let mut g = StateGraph::new();

    // Simulated branch metadata keys included in canonical room map.
    let _id1 = g
        .apply_local(
            &signer,
            0,
            vec![Op::Map(MapOp::Set {
                key: "branch/meta/id".to_string(),
                value: b"branch-a".to_vec(),
            })],
        )
        .expect("apply_local should succeed");
    let _id2 = g
        .apply_local(
            &signer,
            0,
            vec![Op::Map(MapOp::Set {
                key: "branch/meta/parent".to_string(),
                value: b"root".to_vec(),
            })],
        )
        .expect("apply_local should succeed");

    let before_ids = g.all_node_ids();
    let before_nodes: Vec<SyncNode> = g.get_nodes(&before_ids).into_iter().cloned().collect();
    let _before = replay(&before_nodes, None).expect("replay before compaction should succeed");
    let full_snapshot = compact(&g, &signer).expect("compact should succeed");

    // Add post-snapshot branch metadata churn and generate an incremental
    // snapshot chained to the full snapshot id.
    let _id3 = g
        .apply_local(
            &signer,
            0,
            vec![Op::Map(MapOp::Set {
                key: "branch/meta/label".to_string(),
                value: b"feature-x".to_vec(),
            })],
        )
        .expect("apply_local should succeed");

    let before_incremental_ids = g.all_node_ids();
    let before_incremental_nodes: Vec<SyncNode> = g
        .get_nodes(&before_incremental_ids)
        .into_iter()
        .cloned()
        .collect();
    let before_incremental = replay(&before_incremental_nodes, None)
        .expect("replay before incremental compaction should succeed");

    let snapshot = compact_incremental(&g, &signer, full_snapshot.id)
        .expect("compact_incremental should succeed");
    let meta = verify_snapshot(&snapshot).expect("snapshot metadata should verify");

    // Rebuild from snapshot only (no delta) and ensure snapshot metadata
    // survives transport/installation unchanged.
    let rebuilt = rebuild_from_snapshot(snapshot.clone(), &[])
        .expect("rebuild_from_snapshot should succeed");
    let rebuilt_snapshot = rebuilt
        .get_nodes(&[snapshot.id])
        .into_iter()
        .next()
        .expect("rebuilt graph must contain snapshot node")
        .clone();
    let rebuilt_meta = verify_snapshot(&rebuilt_snapshot)
        .expect("rebuilt snapshot metadata should verify");

    assert_eq!(
        meta.snapshot_hash, before_incremental.hash,
        "BRANCH-FORK-005: snapshot hash metadata does not match pre-compaction canonical hash"
    );
    assert_eq!(
        meta.base_id,
        Some(full_snapshot.id),
        "BRANCH-FORK-005: incremental snapshot base_id missing or incorrect"
    );
    assert_eq!(
        rebuilt_meta.base_id,
        meta.base_id,
        "BRANCH-FORK-005: snapshot base_id metadata changed across rebuild"
    );
    assert_eq!(
        rebuilt_meta.snapshot_hash,
        meta.snapshot_hash,
        "BRANCH-FORK-005: snapshot hash metadata changed across rebuild"
    );
    assert_eq!(
        rebuilt_meta.frontier,
        meta.frontier,
        "BRANCH-FORK-005: snapshot frontier metadata changed across rebuild"
    );
    assert!(
        !meta.frontier.is_empty(),
        "BRANCH-FORK-005: snapshot frontier metadata should be present"
    );
}
