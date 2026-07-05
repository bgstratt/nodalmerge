//! G9 — conflict surfacing.
//!
//! Pure detection over the current node set. The CRDT layer always
//! converges silently; this module re-derives "who lost the LWW race
//! against whom" so the SDK can show a "your edit was overridden"
//! affordance.
//!
//! The output is a *stable function of the graph state*: same nodes →
//! same conflict events. The bridge layer is responsible for tracking
//! which events have already been delivered to the SDK so callers see
//! each conflict at most once.
//!
//! Definition. We report a conflict whenever, for a given key (Map
//! key, or `(list_key, item_id)` for List), an op authored by peer A
//! lost the LWW race to a winning op from a *different* author B.
//! Same-author overwrites are intentional refinement, not conflict.
//!
//! This is a pragmatic approximation — true causal concurrency would
//! require parent traversal — but in practice "different author lost
//! → user surprise" is exactly the surface apps want.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::node::SyncNode;
use crate::op::{ItemId, ListOp, MapOp, Op};

/// Kind discriminator for a [`ConflictEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// A `Set`/`Delete`/`SetBlob` lost LWW for the same Map key.
    MapOverwrite,
    /// A `Move` lost LWW for the same `(list_key, item_id)`.
    ListMoveLost,
    /// A `Move` (or losing `Insert`) was absorbed by a winning `Delete`.
    ListDeleteWon,
}

/// Compact description of the op that participated in a conflict.
///
/// Mirrors the small subset of [`crate::op::Op`] fields the SDK needs
/// to render a "you set k=a, B set k=b" affordance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConflictOp {
    Set { value: Vec<u8> },
    Delete,
    SetBlob { blob_hash: [u8; 32] },
    ListMove { item_id: [u8; 16] },
    ListInsert { item_id: [u8; 16] },
    ListDelete { item_id: [u8; 16] },
}

/// One observed conflict.
///
/// The pair `(winner_lamport, winner_author, loser_lamport, loser_author,
/// key, kind)` is unique within a single graph state — bridge dedup
/// uses that as a fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictEvent {
    pub kind: ConflictKind,
    pub key: String,
    pub winner_author: [u8; 32],
    pub winner_lamport: u64,
    pub winner_op: ConflictOp,
    pub loser_author: [u8; 32],
    pub loser_lamport: u64,
    pub loser_op: ConflictOp,
}

/// Stable fingerprint for dedup.
pub type ConflictFingerprint = (
    ConflictKind,
    String,
    u64,
    [u8; 32],
    u64,
    [u8; 32],
);

impl ConflictEvent {
    pub fn fingerprint(&self) -> ConflictFingerprint {
        (
            self.kind,
            self.key.clone(),
            self.winner_lamport,
            self.winner_author,
            self.loser_lamport,
            self.loser_author,
        )
    }
}

/// Detect all current conflicts in the graph defined by `nodes`.
///
/// Output ordering is deterministic: events are sorted by
/// `(winner_lamport, winner_author, loser_lamport, loser_author)` so two
/// callers seeing the same node set produce identical sequences.
pub fn detect_conflicts(nodes: &[&SyncNode]) -> Vec<ConflictEvent> {
    let mut out = Vec::new();
    detect_map_conflicts(nodes, &mut out);
    detect_list_conflicts(nodes, &mut out);
    out.sort_by(|a, b| {
        (
            a.winner_lamport,
            a.winner_author,
            a.loser_lamport,
            a.loser_author,
        )
            .cmp(&(
                b.winner_lamport,
                b.winner_author,
                b.loser_lamport,
                b.loser_author,
            ))
    });
    out
}

// -----------------------------------------------------------------------------
// Map
// -----------------------------------------------------------------------------

#[derive(Clone)]
struct MapEntry {
    lamport: u64,
    author: [u8; 32],
    op: ConflictOp,
}

fn map_op_to_conflict(op: &MapOp) -> Option<ConflictOp> {
    Some(match op {
        MapOp::Set { value, .. } => ConflictOp::Set {
            value: value.clone(),
        },
        MapOp::Delete { .. } => ConflictOp::Delete,
        MapOp::SetBlob { blob_hash, .. } => ConflictOp::SetBlob {
            blob_hash: *blob_hash.as_bytes(),
        },
    })
}

fn detect_map_conflicts(nodes: &[&SyncNode], out: &mut Vec<ConflictEvent>) {
    // Pass 1: find the winner per key.
    let mut winners: HashMap<String, MapEntry> = HashMap::new();
    for node in nodes {
        let tx = &node.transaction;
        for op in &tx.ops {
            let Op::Map(mop) = op else { continue };
            let Some(cop) = map_op_to_conflict(mop) else { continue };
            let key = match mop {
                MapOp::Set { key, .. }
                | MapOp::Delete { key }
                | MapOp::SetBlob { key, .. } => key.clone(),
            };
            let entry = winners.entry(key).or_insert(MapEntry {
                lamport: 0,
                author: [0u8; 32],
                op: ConflictOp::Delete,
            });
            if (tx.lamport, tx.author) > (entry.lamport, entry.author) {
                entry.lamport = tx.lamport;
                entry.author = tx.author;
                entry.op = cop;
            }
        }
    }

    // Pass 2: every losing op from a different author becomes a conflict.
    for node in nodes {
        let tx = &node.transaction;
        for op in &tx.ops {
            let Op::Map(mop) = op else { continue };
            let Some(cop) = map_op_to_conflict(mop) else { continue };
            let key = match mop {
                MapOp::Set { key, .. }
                | MapOp::Delete { key }
                | MapOp::SetBlob { key, .. } => key.clone(),
            };
            let Some(winner) = winners.get(&key) else { continue };
            // Skip the winner itself.
            if winner.lamport == tx.lamport && winner.author == tx.author {
                continue;
            }
            // Same author overwriting their own past write isn't a conflict.
            if winner.author == tx.author {
                continue;
            }
            out.push(ConflictEvent {
                kind: ConflictKind::MapOverwrite,
                key,
                winner_author: winner.author,
                winner_lamport: winner.lamport,
                winner_op: winner.op.clone(),
                loser_author: tx.author,
                loser_lamport: tx.lamport,
                loser_op: cop,
            });
        }
    }
}

// -----------------------------------------------------------------------------
// List
// -----------------------------------------------------------------------------

#[derive(Default, Clone)]
struct ListItemState {
    /// Highest-priority position-setting op (Insert or Move).
    pos_winner: Option<(u64, [u8; 32], ConflictOp)>,
    /// Highest-priority Delete, if any.
    delete_winner: Option<(u64, [u8; 32])>,
    /// All position-setting ops we've seen, for loser enumeration.
    pos_ops: Vec<(u64, [u8; 32], ConflictOp)>,
}

fn detect_list_conflicts(nodes: &[&SyncNode], out: &mut Vec<ConflictEvent>) {
    // Group by (list_key, item_id).
    let mut items: HashMap<(String, ItemId), ListItemState> = HashMap::new();

    for node in nodes {
        let tx = &node.transaction;
        for op in &tx.ops {
            let Op::List(lop) = op else { continue };
            match lop {
                ListOp::Insert {
                    list_key,
                    item_id,
                    ..
                }
                | ListOp::Move {
                    list_key,
                    item_id,
                    ..
                } => {
                    let st = items
                        .entry((list_key.clone(), *item_id))
                        .or_default();
                    let cop = match lop {
                        ListOp::Insert { item_id, .. } => ConflictOp::ListInsert {
                            item_id: item_id.0,
                        },
                        ListOp::Move { item_id, .. } => ConflictOp::ListMove {
                            item_id: item_id.0,
                        },
                        _ => unreachable!(),
                    };
                    st.pos_ops.push((tx.lamport, tx.author, cop.clone()));
                    if st
                        .pos_winner
                        .as_ref()
                        .map(|(l, a, _)| (tx.lamport, tx.author) > (*l, *a))
                        .unwrap_or(true)
                    {
                        st.pos_winner = Some((tx.lamport, tx.author, cop));
                    }
                }
                ListOp::Delete { list_key, item_id } => {
                    let st = items
                        .entry((list_key.clone(), *item_id))
                        .or_default();
                    if st
                        .delete_winner
                        .as_ref()
                        .map(|(l, a)| (tx.lamport, tx.author) > (*l, *a))
                        .unwrap_or(true)
                    {
                        st.delete_winner = Some((tx.lamport, tx.author));
                    }
                }
            }
        }
    }

    for ((list_key, item_id), st) in &items {
        match (&st.pos_winner, &st.delete_winner) {
            // Delete wins — every position op from a different author
            // is a list_delete_won loser.
            (Some(_), Some((dl, da))) => {
                let key = format!("{}#{}", list_key, item_id.to_hex());
                for (pl, pa, pop) in &st.pos_ops {
                    if pa == da {
                        continue;
                    }
                    out.push(ConflictEvent {
                        kind: ConflictKind::ListDeleteWon,
                        key: key.clone(),
                        winner_author: *da,
                        winner_lamport: *dl,
                        winner_op: ConflictOp::ListDelete {
                            item_id: item_id.0,
                        },
                        loser_author: *pa,
                        loser_lamport: *pl,
                        loser_op: pop.clone(),
                    });
                }
            }
            // No delete — multiple Move/Insert competing. Record losers
            // from different authors than the winner.
            (Some((wl, wa, wop)), None) => {
                let key = format!("{}#{}", list_key, item_id.to_hex());
                for (pl, pa, pop) in &st.pos_ops {
                    if pl == wl && pa == wa {
                        continue;
                    }
                    if pa == wa {
                        continue;
                    }
                    out.push(ConflictEvent {
                        kind: ConflictKind::ListMoveLost,
                        key: key.clone(),
                        winner_author: *wa,
                        winner_lamport: *wl,
                        winner_op: wop.clone(),
                        loser_author: *pa,
                        loser_lamport: *pl,
                        loser_op: pop.clone(),
                    });
                }
            }
            // Pure delete (item never inserted) or pure insert with
            // single author — no conflict surface here.
            _ => {}
        }
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::StateGraph;
    use crate::list::{after, first, FracIdx};
    use crate::op::{ItemId, ListOp, MapOp, Op};
    use ed25519_dalek::SigningKey;

    fn signing(seed: u8) -> SigningKey {
        let mut s = [0u8; 32];
        s[0] = seed;
        SigningKey::from_bytes(&s)
    }

    fn merge_into(dst: &mut StateGraph, src: &StateGraph) {
        let ids = src.all_node_ids();
        let refs = src.get_nodes(&ids);
        let owned: Vec<crate::node::SyncNode> = refs.into_iter().cloned().collect();
        let res = dst.apply_remote_batch(owned);
        assert!(res.rejected.is_empty(), "merge rejected: {:?}", res.rejected);
    }

    #[test]
    fn map_concurrent_set_emits_overwrite() {
        let ka = signing(1);
        let kb = signing(2);
        let mut a = StateGraph::new();
        let mut b = StateGraph::new();
        a.apply_local(
            &ka,
            0,
            vec![Op::Map(MapOp::Set {
                key: "k".into(),
                value: b"a".to_vec(),
            })],
        )
        .unwrap();
        b.apply_local(
            &kb,
            0,
            vec![Op::Map(MapOp::Set {
                key: "k".into(),
                value: b"b".to_vec(),
            })],
        )
        .unwrap();
        merge_into(&mut a, &b);

        let conflicts = a.detect_conflicts();
        assert_eq!(conflicts.len(), 1, "expected one MapOverwrite");
        let c = &conflicts[0];
        assert_eq!(c.kind, ConflictKind::MapOverwrite);
        assert_eq!(c.key, "k");
        let winner_value = match &c.winner_op {
            ConflictOp::Set { value } => value.clone(),
            _ => panic!("winner not Set"),
        };
        let loser_value = match &c.loser_op {
            ConflictOp::Set { value } => value.clone(),
            _ => panic!("loser not Set"),
        };
        assert!(winner_value == b"a" || winner_value == b"b");
        assert_ne!(winner_value, loser_value);
    }

    #[test]
    fn map_same_author_overwrite_is_not_conflict() {
        let ka = signing(1);
        let mut a = StateGraph::new();
        a.apply_local(
            &ka,
            0,
            vec![Op::Map(MapOp::Set {
                key: "k".into(),
                value: b"a".to_vec(),
            })],
        )
        .unwrap();
        a.apply_local(
            &ka,
            1,
            vec![Op::Map(MapOp::Set {
                key: "k".into(),
                value: b"b".to_vec(),
            })],
        )
        .unwrap();
        let conflicts = a.detect_conflicts();
        assert!(conflicts.is_empty());
    }

    #[test]
    fn list_concurrent_move_emits_move_lost() {
        let ka = signing(1);
        let kb = signing(2);
        let mut a = StateGraph::new();
        let mut b = StateGraph::new();
        let item = ItemId([7u8; 16]);
        let p0 = first();
        let p_left = FracIdx::new("40");
        let p_right: FracIdx = after(&p0);

        a.apply_local(
            &ka,
            0,
            vec![Op::List(ListOp::Insert {
                list_key: "L".into(),
                item_id: item,
                position: p0.clone(),
            })],
        )
        .unwrap();
        merge_into(&mut b, &a);

        a.apply_local(
            &ka,
            1,
            vec![Op::List(ListOp::Move {
                list_key: "L".into(),
                item_id: item,
                position: p_left,
            })],
        )
        .unwrap();
        b.apply_local(
            &kb,
            1,
            vec![Op::List(ListOp::Move {
                list_key: "L".into(),
                item_id: item,
                position: p_right,
            })],
        )
        .unwrap();

        merge_into(&mut a, &b);

        let conflicts = a.detect_conflicts();
        let move_lost: Vec<_> = conflicts
            .iter()
            .filter(|c| c.kind == ConflictKind::ListMoveLost)
            .collect();
        assert!(
            !move_lost.is_empty(),
            "expected at least one ListMoveLost, got {:?}",
            conflicts
        );
    }

    #[test]
    fn list_delete_emits_delete_won() {
        let ka = signing(1);
        let kb = signing(2);
        let mut a = StateGraph::new();
        let mut b = StateGraph::new();
        let item = ItemId([3u8; 16]);
        let p0 = first();
        let p_left = FracIdx::new("40");

        a.apply_local(
            &ka,
            0,
            vec![Op::List(ListOp::Insert {
                list_key: "L".into(),
                item_id: item,
                position: p0,
            })],
        )
        .unwrap();

        merge_into(&mut b, &a);

        a.apply_local(
            &ka,
            1,
            vec![Op::List(ListOp::Move {
                list_key: "L".into(),
                item_id: item,
                position: p_left,
            })],
        )
        .unwrap();
        b.apply_local(
            &kb,
            1,
            vec![Op::List(ListOp::Delete {
                list_key: "L".into(),
                item_id: item,
            })],
        )
        .unwrap();

        merge_into(&mut a, &b);

        let conflicts = a.detect_conflicts();
        let delete_won: Vec<_> = conflicts
            .iter()
            .filter(|c| c.kind == ConflictKind::ListDeleteWon)
            .collect();
        assert!(
            !delete_won.is_empty(),
            "expected a ListDeleteWon, got {:?}",
            conflicts
        );
    }
}
