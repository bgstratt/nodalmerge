//! C1: RGA (Replicated Growable Array) collaborative text.
//!
//! Each character has a stable [`OpId`] identity derived from the containing
//! transaction's `(lamport, author)`.  When two peers insert after the same
//! predecessor concurrently, the higher-priority op (larger `OpId`) appears
//! leftward in the output — giving a deterministic, convergent total order
//! on every peer without coordination.
//!
//! # Algorithm
//! Insertions form a forest rooted at a sentinel (represented as `None`).
//! Each insert's subtree is the set of characters inserted immediately after
//! it.  Siblings (same `after` parent) are sorted in **descending** `OpId`
//! order so higher-priority chars appear first.  A pre-order DFS traversal
//! of this forest yields the final character sequence.
//!
//! Deletions are tombstones: the character is excluded from the visible
//! output but its `OpId` remains valid as a `before`/`after` anchor for
//! future insertions.

use std::collections::{HashMap, HashSet};

use crate::node::SyncNode;
use crate::op::{Op, OpId, TextOp};

/// Resolve the visible RGA character sequence for `key`.
///
/// Returns `(OpId, char)` pairs in sequence order, tombstoned characters
/// excluded.  The `OpId` is the stable identity of each character and is
/// used by the bridge to map cursor positions to RGA positions.
pub fn resolve_text_seq(nodes: &[&SyncNode], key: &str) -> Vec<(OpId, char)> {
    let mut inserts: Vec<(OpId, Option<OpId>, char)> = Vec::new();
    let mut deletes: HashSet<OpId> = HashSet::new();

    for node in nodes {
        let tx = &node.transaction;
        // The RGA identity of any Insert in this transaction is (lamport, author).
        let node_id = OpId { lamport: tx.lamport, author: tx.author };
        for op in &tx.ops {
            match op {
                Op::Text(TextOp::Insert { key: k, after, ch }) if k == key => {
                    inserts.push((node_id, *after, *ch));
                }
                Op::Text(TextOp::Delete { key: k, target }) if k == key => {
                    deletes.insert(*target);
                }
                _ => {}
            }
        }
    }

    rga_sequence(&inserts, &deletes)
}

/// Resolve the RGA text for `key` as a plain UTF-8 `String`.
pub fn resolve_text(nodes: &[&SyncNode], key: &str) -> String {
    resolve_text_seq(nodes, key)
        .into_iter()
        .map(|(_, ch)| ch)
        .collect()
}

/// Build the linearized character sequence from raw RGA insert/delete data.
///
/// `inserts`: `(id, after, ch)` tuples from the node log.
/// `deletes`: set of tombstoned character ids.
///
/// Returns visible `(OpId, char)` pairs in the RGA-determined order.
fn rga_sequence(
    inserts: &[(OpId, Option<OpId>, char)],
    deletes: &HashSet<OpId>,
) -> Vec<(OpId, char)> {
    // parent -> children, sorted **descending** (higher priority = leftward).
    let mut children: HashMap<Option<OpId>, Vec<OpId>> = HashMap::new();
    let mut id_to_char: HashMap<OpId, char> = HashMap::new();

    for &(id, after, ch) in inserts {
        children.entry(after).or_default().push(id);
        id_to_char.insert(id, ch);
    }

    for siblings in children.values_mut() {
        siblings.sort_unstable_by(|a, b| b.cmp(a)); // descending
    }

    // Iterative pre-order DFS.
    // We push siblings in *reverse* order so the highest-priority one is on
    // top of the stack and therefore emitted first.
    let mut result = Vec::new();
    let mut stack: Vec<OpId> = Vec::new();

    if let Some(roots) = children.get(&None) {
        for &id in roots.iter().rev() {
            stack.push(id);
        }
    }

    while let Some(id) = stack.pop() {
        if !deletes.contains(&id) {
            if let Some(&ch) = id_to_char.get(&id) {
                result.push((id, ch));
            }
        }
        // Push this node's children (already sorted descending; push in reverse
        // so the first (highest-priority) child is popped first).
        if let Some(ch_ids) = children.get(&Some(id)) {
            for &child_id in ch_ids.iter().rev() {
                stack.push(child_id);
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StateGraph, op::TextOp};
    use ed25519_dalek::SigningKey;

    fn sk(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    /// Compute the `OpId` that `graph.apply_local(key, wall_ms, [op])` will
    /// assign, without actually inserting anything.  Mirrors the formula
    /// `(graph.lamport() + 1).max(wall_ms)`.
    fn next_op_id(graph: &StateGraph, key: &SigningKey, wall_ms: u64) -> OpId {
        let lamport = (graph.lamport() + 1).max(wall_ms);
        OpId { lamport, author: key.verifying_key().to_bytes() }
    }

    #[test]
    fn text_insert_sequential() {
        let sk = sk(1);
        let mut g = StateGraph::new();

        // 'h' at start
        let id_h = next_op_id(&g, &sk, 1000);
        g.apply_local(&sk, 1000, vec![
            Op::Text(TextOp::Insert { key: "doc".into(), after: None, ch: 'h' }),
        ]).unwrap();

        // 'i' after 'h'
        let id_i = next_op_id(&g, &sk, 1001);
        g.apply_local(&sk, 1001, vec![
            Op::Text(TextOp::Insert { key: "doc".into(), after: Some(id_h), ch: 'i' }),
        ]).unwrap();

        assert_eq!(g.resolve_text("doc"), "hi");
        let seq = g.resolve_text_seq("doc");
        assert_eq!(seq.len(), 2);
        assert_eq!(seq[0], (id_h, 'h'));
        assert_eq!(seq[1], (id_i, 'i'));
    }

    #[test]
    fn text_delete() {
        let sk = sk(1);
        let mut g = StateGraph::new();

        let id_x = next_op_id(&g, &sk, 1000);
        g.apply_local(&sk, 1000, vec![
            Op::Text(TextOp::Insert { key: "doc".into(), after: None, ch: 'x' }),
        ]).unwrap();

        g.apply_local(&sk, 1001, vec![
            Op::Text(TextOp::Delete { key: "doc".into(), target: id_x }),
        ]).unwrap();

        assert_eq!(g.resolve_text("doc"), "");
    }

    #[test]
    fn text_insert_middle() {
        // Build "ac" then insert 'b' between them via sequential edits.
        let sk = sk(1);
        let mut g = StateGraph::new();

        let id_a = next_op_id(&g, &sk, 1000);
        g.apply_local(&sk, 1000, vec![
            Op::Text(TextOp::Insert { key: "doc".into(), after: None, ch: 'a' }),
        ]).unwrap();

        let id_c = next_op_id(&g, &sk, 1001);
        g.apply_local(&sk, 1001, vec![
            Op::Text(TextOp::Insert { key: "doc".into(), after: Some(id_a), ch: 'c' }),
        ]).unwrap();

        // Insert 'b' at pos 1 (after 'a', before 'c').
        // 'b' has higher lamport (1002) than 'c' (1001) → higher OpId → goes left.
        g.apply_local(&sk, 1002, vec![
            Op::Text(TextOp::Insert { key: "doc".into(), after: Some(id_a), ch: 'b' }),
        ]).unwrap();

        assert_eq!(g.resolve_text("doc"), "abc");
        let _ = id_c;
    }

    #[test]
    fn text_concurrent_insert_converges() {
        // Two peers both insert at the start (after None) concurrently.
        // Both should arrive at the same string.
        let sk_a = sk(0xff);
        let sk_b = sk(0x01);
        let author_a = sk_a.verifying_key().to_bytes();
        let author_b = sk_b.verifying_key().to_bytes();

        // Peer A inserts 'A' with lamport 1.
        let mut g_a = StateGraph::new();
        g_a.apply_local(&sk_a, 1000, vec![
            Op::Text(TextOp::Insert { key: "doc".into(), after: None, ch: 'A' }),
        ]).unwrap();

        // Peer B inserts 'B' with the same lamport (both start at 0).
        let mut g_b = StateGraph::new();
        g_b.apply_local(&sk_b, 1000, vec![
            Op::Text(TextOp::Insert { key: "doc".into(), after: None, ch: 'B' }),
        ]).unwrap();

        // Exchange nodes.
        {
            let a_ids = g_a.all_node_ids();
            let a_nodes: Vec<_> = g_a.get_nodes(&a_ids).into_iter().cloned().collect();
            let b_ids = g_b.all_node_ids();
            let b_nodes: Vec<_> = g_b.get_nodes(&b_ids).into_iter().cloned().collect();
            for n in b_nodes { g_a.apply_remote(n).unwrap(); }
            for n in a_nodes { g_b.apply_remote(n).unwrap(); }
        }

        let text_a = g_a.resolve_text("doc");
        let text_b = g_b.resolve_text("doc");
        assert_eq!(text_a, text_b, "peers must converge to identical text");
        assert!(text_a.contains('A') && text_a.contains('B'), "both chars must survive");

        // The peer with higher (lamport, author) in OpId appears leftward.
        // Both have lamport=1000, so it's decided by author bytes (verifying key, not seed).
        let a_wins = author_a > author_b;
        let pos_a = text_a.find('A').unwrap();
        let pos_b = text_a.find('B').unwrap();
        if a_wins {
            assert!(pos_a < pos_b, "A should appear left (higher author priority): got {:?}", text_a);
        } else {
            assert!(pos_b < pos_a, "B should appear left (higher author priority): got {:?}", text_a);
        }
    }

    #[test]
    fn text_delete_with_concurrent_insert_around_it() {
        // Tombstone a char that another peer inserted around.
        let sk = sk(2);
        let mut g = StateGraph::new();

        // Insert "ab"
        let id_a = next_op_id(&g, &sk, 1000);
        g.apply_local(&sk, 1000, vec![
            Op::Text(TextOp::Insert { key: "doc".into(), after: None, ch: 'a' }),
        ]).unwrap();

        let id_b = next_op_id(&g, &sk, 1001);
        g.apply_local(&sk, 1001, vec![
            Op::Text(TextOp::Insert { key: "doc".into(), after: Some(id_a), ch: 'b' }),
        ]).unwrap();

        // Delete 'a'
        g.apply_local(&sk, 1002, vec![
            Op::Text(TextOp::Delete { key: "doc".into(), target: id_a }),
        ]).unwrap();

        // 'b' is still anchored after 'a' even though 'a' is tombstoned.
        assert_eq!(g.resolve_text("doc"), "b");
        let seq = g.resolve_text_seq("doc");
        assert_eq!(seq.len(), 1);
        assert_eq!(seq[0], (id_b, 'b'));
    }

    #[test]
    fn text_multiple_keys_independent() {
        let sk = sk(1);
        let mut g = StateGraph::new();

        g.apply_local(&sk, 1000, vec![
            Op::Text(TextOp::Insert { key: "title".into(), after: None, ch: 'H' }),
        ]).unwrap();
        g.apply_local(&sk, 1001, vec![
            Op::Text(TextOp::Insert { key: "body".into(), after: None, ch: 'X' }),
        ]).unwrap();

        assert_eq!(g.resolve_text("title"), "H");
        assert_eq!(g.resolve_text("body"), "X");
    }
}
