//! F8: Fractional-index List CRDT.
//!
//! An ordered collection of items where:
//!
//! * Each item has a stable [`ItemId`] (UUIDv4, 16 random bytes) chosen by
//!   the inserter. Item identity is independent of position and survives
//!   moves.
//! * Each item carries a [`FracIdx`] — a base-62 string position. Sorting
//!   items by `(FracIdx, ItemId)` ascending yields the visible order.
//! * Item *content* lives in a sidecar [`crate::op::Op::Map`]. The list
//!   primitive is concerned only with order and existence.
//!
//! # Concurrency model
//!
//! Per item id, we apply LWW on `(lamport, author)`:
//!
//! * `Insert` and `Move` both **set** the position. Higher `(lamport, author)`
//!   wins. Two concurrent moves of the same id converge deterministically.
//! * `Delete` is **absorbing**. Once an item id has any `Delete` op
//!   anywhere in the DAG, the item is permanently tombstoned — later
//!   `Move`/`Insert` ops with the same id are ignored.
//!
//! This matches the F8 design entry in `PLAN.md`. Move is *not*
//! delete+insert: a concurrent `Move(X)` from Alice and content edit on
//! the sidecar `X` from Bob compose cleanly because they touch
//! orthogonal state.
//!
//! # Fractional indexing
//!
//! Positions are arbitrary-length base-62 strings over the alphabet
//! `0..9 A..Z a..z` (lex-ordered by ASCII code). [`between`] returns a
//! string strictly between two endpoints (or open-ended on either side),
//! suitable for an `Insert`/`Move` between two visible neighbors.
//!
//! Adversarial edit sequences (always insert at position 0) grow the
//! string length linearly. The SDK is responsible for emitting a
//! rebalance op when any position exceeds [`REBALANCE_THRESHOLD`]; the
//! core ships only the primitive.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::node::SyncNode;
use crate::op::{ItemId, ListOp, Op};

// ---------------------------------------------------------------------------
// FracIdx: base-62 fractional-index position string
// ---------------------------------------------------------------------------

/// A fractional-index position in a list.
///
/// The wrapped `String` uses the base-62 alphabet `0..9 A..Z a..z`. Standard
/// `Ord` (lex byte compare) gives the visible order because the alphabet's
/// ASCII codes are themselves lex-sorted.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FracIdx(pub String);

impl FracIdx {
    /// Construct from a raw position string. No validation — callers are
    /// expected to obtain positions from [`between`] / [`first`] / [`last`].
    pub fn new(s: impl Into<String>) -> Self {
        FracIdx(s.into())
    }

    /// Borrow the underlying position string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Length in bytes. Used by SDK to decide when to rebalance.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Position-string length above which the SDK should emit a rebalance op.
///
/// Empirically, well-distributed inserts stay under ~6 bytes for millions
/// of items; this trigger only fires under adversarial "always insert at
/// front" sequences.
pub const REBALANCE_THRESHOLD: usize = 128;

// Base-62 alphabet, lexicographically ordered by ASCII code.
const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const RADIX: u32 = 62;

#[inline]
fn val(c: u8) -> u32 {
    match c {
        b'0'..=b'9' => (c - b'0') as u32,
        b'A'..=b'Z' => (c - b'A') as u32 + 10,
        b'a'..=b'z' => (c - b'a') as u32 + 36,
        _ => panic!("FracIdx contains non-base62 byte: {c:#x}"),
    }
}

#[inline]
fn ch(v: u32) -> u8 {
    debug_assert!(v < RADIX, "char value {v} out of range");
    ALPHABET[v as usize]
}

/// Position to use when both endpoints are unbounded — i.e. the very first
/// insert into an empty list.
pub fn first() -> FracIdx {
    // Midpoint of the alphabet keeps room on both sides.
    FracIdx(String::from("V"))
}

/// Position strictly less than `right`. Used for "insert at front".
pub fn before(right: &FracIdx) -> FracIdx {
    between(None, Some(right))
}

/// Position strictly greater than `left`. Used for "insert at end".
pub fn after(left: &FracIdx) -> FracIdx {
    between(Some(left), None)
}

/// Compute a position strictly between `left` and `right` in lex order.
///
/// * `(None, None)` → centerpoint of the alphabet.
/// * `(Some(a), None)` → some `m` with `a < m`.
/// * `(None, Some(b))` → some `m` with `m < b`.
/// * `(Some(a), Some(b))` with `a < b` → some `m` with `a < m < b`.
///
/// Panics if `left >= right` when both are provided.
///
/// # Degenerate edge case
///
/// "Insert before `'0'`" (the smallest single-digit position) cannot
/// produce a non-empty string less than `"0"` and will panic. Real lists
/// never reach this state because [`first`] returns the alphabet midpoint
/// `"V"`, leaving room on both sides; only an explicit rebalance to
/// the boundary could expose this case. Callers must rebalance before
/// inserting at the front of a list whose minimum is `"0"`.
pub fn between(left: Option<&FracIdx>, right: Option<&FracIdx>) -> FracIdx {
    if let (Some(l), Some(r)) = (left, right) {
        assert!(l < r, "between() requires left < right (got {l:?}, {r:?})");
    }
    let l = left.map(|f| f.0.as_bytes()).unwrap_or(&[]);
    let r_opt = right.map(|f| f.0.as_bytes());
    FracIdx(String::from_utf8(between_raw(l, r_opt)).expect("base62 is utf8"))
}

fn between_raw(left: &[u8], right: Option<&[u8]>) -> Vec<u8> {
    match right {
        None => between_unbounded_above(left),
        Some(r) => between_bounded(left, r),
    }
}

/// Produce a position strictly greater than `left`.
fn between_unbounded_above(left: &[u8]) -> Vec<u8> {
    // Walk left looking for the first digit < RADIX-1; emit the midpoint
    // between it and RADIX (one past max), keeping the prefix.
    for (i, &b) in left.iter().enumerate() {
        let d = val(b);
        if d < RADIX - 1 {
            let mut out = left[..i].to_vec();
            out.push(ch((d + RADIX) / 2));
            return out;
        }
    }
    // All digits are at the max ('z'). Append a midpoint digit.
    let mut out = left.to_vec();
    out.push(ch(RADIX / 2));
    out
}

/// Produce a position strictly between `left` and `right`. Requires
/// `left < right` lexicographically.
fn between_bounded(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    let mut i = 0usize;
    loop {
        let la_opt = left.get(i).copied().map(val);
        let rb_opt = right.get(i).copied().map(val);

        match (la_opt, rb_opt) {
            (Some(la), Some(rb)) if la == rb => {
                // Common prefix digit — keep walking.
                out.push(ch(la));
                i += 1;
            }
            (Some(la), Some(rb)) => {
                // First differing digit. la < rb because left < right.
                debug_assert!(la < rb);
                if rb - la >= 2 {
                    out.push(ch((la + rb) / 2));
                    return out;
                }
                // Adjacent (rb == la + 1). Emit la; right becomes effectively
                // unbounded above from here on (any string starting with
                // out+ch(la) is < right, which starts with prefix+ch(rb)).
                out.push(ch(la));
                let mut tail = between_unbounded_above(&left[i + 1..]);
                out.append(&mut tail);
                return out;
            }
            (None, Some(rb)) => {
                // left is a prefix of right (or both share the prefix walked
                // so far and left is now exhausted). Need c with c > left
                // (any non-empty extension of `out` past this point qualifies)
                // and c < right.
                if rb >= 2 {
                    out.push(ch(rb / 2));
                    return out;
                }
                // rb is 0 or 1. Emit 0 and continue, with left empty.
                out.push(ch(0));
                if rb == 0 {
                    // We've now matched right[..i+1] exactly; recurse with
                    // empty left and right's tail.
                    return splice(out, between_bounded(&[], &right[i + 1..]));
                }
                // rb == 1: out is now prefix+'0' which is < right (since
                // right[i] = '1' here). Need c > out (out is what we've
                // emitted, equal to original prefix + '0'). Append a
                // midpoint to make it strictly larger.
                out.push(ch(RADIX / 2));
                return out;
            }
            (Some(_), None) => {
                // right ran out but left didn't, meaning left's prefix
                // matched right exactly and left has more — left > right.
                // Caller violated the precondition.
                panic!(
                    "between_bounded: left {:?} >= right {:?}",
                    String::from_utf8_lossy(left),
                    String::from_utf8_lossy(right)
                );
            }
            (None, None) => {
                panic!(
                    "between_bounded: left == right ({:?})",
                    String::from_utf8_lossy(left)
                );
            }
        }
    }
}

fn splice(mut head: Vec<u8>, mut tail: Vec<u8>) -> Vec<u8> {
    head.append(&mut tail);
    head
}

// ---------------------------------------------------------------------------
// List resolution
// ---------------------------------------------------------------------------

/// Resolved state of a single list item.
#[derive(Debug, Clone)]
struct ItemState {
    /// Current winning position, set by the highest-priority `Insert` or
    /// `Move` op observed so far for this id. `None` only as a transient
    /// before any position-bearing op has been visited.
    position: Option<FracIdx>,
    /// `(lamport, author)` of the op that wrote `position`. Insert and Move
    /// share this comparator — they're equivalent for position.
    pos_priority: (u64, [u8; 32]),
    /// Set by any `Insert` for this id. An item is visible only after at
    /// least one Insert has been observed; a "lonely Move" (Move with no
    /// Insert anywhere in the DAG, which shouldn't happen under correct
    /// callers but might under adversarial input) does not produce a
    /// phantom item.
    saw_insert: bool,
    /// Set by any `Delete`. Absorbing — once true, the item is hidden
    /// regardless of later/concurrent Insert/Move ops.
    deleted: bool,
}

impl Default for ItemState {
    fn default() -> Self {
        ItemState {
            position: None,
            pos_priority: (0, [0u8; 32]),
            saw_insert: false,
            deleted: false,
        }
    }
}

/// Resolve the visible items of `list_key` from the DAG nodes.
///
/// Returns `(ItemId, FracIdx)` pairs sorted by `(FracIdx, ItemId)` ascending.
/// Tombstoned items are omitted.
///
/// # Order independence
///
/// The algorithm processes nodes in **whatever order** the caller supplies;
/// the result depends only on the *set* of nodes, not their iteration order.
/// `Insert` and `Move` are unified for position-LWW so an out-of-order
/// `Move` followed by its `Insert` produces the same result as the in-order
/// case. `Delete` is absorbing and idempotent.
pub fn resolve_list_seq(nodes: &[&SyncNode], list_key: &str) -> Vec<(ItemId, FracIdx)> {
    let mut items: HashMap<ItemId, ItemState> = HashMap::new();

    for node in nodes {
        let tx = &node.transaction;
        let prio = (tx.lamport, tx.author);
        for op in &tx.ops {
            let Op::List(lop) = op else { continue };
            match lop {
                ListOp::Insert {
                    list_key: k,
                    item_id,
                    position,
                } if k == list_key => {
                    let entry = items.entry(*item_id).or_default();
                    entry.saw_insert = true;
                    if prio > entry.pos_priority {
                        entry.position = Some(position.clone());
                        entry.pos_priority = prio;
                    }
                }
                ListOp::Move {
                    list_key: k,
                    item_id,
                    position,
                } if k == list_key => {
                    // Treat Move as position-LWW exactly like Insert. The
                    // visibility gate (`saw_insert`) ensures Move alone
                    // doesn't materialize a phantom item.
                    let entry = items.entry(*item_id).or_default();
                    if prio > entry.pos_priority {
                        entry.position = Some(position.clone());
                        entry.pos_priority = prio;
                    }
                }
                ListOp::Delete {
                    list_key: k,
                    item_id,
                } if k == list_key => {
                    let entry = items.entry(*item_id).or_default();
                    entry.deleted = true;
                }
                _ => {}
            }
        }
    }

    let mut visible: Vec<(ItemId, FracIdx)> = items
        .into_iter()
        .filter_map(|(id, st)| {
            if !st.saw_insert || st.deleted {
                None
            } else {
                st.position.map(|p| (id, p))
            }
        })
        .collect();
    // Sort by (position, item_id) — item_id breaks ties when two items end up
    // at literally identical positions (e.g. two concurrent inserts at the
    // exact same calculated position).
    visible.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.0.cmp(&b.0.0)));
    visible
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fi(s: &str) -> FracIdx {
        FracIdx::new(s)
    }

    #[test]
    fn first_is_alphabet_midpoint() {
        assert_eq!(first().0, "V");
    }

    #[test]
    fn between_unbounded_returns_midpoint() {
        let m = between(None, None);
        assert_eq!(m.0, "V");
    }

    #[test]
    fn between_after_a_returns_greater() {
        let a = fi("A");
        let m = between(Some(&a), None);
        assert!(m > a, "{m:?} should be > {a:?}");
    }

    #[test]
    fn between_before_z_returns_lesser() {
        let b = fi("z");
        let m = between(None, Some(&b));
        assert!(m < b, "{m:?} should be < {b:?}");
    }

    #[test]
    fn between_two_with_room_picks_midpoint() {
        let a = fi("A");
        let b = fi("Z");
        let m = between(Some(&a), Some(&b));
        assert!(a < m && m < b, "{a:?} < {m:?} < {b:?}");
    }

    #[test]
    fn between_adjacent_recurses() {
        // 'A' and 'B' are adjacent in the alphabet (vals 10 and 11).
        let a = fi("A");
        let b = fi("B");
        let m = between(Some(&a), Some(&b));
        assert!(a < m && m < b, "{a:?} < {m:?} < {b:?}");
        // The result should start with 'A' and have a continuation.
        assert!(m.0.starts_with('A'));
        assert!(m.0.len() > 1);
    }

    #[test]
    fn between_after_max_appends() {
        let a = fi("z");
        let m = between(Some(&a), None);
        assert!(m > a, "{m:?} should be > {a:?}");
        // Result must start with 'z' (we can't go higher in one digit) and
        // append a midpoint.
        assert!(m.0.starts_with('z'));
    }

    #[test]
    #[should_panic(expected = "between_bounded")]
    fn between_before_zero_panics_documented_edge() {
        // "Insert before '0'" is impossible — there's no non-empty string
        // less than "0" in the alphabet. Documented in `between` doc-comment.
        // Real lists never reach this state because `first()` returns "V"
        // (alphabet midpoint) leaving room on both sides.
        let _ = between(None, Some(&fi("0")));
    }

    #[test]
    fn many_inserts_at_end_grow_within_rebalance_window() {
        // Adversarial single-direction inserts: positions grow ~linearly with
        // a small constant. The design answer is the rebalance op fired at
        // REBALANCE_THRESHOLD; this test pins the rate so a rebalance every
        // few hundred inserts is sufficient.
        let mut prev = first();
        let mut crossed_threshold_at = None;
        for i in 0..2000 {
            let next = after(&prev);
            assert!(next > prev);
            prev = next;
            if crossed_threshold_at.is_none() && prev.len() >= REBALANCE_THRESHOLD {
                crossed_threshold_at = Some(i);
            }
        }
        // We expect adversarial end-inserts to *eventually* trip the rebalance
        // threshold — that's the design. Pin the rate: at least 200 inserts
        // before triggering, so rebalance is rare in practice.
        let trip = crossed_threshold_at.expect("never reached threshold in 2000 ops");
        assert!(trip > 200, "rebalance threshold tripped too early at op {trip}");
    }

    #[test]
    fn many_inserts_at_front_grow_within_rebalance_window() {
        // Same as above but inserting at the front. Symmetric behavior.
        let mut anchor = first();
        let mut crossed_threshold_at = None;
        for i in 0..2000 {
            let next = before(&anchor);
            assert!(next < anchor);
            anchor = next;
            if crossed_threshold_at.is_none() && anchor.len() >= REBALANCE_THRESHOLD {
                crossed_threshold_at = Some(i);
            }
        }
        let trip = crossed_threshold_at.expect("never reached threshold in 2000 ops");
        assert!(trip > 200, "rebalance threshold tripped too early at op {trip}");
    }

    #[test]
    fn between_two_neighbors_stays_compact() {
        // The realistic case: insert between two existing items. Each step
        // halves the available digit space, so log_2(62) ≈ 6 inserts per
        // new digit — extremely efficient.
        let mut lo = fi("A");
        let hi = fi("z");
        for _ in 0..100 {
            let mid = between(Some(&lo), Some(&hi));
            assert!(mid > lo && mid < hi);
            lo = mid;
        }
        // 100 between-inserts: should stay tiny (< ~20 bytes).
        assert!(
            lo.len() < 32,
            "between-inserts grew unexpectedly: {} bytes",
            lo.len()
        );
    }

    #[test]
    fn ord_matches_lex() {
        let mut v = vec![fi("Z"), fi("A"), fi("a"), fi("0"), fi("z")];
        v.sort();
        // Expected lex order: "0" < "A" < "Z" < "a" < "z".
        assert_eq!(
            v.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(),
            vec!["0", "A", "Z", "a", "z"]
        );
    }
}

// ---------------------------------------------------------------------------
// CRDT convergence tests (resolve_list_seq via StateGraph)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod resolve_tests {
    use super::*;
    use crate::op::{ItemId, ListOp, Op};
    use crate::StateGraph;
    use ed25519_dalek::SigningKey;

    fn sk(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn item(seed: u8) -> ItemId {
        ItemId([seed; 16])
    }

    fn insert(list_key: &str, id: ItemId, position: FracIdx) -> Op {
        Op::List(ListOp::Insert {
            list_key: list_key.into(),
            item_id: id,
            position,
        })
    }

    fn move_op(list_key: &str, id: ItemId, position: FracIdx) -> Op {
        Op::List(ListOp::Move {
            list_key: list_key.into(),
            item_id: id,
            position,
        })
    }

    fn delete(list_key: &str, id: ItemId) -> Op {
        Op::List(ListOp::Delete {
            list_key: list_key.into(),
            item_id: id,
        })
    }

    /// Drain all nodes from `from` into `into`, ignoring duplicates and
    /// retrying parent-missing nodes until the dependency graph is satisfied.
    fn sync(into: &mut StateGraph, from: &StateGraph) {
        let snap = snapshot(from);
        apply_all(into, &snap);
    }

    /// Take an owned snapshot of all nodes in `g`. Used to capture state
    /// before a bidirectional exchange so each side sees the other's pre-
    /// exchange view.
    fn snapshot(g: &StateGraph) -> Vec<crate::node::SyncNode> {
        let ids = g.all_node_ids();
        g.get_nodes(&ids).into_iter().cloned().collect()
    }

    /// Apply nodes in dependency order via repeated passes. `MemoryNodeStore`
    /// returns ids in hashmap iteration order, so a single linear pass can
    /// deliver children before parents and trip `MissingParent`.
    fn apply_all(g: &mut StateGraph, nodes: &[crate::node::SyncNode]) {
        let mut pending: Vec<_> = nodes.to_vec();
        loop {
            let mut next = Vec::new();
            let mut progress = false;
            for n in pending.drain(..) {
                match g.apply_remote(n.clone()) {
                    Ok(_) => progress = true,
                    Err(crate::SyncError::MissingParent(_)) => next.push(n),
                    Err(crate::SyncError::DuplicateNode(_)) => progress = true, // dupe is fine
                    Err(e) => panic!("apply_all: unexpected error applying node: {e:?}"),
                }
            }
            if next.is_empty() {
                return;
            }
            assert!(progress, "apply_all deadlock: pending nodes have unresolvable parents");
            pending = next;
        }
    }

    #[test]
    fn insert_three_items_in_order() {
        let key = sk(1);
        let mut g = StateGraph::new();
        let a = item(1);
        let b = item(2);
        let c = item(3);
        let pa = first();
        let pb = after(&pa);
        let pc = after(&pb);
        g.apply_local(&key, 1000, vec![insert("buttons", a, pa.clone())]).unwrap();
        g.apply_local(&key, 1001, vec![insert("buttons", b, pb.clone())]).unwrap();
        g.apply_local(&key, 1002, vec![insert("buttons", c, pc.clone())]).unwrap();

        let resolved = g.resolve_list("buttons");
        assert_eq!(resolved.len(), 3);
        assert_eq!(resolved[0].0, a);
        assert_eq!(resolved[1].0, b);
        assert_eq!(resolved[2].0, c);
    }

    #[test]
    fn delete_is_absorbing() {
        let key = sk(1);
        let mut g = StateGraph::new();
        let a = item(1);
        let pa = first();
        g.apply_local(&key, 1000, vec![insert("L", a, pa.clone())]).unwrap();
        g.apply_local(&key, 1001, vec![delete("L", a)]).unwrap();
        g.apply_local(&key, 1002, vec![move_op("L", a, after(&pa))]).unwrap();
        let resolved = g.resolve_list("L");
        assert_eq!(resolved.len(), 0, "deleted item must not appear");
    }

    #[test]
    fn move_repositions_existing_item() {
        let key = sk(1);
        let mut g = StateGraph::new();
        let a = item(1);
        let b = item(2);
        let c = item(3);
        let pa = first();
        let pb = after(&pa);
        let pc = after(&pb);
        g.apply_local(&key, 1000, vec![insert("L", a, pa.clone())]).unwrap();
        g.apply_local(&key, 1001, vec![insert("L", b, pb.clone())]).unwrap();
        g.apply_local(&key, 1002, vec![insert("L", c, pc.clone())]).unwrap();
        let new_pos = before(&pa);
        g.apply_local(&key, 1003, vec![move_op("L", c, new_pos.clone())]).unwrap();
        let resolved = g.resolve_list("L");
        assert_eq!(
            resolved.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![c, a, b]
        );
    }

    #[test]
    fn concurrent_moves_converge_lww() {
        // Alice and Bob both move the same item to different positions.
        // Both peers converge; item exists exactly once.
        let sk_a = sk(0xff);
        let sk_b = sk(0x01);
        let owner = sk(7);
        let a = item(1);
        let b = item(2);
        let pa = first();
        let pb = after(&pa);

        let mut g_owner = StateGraph::new();
        g_owner.apply_local(&owner, 1000, vec![insert("L", a, pa.clone())]).unwrap();
        g_owner.apply_local(&owner, 1001, vec![insert("L", b, pb.clone())]).unwrap();

        let mut g_a = StateGraph::new();
        let mut g_b = StateGraph::new();
        sync(&mut g_a, &g_owner);
        sync(&mut g_b, &g_owner);

        g_a.apply_local(&sk_a, 2000, vec![move_op("L", a, before(&pa))]).unwrap();
        g_b.apply_local(&sk_b, 2000, vec![move_op("L", a, after(&pb))]).unwrap();

        let snap_a = snapshot(&g_a);
        let snap_b = snapshot(&g_b);
        apply_all(&mut g_a, &snap_b);
        apply_all(&mut g_b, &snap_a);

        let res_a = g_a.resolve_list("L");
        let res_b = g_b.resolve_list("L");
        assert_eq!(res_a, res_b, "peers must converge");
        let count_a = res_a.iter().filter(|(id, _)| *id == a).count();
        assert_eq!(count_a, 1, "item must not duplicate under concurrent moves");
        assert!(res_a.iter().any(|(id, _)| *id == b));
    }

    #[test]
    fn concurrent_delete_and_move_delete_wins() {
        let sk_a = sk(0xff);
        let sk_b = sk(0x01);
        let owner = sk(7);
        let a = item(1);
        let b = item(2);
        let pa = first();
        let pb = after(&pa);

        let mut g_owner = StateGraph::new();
        g_owner.apply_local(&owner, 1000, vec![insert("L", a, pa.clone())]).unwrap();
        g_owner.apply_local(&owner, 1001, vec![insert("L", b, pb.clone())]).unwrap();

        let mut g_a = StateGraph::new();
        let mut g_b = StateGraph::new();
        sync(&mut g_a, &g_owner);
        sync(&mut g_b, &g_owner);

        g_a.apply_local(&sk_a, 2000, vec![delete("L", a)]).unwrap();
        g_b.apply_local(&sk_b, 2000, vec![move_op("L", a, after(&pb))]).unwrap();

        let snap_a = snapshot(&g_a);
        let snap_b = snapshot(&g_b);
        apply_all(&mut g_a, &snap_b);
        apply_all(&mut g_b, &snap_a);

        let res_a = g_a.resolve_list("L");
        let res_b = g_b.resolve_list("L");
        assert_eq!(res_a, res_b, "peers must converge");
        assert!(res_a.iter().all(|(id, _)| *id != a), "deleted item leaked");
        assert_eq!(res_a.len(), 1);
        assert_eq!(res_a[0].0, b);
    }

    #[test]
    fn concurrent_inserts_at_same_position_both_survive() {
        // Both peers compute the same midpoint and insert their own item.
        // Tie-break on item_id makes order deterministic.
        let sk_a = sk(0xff);
        let sk_b = sk(0x01);
        let owner = sk(7);
        let lo_id = item(1);
        let hi_id = item(2);
        let lo_pos = first();
        let hi_pos = after(&after(&lo_pos));

        let mut g_owner = StateGraph::new();
        g_owner.apply_local(&owner, 1000, vec![insert("L", lo_id, lo_pos.clone())]).unwrap();
        g_owner.apply_local(&owner, 1001, vec![insert("L", hi_id, hi_pos.clone())]).unwrap();

        let mut g_a = StateGraph::new();
        let mut g_b = StateGraph::new();
        sync(&mut g_a, &g_owner);
        sync(&mut g_b, &g_owner);

        let mid = between(Some(&lo_pos), Some(&hi_pos));
        let alice_item = item(0xa);
        let bob_item = item(0xb);
        g_a.apply_local(&sk_a, 2000, vec![insert("L", alice_item, mid.clone())]).unwrap();
        g_b.apply_local(&sk_b, 2000, vec![insert("L", bob_item, mid.clone())]).unwrap();

        let snap_a = snapshot(&g_a);
        let snap_b = snapshot(&g_b);
        apply_all(&mut g_a, &snap_b);
        apply_all(&mut g_b, &snap_a);

        let res_a = g_a.resolve_list("L");
        let res_b = g_b.resolve_list("L");
        assert_eq!(res_a, res_b, "peers must converge");
        assert_eq!(res_a.len(), 4, "all four items expected");
        let ids: Vec<_> = res_a.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&alice_item));
        assert!(ids.contains(&bob_item));
        let pos_alice = ids.iter().position(|id| *id == alice_item).unwrap();
        let pos_bob = ids.iter().position(|id| *id == bob_item).unwrap();
        assert!(pos_alice < pos_bob, "tie-break by ItemId byte order");
    }

    #[test]
    fn out_of_order_delivery_converges() {
        let key = sk(1);
        let mut g_src = StateGraph::new();
        let a = item(1);
        let b = item(2);
        let c = item(3);
        let pa = first();
        let pb = after(&pa);
        let pc = after(&pb);
        g_src.apply_local(&key, 1000, vec![insert("L", a, pa.clone())]).unwrap();
        g_src.apply_local(&key, 1001, vec![insert("L", b, pb.clone())]).unwrap();
        g_src.apply_local(&key, 1002, vec![insert("L", c, pc.clone())]).unwrap();
        g_src.apply_local(&key, 1003, vec![move_op("L", b, after(&pc))]).unwrap();

        let ids = g_src.all_node_ids();
        let nodes: Vec<_> = g_src.get_nodes(&ids).into_iter().cloned().collect();

        // Peer 1: deliver in whatever order all_node_ids returns (may be
        // hash-randomized for MemoryNodeStore). apply_all retries on
        // missing parents.
        let mut g1 = StateGraph::new();
        apply_all(&mut g1, &nodes);

        // Peer 2: deliver in reverse, retrying on missing-parent errors.
        let mut g2 = StateGraph::new();
        let reversed: Vec<_> = nodes.iter().rev().cloned().collect();
        apply_all(&mut g2, &reversed);

        assert_eq!(g1.resolve_list("L"), g2.resolve_list("L"));
    }

    // ----------------------------------------------------------------------
    // Gesture-level convergence (F8 SDK gestures)
    //
    // The SDK gesture helpers (`list.gestures.*`) are thin compositions over
    // the primitive ops. These tests exercise the *primitive sequences* the
    // gestures emit, not the JS code, and assert the convergence claims the
    // gesture API contracts make in PLAN.md F8:
    //   - dropOnto: target gone, both dragged items survive
    //   - dropBetween: dragged lands in the gap, identity preserved
    //   - swap: pairwise positions exchanged, no duplicates
    // ----------------------------------------------------------------------

    /// Both peers concurrently drop their own dragged item onto the same
    /// target. Per gesture spec: target is tombstoned and **neither
    /// dragged item is lost**.
    #[test]
    fn gesture_drop_onto_concurrent_preserves_both_drags() {
        let sk_a = sk(0xff);
        let sk_b = sk(0x01);
        let owner = sk(7);
        let target = item(1);
        let drag_a = item(0xaa);
        let drag_b = item(0xbb);
        let p_target = first();
        let p_drag_a = before(&p_target);
        let p_drag_b = after(&p_target);

        // Owner sets up: [drag_a, target, drag_b]
        let mut g_owner = StateGraph::new();
        g_owner.apply_local(&owner, 1000, vec![insert("L", drag_a, p_drag_a.clone())]).unwrap();
        g_owner.apply_local(&owner, 1001, vec![insert("L", target, p_target.clone())]).unwrap();
        g_owner.apply_local(&owner, 1002, vec![insert("L", drag_b, p_drag_b.clone())]).unwrap();

        let mut g_a = StateGraph::new();
        let mut g_b = StateGraph::new();
        sync(&mut g_a, &g_owner);
        sync(&mut g_b, &g_owner);

        // Both peers' gesture: delete(target) + move(my_drag, target's slot).
        // Each peer computes "target's position" locally, which is identical
        // since both have the same view at the time of gesture.
        g_a.apply_local(&sk_a, 2000, vec![
            delete("L", target),
            move_op("L", drag_a, p_target.clone()),
        ]).unwrap();
        g_b.apply_local(&sk_b, 2000, vec![
            delete("L", target),
            move_op("L", drag_b, p_target.clone()),
        ]).unwrap();

        let snap_a = snapshot(&g_a);
        let snap_b = snapshot(&g_b);
        apply_all(&mut g_a, &snap_b);
        apply_all(&mut g_b, &snap_a);

        let res_a = g_a.resolve_list("L");
        let res_b = g_b.resolve_list("L");
        assert_eq!(res_a, res_b, "peers must converge");
        let ids: Vec<_> = res_a.iter().map(|(id, _)| *id).collect();
        assert!(!ids.contains(&target), "target must be tombstoned");
        assert!(ids.contains(&drag_a), "Alice's drag must survive");
        assert!(ids.contains(&drag_b), "Bob's drag must survive");
        assert_eq!(res_a.len(), 2, "exactly drag_a + drag_b remain");
    }

    /// Both peers concurrently drop the same dragged item between the same
    /// neighbors (e.g. both want it after item-1, before item-3). Per LWW:
    /// last-author wins for the move; item identity preserved.
    #[test]
    fn gesture_drop_between_concurrent_lww_no_duplicate() {
        let sk_a = sk(0xff);
        let sk_b = sk(0x01);
        let owner = sk(7);
        let i1 = item(1);
        let i2 = item(2);
        let i3 = item(3);
        let p1 = first();
        let p2 = after(&p1);
        let p3 = after(&p2);

        let mut g_owner = StateGraph::new();
        g_owner.apply_local(&owner, 1000, vec![insert("L", i1, p1.clone())]).unwrap();
        g_owner.apply_local(&owner, 1001, vec![insert("L", i2, p2.clone())]).unwrap();
        g_owner.apply_local(&owner, 1002, vec![insert("L", i3, p3.clone())]).unwrap();

        let mut g_a = StateGraph::new();
        let mut g_b = StateGraph::new();
        sync(&mut g_a, &g_owner);
        sync(&mut g_b, &g_owner);

        // Both move i2 between i1 and i3 — but they pick slightly different
        // fractional positions (different midpoints could legitimately
        // diverge if either peer's local view evolved). Both moves on same
        // item resolve via LWW.
        let mid_a = between(Some(&p1), Some(&p3));
        let mid_b = between(Some(&p1), Some(&p3));
        g_a.apply_local(&sk_a, 2000, vec![move_op("L", i2, mid_a)]).unwrap();
        g_b.apply_local(&sk_b, 2000, vec![move_op("L", i2, mid_b)]).unwrap();

        let snap_a = snapshot(&g_a);
        let snap_b = snapshot(&g_b);
        apply_all(&mut g_a, &snap_b);
        apply_all(&mut g_b, &snap_a);

        let res_a = g_a.resolve_list("L");
        let res_b = g_b.resolve_list("L");
        assert_eq!(res_a, res_b, "peers must converge");
        let count_i2 = res_a.iter().filter(|(id, _)| *id == i2).count();
        assert_eq!(count_i2, 1, "no duplicate of moved item");
        assert_eq!(res_a.len(), 3, "all three originals still present");
    }

    /// Two peers concurrently swap overlapping pairs:
    /// Alice swaps (i1, i2); Bob swaps (i2, i3). Asserts identity preserved
    /// (no item lost or duplicated) and convergence.
    #[test]
    fn gesture_swap_overlapping_pairs_preserves_identity() {
        let sk_a = sk(0xff);
        let sk_b = sk(0x01);
        let owner = sk(7);
        let i1 = item(1);
        let i2 = item(2);
        let i3 = item(3);
        let p1 = first();
        let p2 = after(&p1);
        let p3 = after(&p2);

        let mut g_owner = StateGraph::new();
        g_owner.apply_local(&owner, 1000, vec![insert("L", i1, p1.clone())]).unwrap();
        g_owner.apply_local(&owner, 1001, vec![insert("L", i2, p2.clone())]).unwrap();
        g_owner.apply_local(&owner, 1002, vec![insert("L", i3, p3.clone())]).unwrap();

        let mut g_a = StateGraph::new();
        let mut g_b = StateGraph::new();
        sync(&mut g_a, &g_owner);
        sync(&mut g_b, &g_owner);

        // Alice swaps i1 <-> i2: move i1 to p2, move i2 to p1.
        g_a.apply_local(&sk_a, 2000, vec![
            move_op("L", i1, p2.clone()),
            move_op("L", i2, p1.clone()),
        ]).unwrap();
        // Bob swaps i2 <-> i3: move i2 to p3, move i3 to p2.
        g_b.apply_local(&sk_b, 2000, vec![
            move_op("L", i2, p3.clone()),
            move_op("L", i3, p2.clone()),
        ]).unwrap();

        let snap_a = snapshot(&g_a);
        let snap_b = snapshot(&g_b);
        apply_all(&mut g_a, &snap_b);
        apply_all(&mut g_b, &snap_a);

        let res_a = g_a.resolve_list("L");
        let res_b = g_b.resolve_list("L");
        assert_eq!(res_a, res_b, "peers must converge");
        assert_eq!(res_a.len(), 3, "no item lost");
        let ids: std::collections::HashSet<_> = res_a.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&i1));
        assert!(ids.contains(&i2));
        assert!(ids.contains(&i3));
    }

    /// `dropBefore`/`dropAfter` semantic: dragged ends up adjacent to target
    /// on both peers under concurrent gestures touching different drags
    /// against the same target.
    #[test]
    fn gesture_drop_before_concurrent_distinct_drags() {
        let sk_a = sk(0xff);
        let sk_b = sk(0x01);
        let owner = sk(7);
        let target = item(1);
        let drag_a = item(0xaa);
        let drag_b = item(0xbb);
        let p_target = first();
        let p_drag_a = after(&p_target);
        let p_drag_b = after(&p_drag_a);

        let mut g_owner = StateGraph::new();
        g_owner.apply_local(&owner, 1000, vec![insert("L", target, p_target.clone())]).unwrap();
        g_owner.apply_local(&owner, 1001, vec![insert("L", drag_a, p_drag_a.clone())]).unwrap();
        g_owner.apply_local(&owner, 1002, vec![insert("L", drag_b, p_drag_b.clone())]).unwrap();

        let mut g_a = StateGraph::new();
        let mut g_b = StateGraph::new();
        sync(&mut g_a, &g_owner);
        sync(&mut g_b, &g_owner);

        // Both peers dropBefore(_, target). The new positions both fall
        // before p_target; identity preserved.
        let pos_before_target = before(&p_target);
        g_a.apply_local(&sk_a, 2000, vec![move_op("L", drag_a, pos_before_target.clone())]).unwrap();
        g_b.apply_local(&sk_b, 2000, vec![move_op("L", drag_b, pos_before_target.clone())]).unwrap();

        let snap_a = snapshot(&g_a);
        let snap_b = snapshot(&g_b);
        apply_all(&mut g_a, &snap_b);
        apply_all(&mut g_b, &snap_a);

        let res_a = g_a.resolve_list("L");
        let res_b = g_b.resolve_list("L");
        assert_eq!(res_a, res_b, "peers must converge");
        assert_eq!(res_a.len(), 3, "all items present");
        // target should be last (both drags moved before it).
        assert_eq!(res_a.last().unwrap().0, target, "target stays put");
    }
}
