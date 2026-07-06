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
//! it.  Siblings (same `after` parent) are ordered in **descending** `OpId`
//! order so higher-priority chars appear first.  A pre-order DFS traversal
//! of this forest yields the final character sequence.
//!
//! # Storage
//! The projection stores the flattened sequence directly (the classic flat
//! RGA formulation, equivalent to the forest DFS order): a chunked
//! order-statistic list over **all** items including tombstones, indexed by
//! two Fenwick trees (visible chars / visible UTF-8 bytes per chunk).
//! Integration of a new insert scans forward from its `after` anchor,
//! skipping items with larger `OpId` — the standard RGA insertion rule,
//! which visits only the local conflict window (usually zero items) instead
//! of walking the whole document.  All per-op operations are
//! O(log chunks + chunk size); nothing is O(document) on the apply path.
//!
//! Deletions are tombstones: the character is excluded from the visible
//! output but its `OpId` remains valid as a `before`/`after` anchor for
//! future insertions.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Instant;

use crate::node::SyncNode;
use crate::op::{Op, OpId, TextOp};
use crate::text_range::TextRangeAnchor;

/// Runtime mode for text projection reads and parity behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextProjectionMode {
    /// Always use replay resolver for reads.
    #[default]
    Disabled,
    /// Use projection for reads.
    Enabled,
    /// Use projection for reads and run parity checks on write updates.
    ParityCheck,
}

/// Lightweight parity mismatch diagnostic emitted by `StateGraph`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextParityMismatch {
    pub key: String,
    pub trigger_node: Option<crate::node::NodeId>,
    pub projected_len: usize,
    pub legacy_len: usize,
    pub first_projected: Option<OpId>,
    pub first_legacy: Option<OpId>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextProjectionDebugStats {
    pub metadata_entries: usize,
    pub visible_len: usize,
    pub tombstone_count: usize,
    pub tombstone_span_count: usize,
    pub tombstone_author_bucket_count: usize,
    pub pending_deletes_count: usize,
    pub child_bucket_count: usize,
    pub visible_string_capacity: usize,
    pub index_weights_len: usize,
    pub index_fenwick_len: usize,
    pub full_rebuild_count: u64,
    pub sibling_fanout_max: usize,
    pub sibling_fanout_avg_milli: u64,
    pub insert_ops_applied: u64,
    pub delete_ops_applied: u64,
    pub range_insert_ops_applied: u64,
    pub range_delete_ops_applied: u64,
    pub resolve_seq_calls: u64,
    pub resolve_string_calls: u64,
    pub resolve_range_calls: u64,
    pub invalidation_count: u64,
    pub index_update_time_ns: u64,
    pub index_rebuild_time_ns: u64,
    pub dirty_range_count: usize,
    pub dirty_span_chars: usize,
    pub dirty_range_merge_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionUpdateOp {
    Insert {
        id: OpId,
        after: Option<OpId>,
        ch: char,
    },
    Delete {
        target: OpId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
struct CompactId {
    actor_idx: u32,
    lamport: u64,
}

impl CompactId {
    fn from_op_id(id: OpId, actor_table: &mut ProjectionActorTable) -> Self {
        Self {
            actor_idx: actor_table.get_or_insert(id.author),
            lamport: id.lamport,
        }
    }

    fn from_op_id_existing(id: OpId, actor_table: &ProjectionActorTable) -> Option<Self> {
        actor_table.author_to_idx.get(&id.author).copied().map(|actor_idx| Self {
            actor_idx,
            lamport: id.lamport,
        })
    }

    fn to_op_id(self, actor_table: &ProjectionActorTable) -> Option<OpId> {
        let author = actor_table.author_for_idx(self.actor_idx)?;
        Some(OpId {
            lamport: self.lamport,
            author,
        })
    }
}

#[derive(Debug, Clone, Default)]
struct ProjectionActorTable {
    authors: Vec<[u8; 32]>,
    author_to_idx: HashMap<[u8; 32], u32>,
}

impl ProjectionActorTable {
    fn get_or_insert(&mut self, author: [u8; 32]) -> u32 {
        if let Some(idx) = self.author_to_idx.get(&author) {
            return *idx;
        }
        let idx = self.authors.len() as u32;
        self.authors.push(author);
        self.author_to_idx.insert(author, idx);
        idx
    }

    fn author_for_idx(&self, idx: u32) -> Option<[u8; 32]> {
        self.authors.get(idx as usize).copied()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.authors.len()
    }
}

/// Counted position index over per-chunk weights (Fenwick tree).
#[derive(Debug, Clone, Default)]
struct CountedPositionIndex {
    weights: Vec<u32>,
    fenwick: Vec<u32>,
}

impl CountedPositionIndex {
    #[inline]
    fn to_u32(value: usize) -> u32 {
        value.min(u32::MAX as usize) as u32
    }

    fn total_len(&self) -> usize {
        self.prefix_sum(self.weights.len())
    }

    fn rebuild_from_weights(&mut self, weights: &[usize]) {
        self.weights.clear();
        self.weights
            .extend(weights.iter().map(|&w| Self::to_u32(w)));
        self.rebuild_fenwick();
    }

    fn update_weight(&mut self, idx: usize, new_weight: usize) {
        if idx >= self.weights.len() {
            return;
        }
        let new_weight = Self::to_u32(new_weight);
        let old_weight = self.weights[idx];
        if old_weight == new_weight {
            return;
        }
        self.weights[idx] = new_weight;
        let idx1 = idx + 1;
        if new_weight > old_weight {
            self.add_u32(idx1, new_weight - old_weight);
        } else {
            self.sub_u32(idx1, old_weight - new_weight);
        }
    }

    /// Resolve 0-based offset -> `(entry_idx, offset_inside_entry)`.
    fn select_offset(&self, offset: usize) -> Option<(usize, usize)> {
        if offset >= self.total_len() {
            return None;
        }
        let entry_idx = self.lower_bound(offset + 1)?;
        let entry_start = self.prefix_sum(entry_idx);
        Some((entry_idx, offset.saturating_sub(entry_start)))
    }

    fn rebuild_fenwick(&mut self) {
        self.fenwick.clear();
        self.fenwick.resize(self.weights.len() + 1, 0);
        for i in 0..self.weights.len() {
            self.add_u32(i + 1, self.weights[i]);
        }
    }

    fn add_u32(&mut self, mut idx1: usize, delta: u32) {
        while idx1 < self.fenwick.len() {
            self.fenwick[idx1] = self.fenwick[idx1].saturating_add(delta);
            let low = idx1 & idx1.wrapping_neg();
            idx1 += low;
        }
    }

    fn sub_u32(&mut self, mut idx1: usize, delta: u32) {
        while idx1 < self.fenwick.len() {
            self.fenwick[idx1] = self.fenwick[idx1].saturating_sub(delta);
            let low = idx1 & idx1.wrapping_neg();
            idx1 += low;
        }
    }

    fn prefix_sum(&self, mut idx1: usize) -> usize {
        let mut acc = 0u64;
        idx1 = idx1.min(self.fenwick.len().saturating_sub(1));
        while idx1 > 0 {
            acc = acc.saturating_add(self.fenwick[idx1] as u64);
            idx1 &= idx1 - 1;
        }
        acc.min(usize::MAX as u64) as usize
    }

    /// Smallest 0-based index where prefix sum is at least `target`.
    fn lower_bound(&self, target: usize) -> Option<usize> {
        if target == 0 || self.weights.is_empty() {
            return None;
        }
        let total = self.prefix_sum(self.weights.len());
        if target > total {
            return None;
        }

        let mut idx = 0usize;
        let mut bit = 1usize;
        while bit < self.fenwick.len() {
            bit <<= 1;
        }
        let mut running = 0u64;
        while bit > 0 {
            let next = idx + bit;
            if next < self.fenwick.len() {
                let cand = running.saturating_add(self.fenwick[next] as u64);
                if cand < target as u64 {
                    idx = next;
                    running = cand;
                }
            }
            bit >>= 1;
        }
        Some(idx)
    }
}

/// One character in the flattened RGA sequence, tombstones included.
#[derive(Debug, Clone, Copy)]
struct Item {
    id: CompactId,
    ch: char,
    deleted: bool,
}

/// Split a chunk once it grows past this many items.
const CHUNK_SPLIT: usize = 128;

#[derive(Debug, Default)]
struct Chunk {
    items: Vec<Item>,
    visible_chars: u32,
    visible_bytes: u32,
    /// Index of this chunk in `TextProjection::order`; maintained on
    /// structural changes (chunk splits).
    order_pos: usize,
}

impl Chunk {
    fn recount(&mut self) {
        let mut chars = 0u32;
        let mut bytes = 0u32;
        for item in &self.items {
            if !item.deleted {
                chars += 1;
                bytes += item.ch.len_utf8() as u32;
            }
        }
        self.visible_chars = chars;
        self.visible_bytes = bytes;
    }
}

/// Location in the all-items sequence: `(order position, index in chunk)`.
/// `item_idx` may equal the chunk length when denoting an insertion point
/// at the end of a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Loc {
    order_pos: usize,
    item_idx: usize,
}

/// Incrementally maintained per-key text materialization.
///
/// Keeps the flat RGA item sequence (tombstones included) in fixed-size
/// chunks with Fenwick indexes for visible-char and visible-byte offsets,
/// plus an id -> chunk map for O(log n) anchor resolution.  The visible
/// string is maintained eagerly so reads are cache hits.
#[derive(Debug, Default)]
pub struct TextProjection {
    actor_table: ProjectionActorTable,
    /// Chunk slab; slots are stable, sequence order lives in `order`.
    chunks: Vec<Chunk>,
    free_slots: Vec<u32>,
    /// Chunk slots in document order.
    order: Vec<u32>,
    /// Visible chars per chunk (parallel to `order`).
    char_index: CountedPositionIndex,
    /// Visible UTF-8 bytes per chunk (parallel to `order`).
    byte_index: CountedPositionIndex,
    /// Which chunk slot an item id lives in (tombstones included).
    id_to_chunk: HashMap<CompactId, u32>,
    /// Deletes that arrived before their target insert.
    pending_deletes: HashSet<CompactId>,
    /// Inserts that arrived before their `after` anchor: parent -> children.
    orphans: HashMap<CompactId, Vec<(CompactId, char)>>,
    orphan_ids: HashSet<CompactId>,
    visible_string: String,
    visible_chars_total: usize,
    total_items: usize,
    tombstone_count: usize,
    /// Last locally-updated visible character span `[start, end)`.
    last_dirty_range: Option<(usize, usize)>,
    /// Coalesced dirty spans in visible-offset space.
    dirty_ranges: Vec<(usize, usize)>,
    dirty_range_merge_count: u64,
    full_rebuild_count: u64,
    insert_ops_applied: u64,
    delete_ops_applied: u64,
    range_insert_ops_applied: u64,
    range_delete_ops_applied: u64,
    resolve_seq_calls: Mutex<u64>,
    resolve_string_calls: Mutex<u64>,
    resolve_range_calls: Mutex<u64>,
    invalidation_count: u64,
    index_update_time_ns: u64,
    index_rebuild_time_ns: u64,
}

impl TextProjection {
    fn increment_counter(counter: &Mutex<u64>) {
        match counter.lock() {
            Ok(mut guard) => {
                *guard = guard.saturating_add(1);
            }
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                *guard = guard.saturating_add(1);
            }
        }
    }

    fn read_counter(counter: &Mutex<u64>) -> u64 {
        match counter.lock() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    fn compact_id_for(&mut self, id: OpId) -> CompactId {
        CompactId::from_op_id(id, &mut self.actor_table)
    }

    fn compact_id_existing(&self, id: OpId) -> Option<CompactId> {
        CompactId::from_op_id_existing(id, &self.actor_table)
    }

    fn compact_id_cmp(&self, left: CompactId, right: CompactId) -> Ordering {
        left.lamport.cmp(&right.lamport).then_with(|| {
            let left_author = self.actor_table.author_for_idx(left.actor_idx).unwrap_or([0; 32]);
            let right_author = self.actor_table.author_for_idx(right.actor_idx).unwrap_or([0; 32]);
            left_author.cmp(&right_author)
        })
    }

    /// `Instant::now()` panics on `wasm32-unknown-unknown`; skip timing there.
    fn projection_timing_start() -> Option<Instant> {
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Some(Instant::now())
        }
    }

    fn elapsed_ns_u64(start: Option<Instant>) -> u64 {
        let Some(start) = start else {
            return 0;
        };
        let ns = start.elapsed().as_nanos();
        ns.min(u64::MAX as u128) as u64
    }

    fn record_dirty_range(&mut self, start: usize, end: usize) {
        let mut new_start = start.min(end);
        let mut new_end = start.max(end);
        self.last_dirty_range = Some((new_start, new_end));

        if self.dirty_ranges.is_empty() {
            self.dirty_ranges.push((new_start, new_end));
            return;
        }

        // Fast path: most edits extend or touch the current tail range.
        if let Some((tail_start, tail_end)) = self.dirty_ranges.last_mut() {
            if new_start <= tail_end.saturating_add(1) {
                if new_start < *tail_start {
                    *tail_start = new_start;
                }
                if new_end > *tail_end {
                    *tail_end = new_end;
                }
                self.dirty_range_merge_count = self.dirty_range_merge_count.saturating_add(1);
                return;
            }
            if new_start >= *tail_end {
                self.dirty_ranges.push((new_start, new_end));
                return;
            }
        }

        let mut i = 0usize;
        while i < self.dirty_ranges.len() {
            let (s, e) = self.dirty_ranges[i];
            if e.saturating_add(1) < new_start {
                i += 1;
                continue;
            }
            if s > new_end.saturating_add(1) {
                break;
            }
            new_start = new_start.min(s);
            new_end = new_end.max(e);
            self.dirty_ranges.remove(i);
            self.dirty_range_merge_count = self.dirty_range_merge_count.saturating_add(1);
        }
        self.dirty_ranges.insert(i, (new_start, new_end));
    }

    // -----------------------------------------------------------------------
    // Chunk navigation
    // -----------------------------------------------------------------------

    fn chunk(&self, slot: u32) -> &Chunk {
        &self.chunks[slot as usize]
    }

    fn alloc_chunk(&mut self) -> u32 {
        if let Some(slot) = self.free_slots.pop() {
            self.chunks[slot as usize] = Chunk::default();
            slot
        } else {
            self.chunks.push(Chunk::default());
            (self.chunks.len() - 1) as u32
        }
    }

    fn loc_of_id(&self, cid: CompactId) -> Option<Loc> {
        let slot = *self.id_to_chunk.get(&cid)?;
        let chunk = self.chunk(slot);
        let item_idx = chunk.items.iter().position(|it| it.id == cid)?;
        Some(Loc {
            order_pos: chunk.order_pos,
            item_idx,
        })
    }

    /// Normalize `loc` to point at an actual item, crossing chunk boundaries.
    /// Returns `None` when `loc` is at/past the end of the sequence.
    fn norm_item_loc(&self, mut loc: Loc) -> Option<Loc> {
        loop {
            let slot = *self.order.get(loc.order_pos)?;
            if loc.item_idx < self.chunk(slot).items.len() {
                return Some(loc);
            }
            loc = Loc {
                order_pos: loc.order_pos + 1,
                item_idx: 0,
            };
        }
    }

    fn item_at(&self, loc: Loc) -> &Item {
        let slot = self.order[loc.order_pos];
        &self.chunk(slot).items[loc.item_idx]
    }

    /// End-of-sequence insertion location.
    fn end_loc(&self) -> Loc {
        let order_pos = self.order.len().saturating_sub(1);
        let item_idx = self
            .order
            .last()
            .map(|&slot| self.chunk(slot).items.len())
            .unwrap_or(0);
        Loc {
            order_pos,
            item_idx,
        }
    }

    /// Visible char offset of the (insertion) location.
    fn visible_prefix_at(&self, loc: Loc) -> usize {
        let base = self.char_index.prefix_sum(loc.order_pos);
        let slot = match self.order.get(loc.order_pos) {
            Some(&slot) => slot,
            None => return base,
        };
        let chunk = self.chunk(slot);
        let upto = loc.item_idx.min(chunk.items.len());
        base + chunk.items[..upto].iter().filter(|it| !it.deleted).count()
    }

    /// Visible byte offset of the (insertion) location.
    fn byte_prefix_at(&self, loc: Loc) -> usize {
        let base = self.byte_index.prefix_sum(loc.order_pos);
        let slot = match self.order.get(loc.order_pos) {
            Some(&slot) => slot,
            None => return base,
        };
        let chunk = self.chunk(slot);
        let upto = loc.item_idx.min(chunk.items.len());
        base + chunk.items[..upto]
            .iter()
            .filter(|it| !it.deleted)
            .map(|it| it.ch.len_utf8())
            .sum::<usize>()
    }

    /// Location of the visible item at `offset`.
    fn loc_of_visible_offset(&self, offset: usize) -> Option<Loc> {
        let (order_pos, in_chunk_visible) = self.char_index.select_offset(offset)?;
        let slot = *self.order.get(order_pos)?;
        let chunk = self.chunk(slot);
        let mut seen = 0usize;
        for (item_idx, item) in chunk.items.iter().enumerate() {
            if item.deleted {
                continue;
            }
            if seen == in_chunk_visible {
                return Some(Loc {
                    order_pos,
                    item_idx,
                });
            }
            seen += 1;
        }
        None
    }

    /// `OpId` of the visible character at `offset`, via the chunk index —
    /// O(log chunks + chunk size), no sequence materialization. Public so
    /// `StateGraph` can canonicalize `Offset` range anchors without paying
    /// an O(document) `resolve_seq` per local edit.
    pub fn id_at_visible_offset(&self, offset: usize) -> Option<OpId> {
        let loc = self.loc_of_visible_offset(offset)?;
        self.item_at(loc).id.to_op_id(&self.actor_table)
    }

    /// Number of visible (non-tombstoned) characters.
    pub fn visible_char_len(&self) -> usize {
        self.visible_chars_total
    }

    /// Visible offset of `cid`, or `None` when unknown or tombstoned.
    fn visible_offset_of_id(&self, cid: CompactId) -> Option<usize> {
        let loc = self.loc_of_id(cid)?;
        if self.item_at(loc).deleted {
            return None;
        }
        Some(self.visible_prefix_at(loc))
    }

    /// Visible byte offset for a visible char offset (== string length when
    /// `offset` is at/past the end).
    fn byte_offset_of_visible(&self, offset: usize) -> usize {
        match self.loc_of_visible_offset(offset) {
            Some(loc) => self.byte_prefix_at(loc),
            None => self.visible_string.len(),
        }
    }

    // -----------------------------------------------------------------------
    // Mutation
    // -----------------------------------------------------------------------

    /// Rebuild both Fenwick indexes from current chunk weights. O(chunks);
    /// only called on chunk-structure changes (splits), which are amortized
    /// over `CHUNK_SPLIT/2` inserts.
    fn rebuild_indexes(&mut self) {
        let started = Self::projection_timing_start();
        let char_weights: Vec<usize> = self
            .order
            .iter()
            .map(|&slot| self.chunk(slot).visible_chars as usize)
            .collect();
        let byte_weights: Vec<usize> = self
            .order
            .iter()
            .map(|&slot| self.chunk(slot).visible_bytes as usize)
            .collect();
        self.char_index.rebuild_from_weights(&char_weights);
        self.byte_index.rebuild_from_weights(&byte_weights);
        self.index_rebuild_time_ns = self
            .index_rebuild_time_ns
            .saturating_add(Self::elapsed_ns_u64(started));
    }

    fn split_chunk(&mut self, order_pos: usize) {
        let slot = self.order[order_pos];
        let mid = self.chunk(slot).items.len() / 2;
        let moved = self.chunks[slot as usize].items.split_off(mid);
        let new_slot = self.alloc_chunk();
        for item in &moved {
            self.id_to_chunk.insert(item.id, new_slot);
        }
        self.chunks[new_slot as usize].items = moved;
        self.chunks[slot as usize].recount();
        self.chunks[new_slot as usize].recount();
        self.order.insert(order_pos + 1, new_slot);
        for i in order_pos + 1..self.order.len() {
            let s = self.order[i] as usize;
            self.chunks[s].order_pos = i;
        }
        self.rebuild_indexes();
    }

    /// Insert a brand-new item using RGA integration: scan forward from the
    /// position right after `parent` (or the sequence start for root
    /// inserts), skipping items with larger `OpId`.  The caller guarantees
    /// `parent` (when `Some`) is present.
    fn integrate(&mut self, cid: CompactId, parent: Option<CompactId>, ch: char) {
        if self.order.is_empty() {
            let slot = self.alloc_chunk();
            self.chunks[slot as usize].order_pos = 0;
            self.order.push(slot);
            self.rebuild_indexes();
        }

        let start = match parent {
            None => Loc {
                order_pos: 0,
                item_idx: 0,
            },
            Some(p) => {
                let ploc = match self.loc_of_id(p) {
                    Some(loc) => loc,
                    // Caller checked presence; treat a race as end-insert.
                    None => self.end_loc(),
                };
                Loc {
                    order_pos: ploc.order_pos,
                    item_idx: ploc.item_idx + 1,
                }
            }
        };

        // RGA scan: skip over items with higher priority than the new one.
        let mut loc = start;
        loop {
            match self.norm_item_loc(loc) {
                None => {
                    loc = self.end_loc();
                    break;
                }
                Some(norm) => {
                    let existing = self.item_at(norm).id;
                    if self.compact_id_cmp(existing, cid) == Ordering::Greater {
                        loc = Loc {
                            order_pos: norm.order_pos,
                            item_idx: norm.item_idx + 1,
                        };
                    } else {
                        loc = norm;
                        break;
                    }
                }
            }
        }
        // Clamp the insertion point inside its chunk.
        let slot = self.order[loc.order_pos];
        let item_idx = loc.item_idx.min(self.chunk(slot).items.len());
        let loc = Loc {
            order_pos: loc.order_pos,
            item_idx,
        };

        let deleted = self.pending_deletes.remove(&cid);
        let (vis_idx, byte_off) = if deleted {
            (0, 0)
        } else {
            (self.visible_prefix_at(loc), self.byte_prefix_at(loc))
        };

        self.chunks[slot as usize].items.insert(
            loc.item_idx,
            Item {
                id: cid,
                ch,
                deleted,
            },
        );
        self.id_to_chunk.insert(cid, slot);
        self.total_items += 1;

        if deleted {
            self.tombstone_count += 1;
        } else {
            let started = Self::projection_timing_start();
            let chunk = &mut self.chunks[slot as usize];
            chunk.visible_chars += 1;
            chunk.visible_bytes += ch.len_utf8() as u32;
            let (chars, bytes) = (chunk.visible_chars as usize, chunk.visible_bytes as usize);
            self.char_index.update_weight(loc.order_pos, chars);
            self.byte_index.update_weight(loc.order_pos, bytes);
            self.index_update_time_ns = self
                .index_update_time_ns
                .saturating_add(Self::elapsed_ns_u64(started));
            self.visible_chars_total += 1;
            if byte_off == self.visible_string.len() {
                self.visible_string.push(ch);
            } else {
                self.visible_string.insert(byte_off, ch);
            }
            self.record_dirty_range(vis_idx, vis_idx + 1);
        }

        if self.chunks[slot as usize].items.len() > CHUNK_SPLIT {
            self.split_chunk(loc.order_pos);
        }
    }

    /// Integrate any buffered orphan inserts now that `parent` exists.
    fn drain_orphans_of(&mut self, parent: CompactId) {
        if self.orphans.is_empty() {
            return;
        }
        let mut stack = vec![parent];
        while let Some(p) = stack.pop() {
            let Some(children) = self.orphans.remove(&p) else {
                continue;
            };
            for (child, ch) in children {
                self.orphan_ids.remove(&child);
                self.integrate(child, Some(p), ch);
                stack.push(child);
            }
        }
    }

    fn delete_known(&mut self, cid: CompactId) {
        let Some(loc) = self.loc_of_id(cid) else {
            self.pending_deletes.insert(cid);
            return;
        };
        if self.item_at(loc).deleted {
            return; // idempotent
        }
        self.delete_ops_applied = self.delete_ops_applied.saturating_add(1);
        self.tombstone_count += 1;
        let vis_idx = self.visible_prefix_at(loc);
        let byte_off = self.byte_prefix_at(loc);
        let slot = self.order[loc.order_pos];
        let ch_len = {
            let chunk = &mut self.chunks[slot as usize];
            let item = &mut chunk.items[loc.item_idx];
            item.deleted = true;
            let ch_len = item.ch.len_utf8();
            chunk.visible_chars -= 1;
            chunk.visible_bytes -= ch_len as u32;
            ch_len
        };
        let started = Self::projection_timing_start();
        let (chars, bytes) = {
            let chunk = self.chunk(slot);
            (chunk.visible_chars as usize, chunk.visible_bytes as usize)
        };
        self.char_index.update_weight(loc.order_pos, chars);
        self.byte_index.update_weight(loc.order_pos, bytes);
        self.index_update_time_ns = self
            .index_update_time_ns
            .saturating_add(Self::elapsed_ns_u64(started));
        self.visible_chars_total -= 1;
        self.visible_string
            .replace_range(byte_off..byte_off + ch_len, "");
        self.record_dirty_range(vis_idx, vis_idx);
    }

    // -----------------------------------------------------------------------
    // Public apply API
    // -----------------------------------------------------------------------

    /// Build a fresh projection from the current oplog view for one key.
    pub fn from_nodes(nodes: &[&SyncNode], key: &str) -> Self {
        let mut projection = Self::default();
        projection.full_rebuild_count = 1;
        for node in nodes {
            projection.apply_node(node, key);
        }
        projection
    }

    /// Apply text ops from one node for `key`.
    pub fn apply_node(&mut self, node: &SyncNode, key: &str) {
        let tx = &node.transaction;
        for op in &tx.ops {
            match op {
                Op::Text(TextOp::Insert {
                    key: op_key,
                    after,
                    ch,
                }) if op_key == key => {
                    self.apply_insert(
                        OpId {
                            lamport: tx.lamport,
                            author: tx.author,
                        },
                        *after,
                        *ch,
                    );
                }
                Op::Text(TextOp::Delete {
                    key: op_key,
                    target,
                }) if op_key == key => {
                    self.apply_delete(*target);
                }
                Op::Text(TextOp::InsertRange {
                    key: op_key,
                    anchor,
                    text,
                }) if op_key == key => {
                    self.apply_range_insert(tx.lamport, tx.author, *anchor, text);
                }
                Op::Text(TextOp::DeleteRange {
                    key: op_key,
                    anchor,
                    len_chars,
                }) if op_key == key => {
                    self.apply_range_delete(*anchor, *len_chars);
                }
                _ => {}
            }
        }
    }

    pub(crate) fn apply_updates_coalesced(&mut self, updates: &[ProjectionUpdateOp]) {
        for update in updates {
            match update {
                ProjectionUpdateOp::Insert { id, after, ch } => {
                    self.apply_insert(*id, *after, *ch);
                }
                ProjectionUpdateOp::Delete { target } => {
                    self.apply_delete(*target);
                }
            }
        }
    }

    pub(crate) fn apply_insert(&mut self, id: OpId, after: Option<OpId>, ch: char) {
        let cid = self.compact_id_for(id);
        if self.id_to_chunk.contains_key(&cid) || self.orphan_ids.contains(&cid) {
            return;
        }
        self.insert_ops_applied = self.insert_ops_applied.saturating_add(1);
        let after_cid = after.map(|p| self.compact_id_for(p));
        if let Some(p) = after_cid {
            if !self.id_to_chunk.contains_key(&p) {
                // Anchor not applied yet (out-of-causal-order delivery):
                // buffer until the parent arrives.
                self.orphans.entry(p).or_default().push((cid, ch));
                self.orphan_ids.insert(cid);
                return;
            }
        }
        self.integrate(cid, after_cid, ch);
        self.drain_orphans_of(cid);
    }

    pub(crate) fn apply_delete(&mut self, target: OpId) {
        let Some(cid) = self.compact_id_existing(target) else {
            let cid = self.compact_id_for(target);
            self.pending_deletes.insert(cid);
            return;
        };
        if !self.id_to_chunk.contains_key(&cid) {
            // Unknown or still-orphaned insert: tombstone at birth.
            self.pending_deletes.insert(cid);
            return;
        }
        self.delete_known(cid);
    }

    fn apply_range_insert(
        &mut self,
        base_lamport: u64,
        author: [u8; 32],
        anchor: TextRangeAnchor,
        text: &str,
    ) {
        if text.is_empty() {
            return;
        }
        self.range_insert_ops_applied = self.range_insert_ops_applied.saturating_add(1);

        let mut insert_after = self.resolve_anchor_after_visible(anchor);
        for (offset, ch) in text.chars().enumerate() {
            let id = OpId {
                lamport: base_lamport.saturating_add(offset as u64),
                author,
            };
            self.apply_insert(id, insert_after, ch);
            insert_after = Some(id);
        }
    }

    fn apply_range_delete(&mut self, anchor: TextRangeAnchor, len_chars: usize) {
        if len_chars == 0 {
            return;
        }
        self.range_delete_ops_applied = self.range_delete_ops_applied.saturating_add(1);

        let start = self.resolve_anchor_index(anchor);
        let count = len_chars.min(self.visible_chars_total.saturating_sub(start));
        for _ in 0..count {
            let Some(loc) = self.loc_of_visible_offset(start) else {
                break;
            };
            let cid = self.item_at(loc).id;
            self.delete_known(cid);
        }
    }

    /// `OpId` of the visible char the anchor points *after* (`None` = start).
    fn resolve_anchor_after_visible(&self, anchor: TextRangeAnchor) -> Option<OpId> {
        let idx = self.resolve_anchor_index(anchor);
        if idx == 0 {
            None
        } else {
            self.id_at_visible_offset(idx - 1)
        }
    }

    /// Anchor -> 0-based visible index (insertion point semantics).
    fn resolve_anchor_index(&self, anchor: TextRangeAnchor) -> usize {
        match anchor {
            TextRangeAnchor::Start => 0,
            TextRangeAnchor::End => self.visible_chars_total,
            TextRangeAnchor::Offset(i) => i.min(self.visible_chars_total),
            TextRangeAnchor::After(id) => {
                let Some(cid) = CompactId::from_op_id_existing(id, &self.actor_table) else {
                    return self.visible_chars_total;
                };
                match self.visible_offset_of_id(cid) {
                    Some(offset) => offset + 1,
                    None => self.visible_chars_total,
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Reads
    // -----------------------------------------------------------------------

    pub fn resolve_seq(&self) -> Vec<(OpId, char)> {
        Self::increment_counter(&self.resolve_seq_calls);
        self.build_visible_seq_pairs()
    }

    pub fn resolve_string(&self) -> String {
        Self::increment_counter(&self.resolve_string_calls);
        self.visible_string.clone()
    }

    pub fn resolve_seq_cached(&self) -> Vec<(OpId, char)> {
        self.build_visible_seq_pairs()
    }

    pub fn resolve_string_cached(&self) -> &str {
        &self.visible_string
    }

    pub fn resolve_string_range(&self, start: usize, len: usize) -> String {
        Self::increment_counter(&self.resolve_range_calls);
        if len == 0 || start >= self.visible_chars_total {
            return String::new();
        }
        let byte_start = self.byte_offset_of_visible(start);
        let byte_end = self.byte_offset_of_visible(start.saturating_add(len));
        self.visible_string[byte_start..byte_end].to_string()
    }

    pub fn last_dirty_range(&self) -> Option<(usize, usize)> {
        self.last_dirty_range
    }

    pub(crate) fn anchor_for_offset(&self, offset: usize) -> TextRangeAnchor {
        if offset == 0 {
            return TextRangeAnchor::Start;
        }
        if offset >= self.visible_chars_total {
            return TextRangeAnchor::End;
        }
        match self.id_at_visible_offset(offset.saturating_sub(1)) {
            Some(id) => TextRangeAnchor::After(id),
            None => TextRangeAnchor::Start,
        }
    }

    pub(crate) fn offset_for_anchor(&self, anchor: TextRangeAnchor) -> usize {
        self.resolve_anchor_index(anchor)
    }

    fn build_visible_seq_pairs(&self) -> Vec<(OpId, char)> {
        let mut out = Vec::with_capacity(self.visible_chars_total);
        for &slot in &self.order {
            for item in &self.chunk(slot).items {
                if item.deleted {
                    continue;
                }
                if let Some(id) = item.id.to_op_id(&self.actor_table) {
                    out.push((id, item.ch));
                }
            }
        }
        out
    }

    // -----------------------------------------------------------------------
    // Diagnostics / storage management
    // -----------------------------------------------------------------------

    pub fn debug_stats(&self) -> TextProjectionDebugStats {
        let (span_count, author_buckets) = self.tombstone_span_stats();
        let dirty_span_chars = self
            .dirty_ranges
            .iter()
            .map(|(s, e)| e.saturating_sub(*s))
            .sum();
        TextProjectionDebugStats {
            metadata_entries: self.total_items + self.orphan_ids.len(),
            visible_len: self.visible_chars_total,
            tombstone_count: self.tombstone_count,
            tombstone_span_count: span_count,
            tombstone_author_bucket_count: author_buckets,
            pending_deletes_count: self.pending_deletes.len(),
            child_bucket_count: self.orphans.len(),
            visible_string_capacity: self.visible_string.capacity(),
            index_weights_len: self.char_index.weights.len(),
            index_fenwick_len: self.char_index.fenwick.len(),
            full_rebuild_count: self.full_rebuild_count,
            sibling_fanout_max: 0,
            sibling_fanout_avg_milli: 0,
            insert_ops_applied: self.insert_ops_applied,
            delete_ops_applied: self.delete_ops_applied,
            range_insert_ops_applied: self.range_insert_ops_applied,
            range_delete_ops_applied: self.range_delete_ops_applied,
            resolve_seq_calls: Self::read_counter(&self.resolve_seq_calls),
            resolve_string_calls: Self::read_counter(&self.resolve_string_calls),
            resolve_range_calls: Self::read_counter(&self.resolve_range_calls),
            invalidation_count: self.invalidation_count,
            index_update_time_ns: self.index_update_time_ns,
            index_rebuild_time_ns: self.index_rebuild_time_ns,
            dirty_range_count: self.dirty_ranges.len(),
            dirty_span_chars,
            dirty_range_merge_count: self.dirty_range_merge_count,
        }
    }

    /// Tombstone run-length stats, computed on demand (diagnostics only —
    /// not on the apply path).
    fn tombstone_span_stats(&self) -> (usize, usize) {
        if self.tombstone_count == 0 {
            return (0, 0);
        }
        let mut per_actor: HashMap<u32, Vec<u64>> = HashMap::new();
        for &slot in &self.order {
            for item in &self.chunk(slot).items {
                if item.deleted {
                    per_actor
                        .entry(item.id.actor_idx)
                        .or_default()
                        .push(item.id.lamport);
                }
            }
        }
        let buckets = per_actor.len();
        let mut spans = 0usize;
        for lamports in per_actor.values_mut() {
            lamports.sort_unstable();
            let mut prev: Option<u64> = None;
            for &l in lamports.iter() {
                if prev != Some(l.wrapping_sub(1)) && prev != Some(l) {
                    spans += 1;
                }
                prev = Some(l);
            }
        }
        (spans, buckets)
    }

    /// Compact resident projection storage for cold keys by shrinking
    /// over-allocated buffers/maps while keeping semantics unchanged.
    pub fn compact_cold_storage(&mut self) -> bool {
        let capacity_sum = |p: &Self| {
            p.visible_string
                .capacity()
                .saturating_add(p.order.capacity())
                .saturating_add(p.chunks.capacity())
                .saturating_add(p.id_to_chunk.capacity())
                .saturating_add(p.char_index.weights.capacity())
                .saturating_add(p.char_index.fenwick.capacity())
                .saturating_add(p.byte_index.weights.capacity())
                .saturating_add(p.byte_index.fenwick.capacity())
        };
        let before = capacity_sum(self);

        self.visible_string.shrink_to_fit();
        self.order.shrink_to_fit();
        self.free_slots.shrink_to_fit();
        self.dirty_ranges.shrink_to_fit();
        self.id_to_chunk.shrink_to_fit();
        self.pending_deletes.shrink_to_fit();
        self.orphans.shrink_to_fit();
        self.orphan_ids.shrink_to_fit();
        for chunk in &mut self.chunks {
            chunk.items.shrink_to_fit();
        }
        self.chunks.shrink_to_fit();
        self.char_index.weights.shrink_to_fit();
        self.char_index.fenwick.shrink_to_fit();
        self.byte_index.weights.shrink_to_fit();
        self.byte_index.fenwick.shrink_to_fit();

        let after = capacity_sum(self);
        after < before
    }
}

/// Resolve the visible RGA character sequence for `key`.
///
/// Returns `(OpId, char)` pairs in sequence order, tombstoned characters
/// excluded.  The `OpId` is the stable identity of each character and is
/// used by the bridge to map cursor positions to RGA positions.
pub fn resolve_text_seq(nodes: &[&SyncNode], key: &str) -> Vec<(OpId, char)> {
    let mut ordered: Vec<&SyncNode> = nodes.to_vec();
    ordered.sort_unstable_by_key(|n| (n.transaction.lamport, n.transaction.author, n.id));

    let mut projection = TextProjection::default();
    for node in ordered {
        projection.apply_node(node, key);
    }
    projection.resolve_seq()
}

/// Resolve the RGA text for `key` as a plain UTF-8 `String`.
pub fn resolve_text(nodes: &[&SyncNode], key: &str) -> String {
    resolve_text_seq(nodes, key)
        .into_iter()
        .map(|(_, ch)| ch)
        .collect()
}

/// Replay visible RGA state for `key` using only DAG nodes with
/// `transaction.lamport <= max_lamport` (inclusive).
///
/// Tombstones from deletes in the included window are honored. This is the
/// authoritative history playback path — not a glyph filter on the live view.
pub fn resolve_text_seq_upto_lamport(
    nodes: &[&SyncNode],
    key: &str,
    max_lamport: u64,
) -> Vec<(OpId, char)> {
    let filtered: Vec<&SyncNode> = nodes
        .iter()
        .copied()
        .filter(|node| node.transaction.lamport <= max_lamport)
        .collect();
    resolve_text_seq(&filtered, key)
}

/// Plain UTF-8 text at lamport `max_lamport` (see [`resolve_text_seq_upto_lamport`]).
pub fn resolve_text_upto_lamport(nodes: &[&SyncNode], key: &str, max_lamport: u64) -> String {
    resolve_text_seq_upto_lamport(nodes, key, max_lamport)
        .into_iter()
        .map(|(_, ch)| ch)
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StateGraph, op::TextOp};
    use crate::text_range::TextRangeAnchor;
    use ed25519_dalek::SigningKey;

    fn sk(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    /// Compute the `OpId` that `graph.apply_local(key, wall_ms, [op])` will
    /// assign, without actually inserting anything.  Mirrors the formula
    /// `graph.lamport() + 1`.
    fn next_op_id(graph: &StateGraph, key: &SigningKey, _wall_ms: u64) -> OpId {
        let lamport = graph.lamport() + 1;
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
    fn text_replay_upto_lamport_excludes_later_nodes_and_honors_deletes() {
        let sk = sk(1);
        let mut g = StateGraph::new();

        let id_a = next_op_id(&g, &sk, 1000);
        g.apply_local(
            &sk,
            1000,
            vec![Op::Text(TextOp::Insert {
                key: "doc".into(),
                after: None,
                ch: 'a',
            })],
        )
        .unwrap();

        let id_b = next_op_id(&g, &sk, 1001);
        g.apply_local(
            &sk,
            1001,
            vec![Op::Text(TextOp::Insert {
                key: "doc".into(),
                after: Some(id_a),
                ch: 'b',
            })],
        )
        .unwrap();

        let owned = g.all_nodes();
        let nodes: Vec<&SyncNode> = owned.iter().collect();
        assert_eq!(resolve_text_upto_lamport(&nodes, "doc", id_a.lamport), "a");
        assert_eq!(resolve_text_upto_lamport(&nodes, "doc", id_b.lamport), "ab");

        g.apply_local(
            &sk,
            1002,
            vec![Op::Text(TextOp::Delete {
                key: "doc".into(),
                target: id_b,
            })],
        )
        .unwrap();

        let owned = g.all_nodes();
        let nodes: Vec<&SyncNode> = owned.iter().collect();
        let delete_lamport = g.lamport();
        assert_eq!(resolve_text_upto_lamport(&nodes, "doc", delete_lamport), "a");
        assert_eq!(g.resolve_text("doc"), "a");
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

    #[test]
    fn text_insert_range_is_lowered_deterministically() {
        let sk = sk(7);
        let mut g = StateGraph::new();

        g.apply_local(
            &sk,
            1000,
            vec![Op::Text(TextOp::InsertRange {
                key: "doc".into(),
                anchor: TextRangeAnchor::Start,
                text: "hello".into(),
            })],
        )
        .unwrap();

        assert_eq!(g.resolve_text("doc"), "hello");
        assert_eq!(g.resolve_text_seq("doc").len(), 5);
    }

    #[test]
    fn text_delete_range_is_lowered_deterministically() {
        let sk = sk(8);
        let mut g = StateGraph::new();

        g.apply_local(
            &sk,
            1000,
            vec![Op::Text(TextOp::InsertRange {
                key: "doc".into(),
                anchor: TextRangeAnchor::Start,
                text: "abcdef".into(),
            })],
        )
        .unwrap();

        g.apply_local(
            &sk,
            1001,
            vec![Op::Text(TextOp::DeleteRange {
                key: "doc".into(),
                anchor: TextRangeAnchor::Offset(2),
                len_chars: 2,
            })],
        )
        .unwrap();

        assert_eq!(g.resolve_text("doc"), "abef");
    }

    #[test]
    fn text_projection_mid_insert_and_delete_keep_string_consistent() {
        let sk = sk(9);
        let mut projection = TextProjection::default();

        let id_a = OpId { lamport: 1, author: sk.verifying_key().to_bytes() };
        let id_c = OpId { lamport: 2, author: sk.verifying_key().to_bytes() };
        let id_b = OpId { lamport: 3, author: sk.verifying_key().to_bytes() };

        projection.apply_insert(id_a, None, 'a');
        projection.apply_insert(id_c, Some(id_a), 'c');
        assert_eq!(projection.resolve_string(), "ac");

        projection.apply_insert(id_b, Some(id_a), 'b');
        assert_eq!(projection.resolve_string(), "abc");

        projection.apply_delete(id_b);
        assert_eq!(projection.resolve_string(), "ac");
        let seq: String = projection.resolve_seq().into_iter().map(|(_, ch)| ch).collect();
        assert_eq!(seq, "ac");
    }

    #[test]
    fn text_projection_run_helpers_preserve_mid_insert_ordering() {
        let sk = sk(10);
        let mut projection = TextProjection::default();
        let author = sk.verifying_key().to_bytes();

        let id_h = OpId { lamport: 10, author };
        let id_i = OpId { lamport: 11, author };
        let id_ex = OpId { lamport: 12, author };

        projection.apply_insert(id_h, None, 'h');
        projection.apply_insert(id_i, Some(id_h), 'i');
        projection.apply_insert(id_ex, Some(id_h), '!');

        assert_eq!(projection.resolve_string(), "h!i");
        let seq: String = projection.resolve_seq().into_iter().map(|(_, ch)| ch).collect();
        assert_eq!(seq, "h!i");
    }

    #[test]
    fn text_projection_actor_table_indirection_is_deduplicated() {
        let sk_a = sk(11);
        let sk_b = sk(12);
        let mut projection = TextProjection::default();

        let id_a1 = OpId { lamport: 1, author: sk_a.verifying_key().to_bytes() };
        let id_a2 = OpId { lamport: 2, author: sk_a.verifying_key().to_bytes() };
        let id_b1 = OpId { lamport: 1, author: sk_b.verifying_key().to_bytes() };

        projection.apply_insert(id_a1, None, 'a');
        projection.apply_insert(id_a2, Some(id_a1), 'b');
        projection.apply_insert(id_b1, Some(id_a2), 'x');

        assert_eq!(projection.resolve_string(), "abx");
        assert_eq!(projection.actor_table.len(), 2, "authors should be interned once");

        // Validate id derivation via the actor table remains correct.
        let derived = projection
            .id_at_visible_offset(0)
            .expect("first visible id should derive");
        assert_eq!(derived.author, id_a1.author);
        assert_eq!(derived.lamport, id_a1.lamport);
    }

    #[test]
    fn text_projection_tombstone_spans_compress_contiguous_deletes() {
        let sk = sk(13);
        let mut projection = TextProjection::default();
        let author = sk.verifying_key().to_bytes();

        let id1 = OpId { lamport: 100, author };
        let id2 = OpId { lamport: 101, author };
        let id3 = OpId { lamport: 102, author };

        projection.apply_insert(id1, None, 'a');
        projection.apply_insert(id2, Some(id1), 'b');
        projection.apply_insert(id3, Some(id2), 'c');
        assert_eq!(projection.resolve_string(), "abc");

        projection.apply_delete(id1);
        projection.apply_delete(id2);
        projection.apply_delete(id3);

        let stats = projection.debug_stats();
        assert_eq!(stats.tombstone_count, 3);
        assert_eq!(stats.tombstone_span_count, 1, "contiguous lamports should merge into one tombstone span");
        assert_eq!(projection.resolve_string(), "");
    }

    #[test]
    fn text_projection_anchor_offset_roundtrip_under_split_merge_churn() {
        let sk = sk(14);
        let mut projection = TextProjection::default();
        let author = sk.verifying_key().to_bytes();

        // Build baseline visible sequence.
        let id_a = OpId { lamport: 1, author };
        let id_b = OpId { lamport: 2, author };
        let id_c = OpId { lamport: 3, author };
        projection.apply_insert(id_a, None, 'a');
        projection.apply_insert(id_b, Some(id_a), 'b');
        projection.apply_insert(id_c, Some(id_b), 'c');

        // Force churn around the middle and tail.
        let id_x = OpId { lamport: 4, author };
        let id_y = OpId { lamport: 5, author };
        let id_b2 = OpId { lamport: 6, author };
        projection.apply_insert(id_x, Some(id_a), 'x'); // a x b c
        projection.apply_insert(id_y, Some(id_c), 'y'); // a x b c y
        projection.apply_delete(id_b); // a x c y
        projection.apply_delete(id_x); // a c y
        projection.apply_insert(id_b2, Some(id_a), 'b'); // a b c y

        let stats = projection.debug_stats();
        let visible_len = stats.visible_len;
        assert_eq!(projection.resolve_string(), "abcy");

        // Verify offset <-> anchor roundtrip for all visible boundaries.
        for offset in 0..=visible_len {
            let anchor = projection.anchor_for_offset(offset);
            let roundtrip = projection.offset_for_anchor(anchor);
            assert_eq!(
                roundtrip, offset,
                "offset/anchor roundtrip mismatch at offset {}",
                offset
            );
        }
    }

    #[test]
    fn text_projection_tombstone_accounting_is_exact_and_idempotent() {
        let sk = sk(15);
        let mut projection = TextProjection::default();
        let author = sk.verifying_key().to_bytes();

        let id1 = OpId { lamport: 10, author };
        let id2 = OpId { lamport: 11, author };
        let id3 = OpId { lamport: 12, author };
        let id4 = OpId { lamport: 13, author };

        projection.apply_insert(id1, None, 'w');
        projection.apply_insert(id2, Some(id1), 'x');
        projection.apply_insert(id3, Some(id2), 'y');
        projection.apply_insert(id4, Some(id3), 'z');
        assert_eq!(projection.resolve_string(), "wxyz");

        projection.apply_delete(id2);
        projection.apply_delete(id3);
        projection.apply_delete(id2); // duplicate delete must not double count

        let stats = projection.debug_stats();
        assert_eq!(stats.tombstone_count, 2, "expected exactly two tombstoned ids");
        assert_eq!(projection.resolve_string(), "wz");

        let seq: Vec<OpId> = projection.resolve_seq().into_iter().map(|(id, _)| id).collect();
        assert!(!seq.contains(&id2), "tombstoned id2 must be absent from visible seq");
        assert!(!seq.contains(&id3), "tombstoned id3 must be absent from visible seq");
    }

    #[test]
    fn text_projection_orphan_insert_integrates_when_parent_arrives() {
        let mut projection = TextProjection::default();
        let author = sk(16).verifying_key().to_bytes();

        let id_a = OpId { lamport: 1, author };
        let id_b = OpId { lamport: 2, author };
        let id_c = OpId { lamport: 3, author };

        // Children arrive before their parents (out-of-causal-order).
        projection.apply_insert(id_c, Some(id_b), 'c');
        projection.apply_insert(id_b, Some(id_a), 'b');
        assert_eq!(projection.resolve_string(), "", "orphans stay invisible");

        projection.apply_insert(id_a, None, 'a');
        assert_eq!(projection.resolve_string(), "abc", "orphan chain integrates recursively");
        assert_eq!(projection.debug_stats().child_bucket_count, 0);
    }

    #[test]
    fn text_projection_delete_before_insert_tombstones_at_birth() {
        let mut projection = TextProjection::default();
        let author = sk(17).verifying_key().to_bytes();

        let id_a = OpId { lamport: 1, author };
        let id_b = OpId { lamport: 2, author };

        projection.apply_delete(id_b); // delete arrives first
        projection.apply_insert(id_a, None, 'a');
        projection.apply_insert(id_b, Some(id_a), 'b');

        assert_eq!(projection.resolve_string(), "a");
        let stats = projection.debug_stats();
        assert_eq!(stats.tombstone_count, 1);
        assert_eq!(stats.pending_deletes_count, 0);
    }

    #[test]
    fn text_projection_multibyte_chars_keep_byte_offsets_correct() {
        let mut projection = TextProjection::default();
        let author = sk(18).verifying_key().to_bytes();

        let id1 = OpId { lamport: 1, author };
        let id2 = OpId { lamport: 2, author };
        let id3 = OpId { lamport: 3, author };
        let id4 = OpId { lamport: 4, author };

        projection.apply_insert(id1, None, 'é');
        projection.apply_insert(id2, Some(id1), '漢');
        projection.apply_insert(id3, Some(id2), 'z');
        assert_eq!(projection.resolve_string(), "é漢z");

        // Insert between the multibyte chars.
        projection.apply_insert(id4, Some(id1), '🎉');
        assert_eq!(projection.resolve_string(), "é🎉漢z");

        projection.apply_delete(id2);
        assert_eq!(projection.resolve_string(), "é🎉z");
        assert_eq!(projection.resolve_string_range(1, 2), "🎉z");
    }

    #[test]
    fn text_projection_chunk_splits_preserve_order_and_offsets() {
        let mut projection = TextProjection::default();
        let author = sk(19).verifying_key().to_bytes();

        // Enough sequential appends to force multiple chunk splits.
        let n = CHUNK_SPLIT * 4 + 17;
        let mut prev: Option<OpId> = None;
        let mut expected = String::new();
        for i in 0..n {
            let id = OpId { lamport: (i + 1) as u64, author };
            let ch = char::from(b'a' + (i % 26) as u8);
            projection.apply_insert(id, prev, ch);
            expected.push(ch);
            prev = Some(id);
        }
        assert_eq!(projection.resolve_string(), expected);
        assert!(projection.debug_stats().index_weights_len > 1, "should have split into chunks");

        // Mid-document insert after a split still lands correctly.
        let mid_parent = OpId { lamport: (CHUNK_SPLIT * 2) as u64, author };
        let id_mid = OpId { lamport: (n + 1) as u64, author };
        projection.apply_insert(id_mid, Some(mid_parent), 'Q');
        let mut expected2 = expected.clone();
        expected2.insert(CHUNK_SPLIT * 2, 'Q');
        assert_eq!(projection.resolve_string(), expected2);

        // Range read across chunk boundaries.
        assert_eq!(
            projection.resolve_string_range(CHUNK_SPLIT - 3, 7),
            expected2[CHUNK_SPLIT - 3..CHUNK_SPLIT + 4].to_string()
        );
    }
}
