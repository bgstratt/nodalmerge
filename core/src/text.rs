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

#[derive(Debug, Clone)]
struct ProjectionEntry {
    ch: char,
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
struct TombstoneStore {
    deleted_ids: HashSet<CompactId>,
    spans_by_actor: HashMap<u32, Vec<(u64, u64)>>,
}

impl TombstoneStore {
    fn contains(&self, id: CompactId) -> bool {
        self.deleted_ids.contains(&id)
    }

    fn count(&self) -> usize {
        self.deleted_ids.len()
    }

    fn span_count(&self) -> usize {
        self.spans_by_actor.values().map(Vec::len).sum()
    }

    fn author_bucket_count(&self) -> usize {
        self.spans_by_actor.len()
    }

    fn mark_deleted(&mut self, id: CompactId) -> bool {
        if !self.deleted_ids.insert(id) {
            return false;
        }
        self.insert_span(id.actor_idx, id.lamport, id.lamport);
        true
    }

    fn insert_span(&mut self, actor_idx: u32, start: u64, end: u64) {
        let spans = self.spans_by_actor.entry(actor_idx).or_default();
        let mut new_start = start.min(end);
        let mut new_end = start.max(end);

        let mut i = 0usize;
        while i < spans.len() {
            let (s, e) = spans[i];
            if e.saturating_add(1) < new_start {
                i += 1;
                continue;
            }
            if s > new_end.saturating_add(1) {
                break;
            }
            new_start = new_start.min(s);
            new_end = new_end.max(e);
            spans.remove(i);
        }
        spans.insert(i, (new_start, new_end));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct TextRun {
    actor_idx: u32,
    seq_start: u64,
    len_chars: usize,
    text: String,
}

impl TextRun {
    fn from_single(id: OpId, actor_idx: u32, ch: char) -> Self {
        let mut text = String::new();
        text.push(ch);
        Self {
            actor_idx,
            seq_start: id.lamport,
            len_chars: 1,
            text,
        }
    }

    fn len_chars(&self) -> usize {
        self.len_chars
    }

    fn is_empty(&self) -> bool {
        self.len_chars == 0
    }

    fn can_append(&self, id: OpId, actor_idx: u32) -> bool {
        actor_idx == self.actor_idx && id.lamport == self.seq_start.saturating_add(self.len_chars as u64)
    }

    fn can_prepend(&self, id: OpId, actor_idx: u32) -> bool {
        actor_idx == self.actor_idx && id.lamport.saturating_add(1) == self.seq_start
    }

    fn append_char(&mut self, ch: char) {
        self.text.push(ch);
        self.len_chars = self.len_chars.saturating_add(1);
    }

    fn prepend_char(&mut self, ch: char) {
        self.text.insert(0, ch);
        self.seq_start = self.seq_start.saturating_sub(1);
        self.len_chars = self.len_chars.saturating_add(1);
    }

    fn remove_at(&mut self, offset: usize) {
        if offset >= self.len_chars {
            return;
        }
        let start = char_to_byte_offset(&self.text, offset);
        let end = char_to_byte_offset(&self.text, offset + 1);
        if start < end && end <= self.text.len() {
            self.text.replace_range(start..end, "");
        }
        if offset == 0 {
            self.seq_start = self.seq_start.saturating_add(1);
        }
        self.len_chars = self.len_chars.saturating_sub(1);
    }

    fn split_at(&mut self, offset: usize) -> Self {
        let split = offset.min(self.len_chars);
        let byte_idx = char_to_byte_offset(&self.text, split);
        let right_text = self.text.split_off(byte_idx);
        let right = Self {
            actor_idx: self.actor_idx,
            seq_start: self.seq_start.saturating_add(split as u64),
            len_chars: self.len_chars.saturating_sub(split),
            text: right_text,
        };
        self.len_chars = split;
        right
    }

    fn last_lamport(&self) -> u64 {
        self.seq_start
            .saturating_add(self.len_chars.saturating_sub(1) as u64)
    }

    fn append_run(&mut self, right: TextRun) {
        debug_assert!(self.actor_idx == right.actor_idx);
        debug_assert!(self.last_lamport().saturating_add(1) == right.seq_start);
        self.len_chars = self.len_chars.saturating_add(right.len_chars);
        self.text.push_str(&right.text);
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

impl TextRun {
    fn id_at(&self, actor_table: &ProjectionActorTable, offset: usize) -> Option<OpId> {
        if offset >= self.len_chars {
            return None;
        }
        let author = actor_table.author_for_idx(self.actor_idx)?;
        Some(OpId {
            lamport: self.seq_start.saturating_add(offset as u64),
            author,
        })
    }
}

fn char_to_byte_offset(s: &str, char_idx: usize) -> usize {
    if char_idx == 0 {
        return 0;
    }
    s.char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or_else(|| s.len())
}

/// Counted position index over run lengths.
#[derive(Debug, Clone, Default)]
struct CountedPositionIndex {
    weights: Vec<u32>,
    fenwick: Vec<u32>,
}

impl CountedPositionIndex {
    fn new_from_weights(weights: &[usize]) -> Self {
        let mut idx = Self {
            weights: weights.iter().map(|&w| Self::to_u32(w)).collect(),
            fenwick: Vec::new(),
        };
        idx.rebuild_fenwick();
        idx
    }

    #[inline]
    fn to_u32(value: usize) -> u32 {
        value.min(u32::MAX as usize) as u32
    }

    fn run_count(&self) -> usize {
        self.weights.len()
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

    fn update_weight(&mut self, run_idx: usize, new_weight: usize) {
        if run_idx >= self.weights.len() {
            return;
        }
        let new_weight = Self::to_u32(new_weight);
        let old_weight = self.weights[run_idx];
        if old_weight == new_weight {
            return;
        }
        self.weights[run_idx] = new_weight;
        let idx1 = run_idx + 1;
        if new_weight > old_weight {
            self.add_u32(idx1, new_weight - old_weight);
        } else {
            self.sub_u32(idx1, old_weight - new_weight);
        }
    }

    /// Resolve 0-based visible offset -> `(run_idx, offset_inside_run)`.
    fn select_offset(&self, offset: usize) -> Option<(usize, usize)> {
        if offset >= self.total_len() {
            return None;
        }
        let run_idx = self.lower_bound(offset + 1)?;
        let run_start = self.prefix_sum(run_idx);
        Some((run_idx, offset.saturating_sub(run_start)))
    }

    fn rebuild_fenwick(&mut self) {
        self.fenwick.clear();
        self.fenwick.resize(self.weights.len() + 1, 0);
        for i in 0..self.weights.len() {
            self.add(i + 1, self.weights[i] as usize);
        }
    }

    fn add(&mut self, idx1: usize, delta: usize) {
        let delta = Self::to_u32(delta);
        self.add_u32(idx1, delta);
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
        while idx1 > 0 {
            acc = acc.saturating_add(self.fenwick[idx1] as u64);
            idx1 &= idx1 - 1;
        }
        acc.min(usize::MAX as u64) as usize
    }

    /// Smallest 0-based index where prefix sum is at least `target`.
    fn lower_bound(&self, target: usize) -> Option<usize> {
        if target == 0 || self.run_count() == 0 {
            return None;
        }
        let total = self.prefix_sum(self.run_count());
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

/// Incrementally maintained per-key text materialization.
///
/// This scaffold keeps two layers:
/// - metadata (`entries`, `children`, `pending_deletes`)
/// - dense visible cache (`visible_ids`, `visible_string`)
#[derive(Debug, Default)]
pub struct TextProjection {
    entries: HashMap<CompactId, ProjectionEntry>,
    root_children: Vec<CompactId>,
    children: HashMap<CompactId, Vec<CompactId>>,
    tombstones: TombstoneStore,
    pending_deletes: HashSet<CompactId>,
    visible_ids: Vec<CompactId>,
    visible_runs: Vec<TextRun>,
    actor_table: ProjectionActorTable,
    visible_string: String,
    visible_pos_by_id: HashMap<CompactId, usize>,
    position_index: CountedPositionIndex,
    range_delete_scratch: Vec<OpId>,
    /// Last locally-updated visible character span `[start, end)`.
    last_dirty_range: Option<(usize, usize)>,
    /// Coalesced dirty spans in visible-offset space.
    dirty_ranges: Vec<(usize, usize)>,
    dirty_range_merge_count: u64,
    full_rebuild_count: u64,
    dirty: bool,
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

    fn child_insert_idx(&self, siblings: &[CompactId], child: CompactId) -> usize {
        siblings.partition_point(|existing| self.compact_id_cmp(*existing, child).is_gt())
    }

    fn add_child_edge(&mut self, parent: Option<CompactId>, child: CompactId) {
        match parent {
            Some(parent_id) => {
                let idx = {
                    let siblings = self.children.get(&parent_id).map(Vec::as_slice).unwrap_or(&[]);
                    self.child_insert_idx(siblings, child)
                };
                self.children.entry(parent_id).or_default().insert(idx, child);
            }
            None => {
                let idx = self.child_insert_idx(&self.root_children, child);
                self.root_children.insert(idx, child);
            }
        }
    }

    fn elapsed_ns_u64(start: Instant) -> u64 {
        let ns = start.elapsed().as_nanos();
        ns.min(u64::MAX as u128) as u64
    }

    fn mark_dirty(&mut self) {
        if !self.dirty {
            self.invalidation_count = self.invalidation_count.saturating_add(1);
        }
        self.dirty = true;
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

    /// Build a fresh projection from the current oplog view for one key.
    pub fn from_nodes(nodes: &[&SyncNode], key: &str) -> Self {
        let mut projection = Self::default();
        for node in nodes {
            projection.apply_node(node, key);
        }
        projection.ensure_materialized();
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
        if updates.is_empty() {
            return;
        }

        for update in updates {
            match update {
                ProjectionUpdateOp::Insert { id, after, ch } => {
                    self.apply_insert_metadata(*id, *after, *ch);
                }
                ProjectionUpdateOp::Delete { target } => {
                    self.apply_delete_metadata(*target);
                }
            }
        }

        self.mark_dirty();
        self.ensure_materialized();
    }

    fn apply_insert_metadata(&mut self, id: OpId, after: Option<OpId>, ch: char) {
        let compact_id = self.compact_id_for(id);
        if self.entries.contains_key(&compact_id) {
            return;
        }
        self.insert_ops_applied = self.insert_ops_applied.saturating_add(1);
        let tombstoned_at_birth = self.pending_deletes.remove(&compact_id);
        self.entries.insert(compact_id, ProjectionEntry { ch });
        if tombstoned_at_birth {
            self.tombstones.mark_deleted(compact_id);
        }
        let after_compact = after.map(|parent| self.compact_id_for(parent));
        self.add_child_edge(after_compact, compact_id);
    }

    fn apply_delete_metadata(&mut self, target: OpId) {
        let compact_id = self.compact_id_for(target);
        match self.entries.get(&compact_id) {
            Some(_) => {
                if self.tombstones.mark_deleted(compact_id) {
                    self.delete_ops_applied = self.delete_ops_applied.saturating_add(1);
                }
            }
            None => {
                self.pending_deletes.insert(compact_id);
            }
        }
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

        if self.dirty {
            self.ensure_materialized();
        }

        let mut insert_after =
            Self::resolve_anchor_after_visible_seq(&self.visible_ids, anchor, &self.actor_table);

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

        if self.dirty {
            self.ensure_materialized();
        }

        self.range_delete_scratch.clear();
        let start = Self::resolve_anchor_index_for_visible_ids(&self.visible_ids, anchor, &self.actor_table);
        let end = start.saturating_add(len_chars).min(self.visible_ids.len());
        for idx in start..end {
            if let Some(id) = self.visible_ids[idx].to_op_id(&self.actor_table) {
                self.range_delete_scratch.push(id);
            }
        }

        while let Some(id) = self.range_delete_scratch.pop() {
            self.apply_delete(id);
        }
    }

    fn resolve_anchor_after_visible_seq(
        seq: &[CompactId],
        anchor: TextRangeAnchor,
        actor_table: &ProjectionActorTable,
    ) -> Option<OpId> {
        let idx = Self::resolve_anchor_index_for_visible_ids(seq, anchor, actor_table);
        if idx == 0 {
            None
        } else {
            seq[idx - 1].to_op_id(actor_table)
        }
    }

    fn resolve_anchor_index_for_visible_ids(
        seq: &[CompactId],
        anchor: TextRangeAnchor,
        actor_table: &ProjectionActorTable,
    ) -> usize {
        match anchor {
            TextRangeAnchor::Start => 0,
            TextRangeAnchor::End => seq.len(),
            TextRangeAnchor::Offset(i) => i.min(seq.len()),
            TextRangeAnchor::After(id) => {
                let Some(compact_id) = CompactId::from_op_id_existing(id, actor_table) else {
                    return seq.len();
                };
                seq.iter()
                    .position(|existing| *existing == compact_id)
                    .map(|i| i + 1)
                    .unwrap_or(seq.len())
            }
        }
    }

    pub(crate) fn apply_insert(&mut self, id: OpId, after: Option<OpId>, ch: char) {
        let compact_id = self.compact_id_for(id);
        if self.dirty {
            self.ensure_materialized();
        }
        if self.entries.contains_key(&compact_id) {
            return;
        }
        self.insert_ops_applied = self.insert_ops_applied.saturating_add(1);
        let after_compact = after.map(|parent| self.compact_id_for(parent));
        let can_fast_append = self.can_fast_append(after_compact);
        let tombstoned_at_birth = self.pending_deletes.remove(&compact_id);
        self.entries.insert(compact_id, ProjectionEntry { ch });
        if tombstoned_at_birth {
            self.tombstones.mark_deleted(compact_id);
        }
        self.add_child_edge(after_compact, compact_id);

        if tombstoned_at_birth {
            return; // tombstoned-at-birth: metadata only, no visible delta
        }

        if can_fast_append {
            let visible_idx = self.visible_ids.len();
            self.visible_ids.push(compact_id);
            self.insert_into_runs(visible_idx, id, ch);
            self.visible_string.push(ch);
            self.visible_pos_by_id.insert(compact_id, visible_idx);
            self.record_dirty_range(visible_idx, visible_idx + 1);
            self.dirty = false;
            return;
        }

        if let Some(visible_idx) = self.compute_visible_index_for_id(id) {
            self.visible_ids.insert(visible_idx, compact_id);
            self.insert_into_runs(visible_idx, id, ch);
            self.insert_char_at(visible_idx, ch);
            for pos in self.visible_pos_by_id.values_mut() {
                if *pos >= visible_idx {
                    *pos += 1;
                }
            }
            self.visible_pos_by_id.insert(compact_id, visible_idx);
            self.record_dirty_range(visible_idx, visible_idx + 1);
            self.dirty = false;
        } else {
            // Conservative fallback when incremental placement cannot be derived.
            self.mark_dirty();
            self.ensure_materialized();
        }
    }

    fn can_fast_append(&self, after_compact: Option<CompactId>) -> bool {
        match after_compact {
            None => self.visible_ids.is_empty() && self.root_children.is_empty(),
            Some(parent_cid) => {
                let Some(tail_cid) = self.visible_ids.last().copied() else {
                    return false;
                };
                if tail_cid != parent_cid {
                    return false;
                }
                self.children
                    .get(&parent_cid)
                    .is_none_or(Vec::is_empty)
            }
        }
    }

    pub(crate) fn apply_delete(&mut self, target: OpId) {
        if self.dirty {
            self.ensure_materialized();
        }
        let Some(target_cid) = self.compact_id_existing(target) else {
            let compact_id = self.compact_id_for(target);
            self.pending_deletes.insert(compact_id);
            return;
        };
        match self.entries.get(&target_cid) {
            Some(_) => {
                if self.tombstones.mark_deleted(target_cid) {
                    self.delete_ops_applied = self.delete_ops_applied.saturating_add(1);
                    if let Some(visible_idx) = self.index_of_visible_id(target) {
                        self.visible_ids.remove(visible_idx);
                        self.remove_from_runs(visible_idx);
                        self.remove_char_at(visible_idx);
                        self.visible_pos_by_id.remove(&target_cid);
                        for pos in self.visible_pos_by_id.values_mut() {
                            if *pos > visible_idx {
                                *pos -= 1;
                            }
                        }
                        self.record_dirty_range(visible_idx, visible_idx);
                        self.dirty = false;
                    } else {
                        self.mark_dirty();
                        self.ensure_materialized();
                    }
                }
            }
            None => {
                self.pending_deletes.insert(target_cid);
            }
        }
    }

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
        if len == 0 {
            return String::new();
        }
        self.visible_string.chars().skip(start).take(len).collect()
    }

    pub fn last_dirty_range(&self) -> Option<(usize, usize)> {
        self.last_dirty_range
    }

    pub fn debug_stats(&self) -> TextProjectionDebugStats {
        let (fanout_max, fanout_avg_milli) = self.sibling_fanout_stats();
        let dirty_span_chars = self
            .dirty_ranges
            .iter()
            .map(|(s, e)| e.saturating_sub(*s))
            .sum();
        let tombstone_count = self.tombstones.count();
        TextProjectionDebugStats {
            metadata_entries: self.entries.len(),
            visible_len: self.visible_ids.len(),
            tombstone_count,
            tombstone_span_count: self.tombstones.span_count(),
            tombstone_author_bucket_count: self.tombstones.author_bucket_count(),
            pending_deletes_count: self.pending_deletes.len(),
            child_bucket_count: self.children.len(),
            visible_string_capacity: self.visible_string.capacity(),
            index_weights_len: self.position_index.weights.len(),
            index_fenwick_len: self.position_index.fenwick.len(),
            full_rebuild_count: self.full_rebuild_count,
            sibling_fanout_max: fanout_max,
            sibling_fanout_avg_milli: fanout_avg_milli,
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

    /// Compact resident projection storage for cold keys by shrinking
    /// over-allocated buffers/maps while keeping semantics unchanged.
    pub fn compact_cold_storage(&mut self) -> bool {
        if self.dirty {
            self.ensure_materialized();
        }

        let before = self.visible_string.capacity()
            .saturating_add(self.visible_ids.capacity())
            .saturating_add(self.visible_runs.capacity())
            .saturating_add(self.visible_pos_by_id.capacity())
            .saturating_add(self.position_index.weights.capacity())
            .saturating_add(self.position_index.fenwick.capacity());

        self.visible_string.shrink_to_fit();
        self.visible_ids.shrink_to_fit();
        self.visible_runs.shrink_to_fit();
        self.visible_pos_by_id.shrink_to_fit();
        self.range_delete_scratch.shrink_to_fit();
        self.dirty_ranges.shrink_to_fit();
        self.entries.shrink_to_fit();
        self.children.shrink_to_fit();
        self.pending_deletes.shrink_to_fit();
        self.tombstones.deleted_ids.shrink_to_fit();
        self.tombstones.spans_by_actor.shrink_to_fit();
        self.root_children.shrink_to_fit();
        for siblings in self.children.values_mut() {
            siblings.shrink_to_fit();
        }
        for spans in self.tombstones.spans_by_actor.values_mut() {
            spans.shrink_to_fit();
        }
        self.position_index.weights.shrink_to_fit();
        self.position_index.fenwick.shrink_to_fit();

        let after = self.visible_string.capacity()
            .saturating_add(self.visible_ids.capacity())
            .saturating_add(self.visible_runs.capacity())
            .saturating_add(self.visible_pos_by_id.capacity())
            .saturating_add(self.position_index.weights.capacity())
            .saturating_add(self.position_index.fenwick.capacity());

        after < before
    }

    fn sibling_fanout_stats(&self) -> (usize, u64) {
        if self.children.is_empty() && self.root_children.is_empty() {
            return (0, 0);
        }
        let mut max = 0usize;
        let mut sum = self.root_children.len();
        let mut n = 1usize;
        max = max.max(self.root_children.len());
        for siblings in self.children.values() {
            let len = siblings.len();
            max = max.max(len);
            sum = sum.saturating_add(len);
            n = n.saturating_add(1);
        }
        if n == 0 {
            (0, 0)
        } else {
            // Fixed-point millesimal average to keep stats struct integer-only.
            (max, ((sum as u64).saturating_mul(1000)) / (n as u64))
        }
    }

    pub(crate) fn anchor_for_offset(&self, offset: usize) -> TextRangeAnchor {
        if offset == 0 {
            return TextRangeAnchor::Start;
        }
        if offset >= self.visible_ids.len() {
            return TextRangeAnchor::End;
        }
        match self.id_for_visible_offset(offset.saturating_sub(1)) {
            Some(id) => TextRangeAnchor::After(id),
            None => TextRangeAnchor::Start,
        }
    }

    pub(crate) fn offset_for_anchor(&self, anchor: TextRangeAnchor) -> usize {
        match anchor {
            TextRangeAnchor::Start => 0,
            TextRangeAnchor::End => self.visible_ids.len(),
            TextRangeAnchor::Offset(i) => i.min(self.visible_ids.len()),
            TextRangeAnchor::After(id) => CompactId::from_op_id_existing(id, &self.actor_table)
                .and_then(|cid| self.visible_pos_by_id.get(&cid).copied())
                .map(|idx| idx.saturating_add(1))
                .unwrap_or(self.visible_ids.len()),
        }
    }

    fn id_for_visible_offset(&self, offset: usize) -> Option<OpId> {
        let (run_idx, in_run_offset) = self.position_index.select_offset(offset)?;
        let run = self.visible_runs.get(run_idx)?;
        run.id_at(&self.actor_table, in_run_offset)
    }

    fn index_of_visible_id(&self, id: OpId) -> Option<usize> {
        let cid = CompactId::from_op_id_existing(id, &self.actor_table)?;
        self.visible_pos_by_id.get(&cid).copied()
    }

    fn char_byte_offset(s: &str, char_idx: usize) -> usize {
        char_to_byte_offset(s, char_idx)
    }

    fn can_runs_merge(left: &TextRun, right: &TextRun) -> bool {
        left.actor_idx == right.actor_idx && left.last_lamport().saturating_add(1) == right.seq_start
    }

    fn merge_neighbor_runs(&mut self, left_idx: usize) {
        if left_idx + 1 >= self.visible_runs.len() {
            return;
        }
        if !Self::can_runs_merge(&self.visible_runs[left_idx], &self.visible_runs[left_idx + 1]) {
            return;
        }
        let right = self.visible_runs.remove(left_idx + 1);
        self.visible_runs[left_idx].append_run(right);
    }

    fn locate_run_offset_for_insert(&self, global_idx: usize) -> Option<(usize, usize)> {
        let mut seen = 0usize;
        for (run_idx, run) in self.visible_runs.iter().enumerate() {
            let run_len = run.len_chars();
            if global_idx < seen + run_len {
                return Some((run_idx, global_idx.saturating_sub(seen)));
            }
            if global_idx == seen + run_len {
                if run_idx + 1 < self.visible_runs.len() {
                    return Some((run_idx + 1, 0));
                }
                return Some((run_idx, run_len));
            }
            seen = seen.saturating_add(run_len);
        }
        None
    }

    fn locate_run_offset_for_existing(&self, global_idx: usize) -> Option<(usize, usize)> {
        let mut seen = 0usize;
        for (run_idx, run) in self.visible_runs.iter().enumerate() {
            let run_len = run.len_chars();
            if global_idx < seen + run_len {
                return Some((run_idx, global_idx.saturating_sub(seen)));
            }
            seen = seen.saturating_add(run_len);
        }
        None
    }

    fn insert_into_runs(&mut self, global_idx: usize, id: OpId, ch: char) {
        let actor_idx = self.actor_table.get_or_insert(id.author);
        if self.visible_runs.is_empty() {
            self.visible_runs.push(TextRun::from_single(id, actor_idx, ch));
            self.refresh_run_index_incremental();
            return;
        }

        if global_idx >= self.visible_ids.len().saturating_sub(1) {
            let last_idx = self.visible_runs.len() - 1;
            if self.visible_runs[last_idx].can_append(id, actor_idx) {
                self.visible_runs[last_idx].append_char(ch);
                self.refresh_run_index_for_weight_change(last_idx);
            } else {
                self.visible_runs.push(TextRun::from_single(id, actor_idx, ch));
                self.refresh_run_index_incremental();
            }
            return;
        }

        let Some((run_idx, offset)) = self.locate_run_offset_for_insert(global_idx) else {
            self.visible_runs.push(TextRun::from_single(id, actor_idx, ch));
            self.refresh_run_index_incremental();
            return;
        };

        let run_len = self.visible_runs[run_idx].len_chars();
        if offset == run_len {
            let mut structure_changed = false;
            let mut weight_changed_run: Option<usize> = None;
            if self.visible_runs[run_idx].can_append(id, actor_idx) {
                self.visible_runs[run_idx].append_char(ch);
                weight_changed_run = Some(run_idx);
            } else if run_idx + 1 < self.visible_runs.len()
                && self.visible_runs[run_idx + 1].can_prepend(id, actor_idx)
            {
                self.visible_runs[run_idx + 1].prepend_char(ch);
                weight_changed_run = Some(run_idx + 1);
            } else {
                self.visible_runs
                    .insert(run_idx + 1, TextRun::from_single(id, actor_idx, ch));
                structure_changed = true;
            }
            if run_idx > 0 {
                let before = self.visible_runs.len();
                self.merge_neighbor_runs(run_idx - 1);
                structure_changed |= self.visible_runs.len() != before;
            }
            let before = self.visible_runs.len();
            self.merge_neighbor_runs(run_idx);
            structure_changed |= self.visible_runs.len() != before;
            if structure_changed {
                self.refresh_run_index_incremental();
            } else if let Some(changed_idx) = weight_changed_run {
                self.refresh_run_index_for_weight_change(changed_idx);
            }
            return;
        }

        let right = self.visible_runs[run_idx].split_at(offset);
        self.visible_runs
            .insert(run_idx + 1, TextRun::from_single(id, actor_idx, ch));
        if !right.is_empty() {
            self.visible_runs.insert(run_idx + 2, right);
        }
        self.merge_neighbor_runs(run_idx);
        self.merge_neighbor_runs(run_idx + 1);
        self.refresh_run_index_incremental();
    }

    fn remove_from_runs(&mut self, global_idx: usize) {
        let Some((run_idx, offset)) = self.locate_run_offset_for_existing(global_idx) else {
            return;
        };

        self.visible_runs[run_idx].remove_at(offset);
        if self.visible_runs[run_idx].is_empty() {
            self.visible_runs.remove(run_idx);
            if run_idx > 0 {
                self.merge_neighbor_runs(run_idx - 1);
            }
            self.refresh_run_index_incremental();
            return;
        }

        let mut structure_changed = false;
        if run_idx > 0 {
            let before = self.visible_runs.len();
            self.merge_neighbor_runs(run_idx - 1);
            structure_changed |= self.visible_runs.len() != before;
        }
        let before = self.visible_runs.len();
        self.merge_neighbor_runs(run_idx);
        structure_changed |= self.visible_runs.len() != before;
        if structure_changed {
            self.refresh_run_index_incremental();
        } else {
            self.refresh_run_index_for_weight_change(run_idx);
        }
    }

    fn rebuild_runs_from_visible_seq(&mut self) {
        self.visible_runs.clear();
        for (cid, ch) in self.visible_ids.iter().copied().zip(self.visible_string.chars()) {
            let id = match cid.to_op_id(&self.actor_table) {
                Some(id) => id,
                None => continue,
            };
            if let Some(last) = self.visible_runs.last_mut() {
                if last.can_append(id, cid.actor_idx) {
                    last.append_char(ch);
                    continue;
                }
            }
            self.visible_runs.push(TextRun::from_single(id, cid.actor_idx, ch));
        }
    }

    fn refresh_run_index_incremental(&mut self) {
        let started = Instant::now();
        let weights: Vec<usize> = self.visible_runs.iter().map(TextRun::len_chars).collect();
        self.position_index.rebuild_from_weights(&weights);
        self.index_update_time_ns = self
            .index_update_time_ns
            .saturating_add(Self::elapsed_ns_u64(started));
    }

    fn refresh_run_index_for_weight_change(&mut self, run_idx: usize) {
        let started = Instant::now();
        let new_weight = self
            .visible_runs
            .get(run_idx)
            .map(TextRun::len_chars)
            .unwrap_or(0);
        self.position_index.update_weight(run_idx, new_weight);
        self.index_update_time_ns = self
            .index_update_time_ns
            .saturating_add(Self::elapsed_ns_u64(started));
    }

    fn insert_char_at(&mut self, char_idx: usize, ch: char) {
        let byte_idx = Self::char_byte_offset(&self.visible_string, char_idx);
        self.visible_string.insert(byte_idx, ch);
    }

    fn remove_char_at(&mut self, char_idx: usize) {
        let start = Self::char_byte_offset(&self.visible_string, char_idx);
        let end = Self::char_byte_offset(&self.visible_string, char_idx + 1);
        if start < end && end <= self.visible_string.len() {
            self.visible_string.replace_range(start..end, "");
        }
    }

    /// Compute this id's visible index from metadata traversal order.
    fn compute_visible_index_for_id(&self, needle: OpId) -> Option<usize> {
        let needle = self.compact_id_existing(needle)?;
        let mut visible_index = 0usize;
        let mut stack: Vec<CompactId> = Vec::new();

        for &id in self.root_children.iter().rev() {
            stack.push(id);
        }

        while let Some(id) = stack.pop() {
            if id == needle {
                return Some(visible_index);
            }
            if self.entries.contains_key(&id) {
                if !self.tombstones.contains(id) {
                    visible_index += 1;
                }
            }
            if let Some(children) = self.children.get(&id) {
                for &child_id in children.iter().rev() {
                    stack.push(child_id);
                }
            }
        }

        None
    }

    fn ensure_materialized(&mut self) {
        if !self.dirty {
            return;
        }
        self.full_rebuild_count = self.full_rebuild_count.saturating_add(1);

        let mut ids = std::mem::take(&mut self.visible_ids);
        ids.clear();
        let mut text = std::mem::take(&mut self.visible_string);
        text.clear();
        let mut stack: Vec<CompactId> = Vec::new();

        for &id in self.root_children.iter().rev() {
            stack.push(id);
        }

        while let Some(id) = stack.pop() {
            if let Some(entry) = self.entries.get(&id) {
                if !self.tombstones.contains(id) {
                    let ch = entry.ch;
                    ids.push(id);
                    text.push(ch);
                }
            }
            if let Some(children) = self.children.get(&id) {
                for &child_id in children.iter().rev() {
                    stack.push(child_id);
                }
            }
        }

        self.visible_ids = ids;
        self.visible_string = text;
        self.rebuild_runs_from_visible_seq();
        self.visible_pos_by_id.clear();
        self.visible_pos_by_id.reserve(self.visible_ids.len());
        for (i, id) in self.visible_ids.iter().copied().enumerate() {
            self.visible_pos_by_id.insert(id, i);
        }
        let idx_rebuild_started = Instant::now();
        let weights: Vec<usize> = self.visible_runs.iter().map(TextRun::len_chars).collect();
        self.position_index = CountedPositionIndex::new_from_weights(&weights);
        self.index_rebuild_time_ns = self
            .index_rebuild_time_ns
            .saturating_add(Self::elapsed_ns_u64(idx_rebuild_started));
        self.last_dirty_range = Some((0, self.visible_ids.len()));
        self.dirty_ranges.clear();
        self.dirty_ranges.push((0, self.visible_ids.len()));
        self.dirty = false;
    }

    fn build_visible_seq_pairs(&self) -> Vec<(OpId, char)> {
        self.visible_ids
            .iter()
            .filter_map(|cid| cid.to_op_id(&self.actor_table))
            .zip(self.visible_string.chars())
            .collect()
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
    fn text_projection_mid_run_insert_splits_and_delete_merges_runs() {
        let sk = sk(9);
        let mut projection = TextProjection::default();

        let id_a = OpId { lamport: 1, author: sk.verifying_key().to_bytes() };
        let id_c = OpId { lamport: 2, author: sk.verifying_key().to_bytes() };
        let id_b = OpId { lamport: 3, author: sk.verifying_key().to_bytes() };

        projection.apply_insert(id_a, None, 'a');
        projection.apply_insert(id_c, Some(id_a), 'c');
        assert_eq!(projection.resolve_string(), "ac");
        assert_eq!(projection.visible_runs.len(), 1, "sequential inserts should share one run");

        projection.apply_insert(id_b, Some(id_a), 'b');
        assert_eq!(projection.resolve_string(), "abc");
        assert!(projection.visible_runs.len() >= 2, "mid-run insert should split run boundaries");

        projection.apply_delete(id_b);
        assert_eq!(projection.resolve_string(), "ac");
        assert_eq!(projection.visible_runs.len(), 1, "deleting split char should allow merge back");
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
        assert!(!projection.visible_runs.is_empty());

        // Validate that run-level id derivation via actor table remains correct.
        let first_run = &projection.visible_runs[0];
        let derived = first_run
            .id_at(&projection.actor_table, 0)
            .expect("run should derive first id");
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

        // Force split/merge churn around the middle and tail.
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
}
