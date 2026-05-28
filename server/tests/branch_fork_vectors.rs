use std::collections::BTreeMap;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use nodalmerge_core::{canonical_hash, Hash, MapOp, Op, StateGraph};
use nodalmerge_server::room::{import_nodes, Room};
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

fn canonical_hash_for_room(state: &std::collections::HashMap<String, Vec<u8>>) -> Hash {
    let canonical: BTreeMap<String, Vec<u8>> =
        state.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    canonical_hash(&canonical)
}

#[tokio::test]
async fn branch_fork_003_target_writes_isolated() {
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let source = Room::new(
        "branch-fork-003-source".to_string(),
        Arc::clone(&persistence),
        512,
    );
    let target = Room::new(
        "branch-fork-003-target".to_string(),
        Arc::clone(&persistence),
        512,
    );
    let signer = SigningKey::from_bytes(&[0x71u8; 32]);

    let fork_payload = build_set_nodes(
        &signer,
        &[
            ("world/fork/base", "v0"),
            ("world/fork/base", "v1"),
            ("world/fork/base", "v2"),
        ],
    );

    let (source_accept, _, source_errs) = import_nodes(&source, fork_payload.clone()).await;
    let (target_accept, _, target_errs) = import_nodes(&target, fork_payload).await;
    assert_eq!(source_accept, 3);
    assert_eq!(target_accept, 3);
    assert!(source_errs.is_empty());
    assert!(target_errs.is_empty());

    let source_hash_before = canonical_hash_for_room(&source.graph.read().await.resolve());

    let target_only = build_set_nodes(&signer, &[("world/fork/target-only", "t1")]);
    let (accepted_target_only, _, target_only_errs) = import_nodes(&target, target_only).await;
    assert_eq!(accepted_target_only, 1);
    assert!(target_only_errs.is_empty());

    let source_hash_after = canonical_hash_for_room(&source.graph.read().await.resolve());
    let target_hash_after = canonical_hash_for_room(&target.graph.read().await.resolve());

    assert_eq!(
        source_hash_before, source_hash_after,
        "BRANCH-FORK-003: target writes must not alter source state hash"
    );
    assert_ne!(
        source_hash_after, target_hash_after,
        "BRANCH-FORK-003: target lineage should diverge after target-only writes"
    );
}

#[tokio::test]
async fn branch_fork_004_source_writes_isolated() {
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let source = Room::new(
        "branch-fork-004-source".to_string(),
        Arc::clone(&persistence),
        512,
    );
    let target = Room::new(
        "branch-fork-004-target".to_string(),
        Arc::clone(&persistence),
        512,
    );
    let signer = SigningKey::from_bytes(&[0x72u8; 32]);

    let fork_payload = build_set_nodes(
        &signer,
        &[
            ("world/fork/base", "v0"),
            ("world/fork/base", "v1"),
            ("world/fork/base", "v2"),
        ],
    );

    let (source_accept, _, source_errs) = import_nodes(&source, fork_payload.clone()).await;
    let (target_accept, _, target_errs) = import_nodes(&target, fork_payload).await;
    assert_eq!(source_accept, 3);
    assert_eq!(target_accept, 3);
    assert!(source_errs.is_empty());
    assert!(target_errs.is_empty());

    let target_hash_before = canonical_hash_for_room(&target.graph.read().await.resolve());

    let source_only = build_set_nodes(&signer, &[("world/fork/source-only", "s1")]);
    let (accepted_source_only, _, source_only_errs) = import_nodes(&source, source_only).await;
    assert_eq!(accepted_source_only, 1);
    assert!(source_only_errs.is_empty());

    let source_hash_after = canonical_hash_for_room(&source.graph.read().await.resolve());
    let target_hash_after = canonical_hash_for_room(&target.graph.read().await.resolve());

    assert_eq!(
        target_hash_before, target_hash_after,
        "BRANCH-FORK-004: source writes must not alter target lineage"
    );
    assert_ne!(
        source_hash_after, target_hash_after,
        "BRANCH-FORK-004: source lineage should diverge after source-only writes"
    );
}
