//! D4: Deterministic Replay
//!
//! `replay()` is the trust anchor for the entire system.  Given the same set
//! of `SyncNode`s, every peer on every platform must arrive at byte-identical
//! `ResolvedState::hash`.  This hash is used as:
//!
//! - The `snapshot_hash` stored in D3 compaction checkpoint nodes.
//! - The server-side validation value before signing a snapshot.
//! - The debug/audit output from `activesync-server replay <file>`.
//!
//! # Determinism guarantees
//! - `BTreeMap` ensures keys are iterated in lexicographic order.
//! - The hash input uses fixed-width little-endian length prefixes so there is
//!   no ambiguity between where a key ends and a value begins.
//! - No wall-clock time, randomness, or I/O is used.
//! - The `E2EE_KEY` sentinel (`\x00e2ee`) is excluded from the hash so that
//!   encrypted and decrypted forms of the same logical state produce the same
//!   hash (callers must decrypt nodes before replaying if they want a
//!   meaningful map; the hash will still be deterministic either way).

use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};

use crate::{
    error::SyncError,
    hash::Hash,
    node::SyncNode,
    policy::Policy,
    graph::StateGraph,
    crypto::E2EE_KEY,
};

/// The fully resolved state after replaying a sequence of nodes.
///
/// `map` is the LWW-Map result (sorted by key).
/// `hash` is the Blake3 fingerprint of the canonical serialization of `map`
/// (see [`canonical_hash`]) — used for snapshot verification in D3.
#[derive(Debug, Clone)]
pub struct ResolvedState {
    pub map: BTreeMap<String, Vec<u8>>,
    pub hash: Hash,
}

/// A policy timeline transition point for deterministic replay.
///
/// The policy becomes effective for any node with `lamport >= effective_lamport`
/// until superseded by a later timeline entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyTimelineEntry {
    pub effective_lamport: u64,
    pub policy: Policy,
}

/// Replay an ordered (or unordered) sequence of `SyncNode`s and produce a
/// deterministic `ResolvedState`.
///
/// - All nodes are fed through `StateGraph::apply_remote`, so Ed25519
///   signatures and content-addressing are fully verified.
/// - Out-of-order delivery is handled by multi-pass retry: nodes whose parents
///   haven't been inserted yet are retried until no further progress is made.
/// - If `policy` is `Some`, it is installed on the graph before replay begins;
///   nodes that violate the policy are rejected (same as live operation).
/// - The E2EE sentinel key (`\x00e2ee`) is excluded from both `map` and the
///   hash so replay is meaningful for both plaintext and E2EE rooms.
///
/// # Errors
/// Returns `SyncError` if a node has an invalid signature or hash, or if
/// nodes reference parents that never arrive (broken pack).
pub fn replay(nodes: &[SyncNode], policy: Option<&Policy>) -> Result<ResolvedState, SyncError> {
    let timeline = policy
        .map(|p| {
            vec![PolicyTimelineEntry {
                effective_lamport: 0,
                policy: p.clone(),
            }]
        })
        .unwrap_or_default();

    replay_with_policy_timeline(nodes, &timeline)
}

/// Replay nodes with a lamport-versioned policy timeline.
///
/// This is the P2 timeline scaffold: callers can describe policy transitions
/// as version points and replay will evaluate each node against the policy that
/// was effective at that node's lamport.
pub fn replay_with_policy_timeline(
    nodes: &[SyncNode],
    timeline: &[PolicyTimelineEntry],
) -> Result<ResolvedState, SyncError> {
    let mut graph = StateGraph::new();
    let default_policy = Policy::default();
    let mut sorted_timeline = timeline.to_vec();
    sorted_timeline.sort_by_key(|entry| entry.effective_lamport);

    // Multi-pass topological insert: keep retrying nodes with missing parents
    // until either all are inserted or no progress is made (broken pack).
    let mut pending: Vec<SyncNode> = nodes.to_vec();
    loop {
        if pending.is_empty() {
            break;
        }
        let before = pending.len();
        let mut still_pending = Vec::new();
        for node in pending {
            graph.set_policy(policy_for_lamport(node.lamport(), &sorted_timeline, &default_policy).clone());
            match graph.apply_remote(node.clone()) {
                Ok(()) => {}
                Err(SyncError::DuplicateNode(_)) => {}
                Err(SyncError::MissingParent(_)) => still_pending.push(node),
                Err(e) => return Err(e),
            }
        }
        pending = still_pending;
        if pending.len() == before {
            // No progress — pack has genuinely missing ancestors.
            return Err(SyncError::MissingParent(
                pending[0].parents().first().copied().unwrap_or(pending[0].id),
            ));
        }
    }

    // Use resolve() — in replay context every node was inserted via
    // apply_remote, so local_node_ids is empty and speculative == canonical.
    let raw: BTreeMap<String, Vec<u8>> = graph
        .resolve()
        .into_iter()
        .filter(|(k, _)| k != E2EE_KEY)
        .collect();

    let hash = canonical_hash(&raw);
    Ok(ResolvedState { map: raw, hash })
}

/// Deterministic fingerprint of a policy timeline for snapshot compatibility.
///
/// The timeline is normalized by `effective_lamport` before hashing so callers
/// can provide entries in any order.
pub fn policy_timeline_hash(timeline: &[PolicyTimelineEntry]) -> Hash {
    let mut sorted_timeline = timeline.to_vec();
    sorted_timeline.sort_by_key(|entry| entry.effective_lamport);
    let bytes = postcard::to_allocvec(&sorted_timeline).expect("timeline is always serializable");
    Hash::of(&bytes)
}

/// Highest policy cutover lamport in a timeline.
pub fn policy_timeline_cutover_lamport(timeline: &[PolicyTimelineEntry]) -> Option<u64> {
    timeline.iter().map(|entry| entry.effective_lamport).max()
}

fn policy_for_lamport<'a>(
    lamport: u64,
    timeline: &'a [PolicyTimelineEntry],
    default_policy: &'a Policy,
) -> &'a Policy {
    let mut selected = default_policy;
    for entry in timeline {
        if entry.effective_lamport <= lamport {
            selected = &entry.policy;
        } else {
            break;
        }
    }

    selected
}

/// Compute a deterministic Blake3 hash over a resolved map.
///
/// Serialization format (no padding, no delimiters between entries):
/// ```text
/// for each (key, value) in BTreeMap order (lexicographic):
///     u32 LE  — byte length of key
///     [u8]    — key as UTF-8
///     u32 LE  — byte length of value
///     [u8]    — value bytes
/// ```
///
/// This format is unambiguous (length-prefixed), platform-independent, and
/// produces identical bytes on every architecture.
pub fn canonical_hash(map: &BTreeMap<String, Vec<u8>>) -> Hash {
    let mut hasher = blake3::Hasher::new();
    for (key, value) in map {
        let klen = key.len() as u32;
        let vlen = value.len() as u32;
        hasher.update(&klen.to_le_bytes());
        hasher.update(key.as_bytes());
        hasher.update(&vlen.to_le_bytes());
        hasher.update(value);
    }
    Hash(*hasher.finalize().as_bytes())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use crate::op::{Op, MapOp, Transaction};
    use crate::node::SyncNode;

    fn key_a() -> SigningKey { SigningKey::from_bytes(&[0x0Au8; 32]) }
    fn key_b() -> SigningKey { SigningKey::from_bytes(&[0x0Bu8; 32]) }

    fn signed_node(key: &SigningKey, lamport: u64, ops: Vec<Op>, parents: Vec<Hash>) -> SyncNode {
        let tx = Transaction {
            author: key.verifying_key().to_bytes(),
            lamport,
            wall_ms: 0,
            ops,
            parents,
        };
        SyncNode::new_signed(tx, key)
    }

    fn set(key: &str, val: &str) -> Op {
        Op::Map(MapOp::Set { key: key.into(), value: val.as_bytes().to_vec() })
    }

    // -------------------------------------------------------------------------
    // Basic correctness
    // -------------------------------------------------------------------------

    #[test]
    fn replay_empty_gives_empty_map_and_stable_hash() {
        let s = replay(&[], None).unwrap();
        assert!(s.map.is_empty());
        // Empty map → deterministic hash of an empty byte stream.
        let h2 = canonical_hash(&BTreeMap::new());
        assert_eq!(s.hash, h2);
    }

    #[test]
    fn replay_single_node_produces_expected_state() {
        let ka = key_a();
        let node = signed_node(&ka, 1, vec![set("x", "hello")], vec![]);
        let s = replay(&[node], None).unwrap();
        assert_eq!(s.map.get("x").map(|v| v.as_slice()), Some(b"hello".as_slice()));
    }

    #[test]
    fn replay_lww_higher_lamport_wins() {
        let ka = key_a();
        let n1 = signed_node(&ka, 1, vec![set("score", "10")], vec![]);
        let n2 = signed_node(&ka, 2, vec![set("score", "20")], vec![n1.id]);
        let s = replay(&[n1, n2], None).unwrap();
        assert_eq!(s.map.get("score").map(|v| v.as_slice()), Some(b"20".as_slice()));
    }

    // -------------------------------------------------------------------------
    // Determinism: shuffle order → same hash
    // -------------------------------------------------------------------------

    /// Build a small DAG: genesis node, then two parallel children from
    /// different authors, then a merge node.  Returns nodes in causal order.
    fn build_dag() -> Vec<SyncNode> {
        let ka = key_a();
        let kb = key_b();

        let root = signed_node(&ka, 1, vec![set("a", "1")], vec![]);
        let left = signed_node(&ka, 2, vec![set("b", "L")], vec![root.id]);
        let right = signed_node(&kb, 2, vec![set("c", "R")], vec![root.id]);
        let merge = signed_node(&ka, 3, vec![set("d", "M")], vec![left.id, right.id]);
        vec![root, left, right, merge]
    }

    #[test]
    fn replay_is_deterministic_regardless_of_node_order() {
        let nodes = build_dag();
        let base = replay(&nodes, None).unwrap();

        // Try all valid topological permutations:
        // root must come first; left and right can swap; merge must be last.
        // For our 4-node DAG the valid orderings are:
        //   [root, left, right, merge] and [root, right, left, merge]
        let alt_order = vec![
            nodes[0].clone(), // root
            nodes[2].clone(), // right
            nodes[1].clone(), // left
            nodes[3].clone(), // merge
        ];
        let alt = replay(&alt_order, None).unwrap();

        assert_eq!(base.hash, alt.hash);
        assert_eq!(base.map, alt.map);
    }

    #[test]
    fn replay_is_deterministic_with_out_of_order_delivery() {
        // Feed nodes in reverse topological order; multi-pass should recover.
        let nodes = build_dag();
        let base = replay(&nodes, None).unwrap();

        let mut reversed = nodes.clone();
        reversed.reverse();
        let alt = replay(&reversed, None).unwrap();

        assert_eq!(base.hash, alt.hash);
    }

    // -------------------------------------------------------------------------
    // canonical_hash stability
    // -------------------------------------------------------------------------

    #[test]
    fn canonical_hash_is_order_independent_via_btreemap() {
        // BTreeMap iterates in sorted key order, so insertion order is irrelevant.
        let mut m1 = BTreeMap::new();
        m1.insert("z".to_string(), b"last".to_vec());
        m1.insert("a".to_string(), b"first".to_vec());

        let mut m2 = BTreeMap::new();
        m2.insert("a".to_string(), b"first".to_vec());
        m2.insert("z".to_string(), b"last".to_vec());

        assert_eq!(canonical_hash(&m1), canonical_hash(&m2));
    }

    #[test]
    fn canonical_hash_different_for_different_values() {
        let mut m1 = BTreeMap::new();
        m1.insert("k".to_string(), b"v1".to_vec());
        let mut m2 = BTreeMap::new();
        m2.insert("k".to_string(), b"v2".to_vec());
        assert_ne!(canonical_hash(&m1), canonical_hash(&m2));
    }

    // -------------------------------------------------------------------------
    // Policy enforcement during replay
    // -------------------------------------------------------------------------

    #[test]
    fn replay_with_policy_rejects_unauthorized_nodes() {
        use crate::policy::{Policy, PolicyDefault, PolicyRule};

        let ka = key_a();
        let kb = key_b();
        let authorized = ka.verifying_key().to_bytes();

        let policy = Policy {
            rules: vec![PolicyRule {
                path_glob: "protected/**".into(),
                can_write: vec![authorized],
                can_read: vec![],
                can_derive: vec![],
            }],
            default: PolicyDefault::DenyAll,
        };

        let good_node = signed_node(&ka, 1, vec![set("protected/x", "ok")], vec![]);
        let bad_node  = signed_node(&kb, 1, vec![set("protected/x", "nope")], vec![]);

        // Good node passes, bad node is rejected.
        assert!(replay(&[good_node], Some(&policy)).is_ok());
        assert!(matches!(
            replay(&[bad_node], Some(&policy)),
            Err(SyncError::PolicyViolation { .. })
        ));
    }

    #[test]
    fn replay_timeline_applies_new_policy_at_cutover_lamport() {
        use crate::policy::{Policy, PolicyDefault, PolicyRule};

        let ka = key_a();
        let kb = key_b();
        let author_a = ka.verifying_key().to_bytes();

        let tightened = Policy {
            rules: vec![PolicyRule {
                path_glob: "protected/**".into(),
                can_write: vec![author_a],
                can_read: vec![],
                can_derive: vec![],
            }],
            default: PolicyDefault::DenyAll,
        };

        let before_cutover = signed_node(&kb, 1, vec![set("protected/x", "before")], vec![]);
        let after_cutover = signed_node(
            &kb,
            2,
            vec![set("protected/x", "after")],
            vec![before_cutover.id],
        );

        let timeline = vec![PolicyTimelineEntry {
            effective_lamport: 2,
            policy: tightened,
        }];

        assert!(matches!(
            replay_with_policy_timeline(&[before_cutover, after_cutover], &timeline),
            Err(SyncError::PolicyViolation { .. })
        ));
    }

    #[test]
    fn replay_timeline_allows_authorized_writer_after_cutover() {
        use crate::policy::{Policy, PolicyDefault, PolicyRule};

        let ka = key_a();
        let author_a = ka.verifying_key().to_bytes();

        let tightened = Policy {
            rules: vec![PolicyRule {
                path_glob: "protected/**".into(),
                can_write: vec![author_a],
                can_read: vec![],
                can_derive: vec![],
            }],
            default: PolicyDefault::DenyAll,
        };

        let n1 = signed_node(&ka, 1, vec![set("protected/x", "before")], vec![]);
        let n2 = signed_node(&ka, 2, vec![set("protected/x", "after")], vec![n1.id]);

        let timeline = vec![PolicyTimelineEntry {
            effective_lamport: 2,
            policy: tightened,
        }];

        let resolved = replay_with_policy_timeline(&[n1, n2], &timeline).unwrap();
        assert_eq!(resolved.map.get("protected/x").map(|v| v.as_slice()), Some(b"after".as_slice()));
    }

    #[test]
    fn policy_timeline_hash_is_stable_for_equal_timelines() {
        use crate::policy::{Policy, PolicyDefault, PolicyRule};

        let ka = key_a();
        let kb = key_b();

        let p1 = Policy {
            rules: vec![PolicyRule {
                path_glob: "protected/**".into(),
                can_write: vec![ka.verifying_key().to_bytes()],
                can_read: vec![],
                can_derive: vec![],
            }],
            default: PolicyDefault::DenyAll,
        };

        let p2 = Policy {
            rules: vec![PolicyRule {
                path_glob: "protected/**".into(),
                can_write: vec![kb.verifying_key().to_bytes()],
                can_read: vec![],
                can_derive: vec![],
            }],
            default: PolicyDefault::DenyAll,
        };

        let t1 = vec![
            PolicyTimelineEntry { effective_lamport: 10, policy: p2.clone() },
            PolicyTimelineEntry { effective_lamport: 5, policy: p1.clone() },
        ];
        let t2 = vec![
            PolicyTimelineEntry { effective_lamport: 5, policy: p1 },
            PolicyTimelineEntry { effective_lamport: 10, policy: p2 },
        ];

        assert_eq!(policy_timeline_hash(&t1), policy_timeline_hash(&t2));
        assert_eq!(policy_timeline_cutover_lamport(&t1), Some(10));
    }

    // -------------------------------------------------------------------------
    // E2EE sentinel filtering
    // -------------------------------------------------------------------------

    #[test]
    fn replay_filters_e2ee_sentinel_key_from_map_and_hash() {
        // Simulate a node carrying the E2EE sentinel op (as if it were plaintext).
        let ka = key_a();
        let e2ee_op = Op::Map(MapOp::Set {
            key: E2EE_KEY.to_string(),
            value: b"ciphertext".to_vec(),
        });
        let node = signed_node(&ka, 1, vec![e2ee_op, set("real_key", "val")], vec![]);
        let s = replay(&[node], None).unwrap();

        // Sentinel must be absent from the resolved map.
        assert!(!s.map.contains_key(E2EE_KEY));
        // Real key is present.
        assert!(s.map.contains_key("real_key"));

        // Hash must equal a map containing only real_key.
        let mut expected_map = BTreeMap::new();
        expected_map.insert("real_key".to_string(), b"val".to_vec());
        assert_eq!(s.hash, canonical_hash(&expected_map));
    }

    // -------------------------------------------------------------------------
    // Snapshot hash round-trip (D3 preview)
    // -------------------------------------------------------------------------

    #[test]
    fn snapshot_hash_round_trip() {
        // Simulate: build a live graph, take a snapshot hash, then verify
        // that replay of the same nodes produces the same hash.
        let ka = key_a();
        let nodes = vec![
            signed_node(&ka, 1, vec![set("pos", "0,0")], vec![]),
            signed_node(&ka, 2, vec![set("pos", "1,0")], vec![]),
            signed_node(&ka, 3, vec![set("vel", "0.5")], vec![]),
        ];
        let s1 = replay(&nodes, None).unwrap();

        // Re-replay the same nodes (as if joining after compaction + delta).
        let s2 = replay(&nodes, None).unwrap();

        assert_eq!(s1.hash, s2.hash);
        assert_eq!(s1.map, s2.map);
    }
}
