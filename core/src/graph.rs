use std::collections::{HashMap, HashSet, BTreeSet};
#[cfg(test)]
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(all(feature = "text_projection", not(target_arch = "wasm32")))]
use std::time::Instant;
use crate::{
    compaction::is_snapshot_node,
    conflicts::{ConflictEvent, ConflictKind, ConflictOp},
    error::SyncError,
    frontier::Frontier,
    hash::Hash,
    list::FracIdx,
    node::{NodeId, SyncNode},
    op::{ItemId, ListOp, Op, MapOp, TextOp, Transaction},
    policy::Policy,
    storage::{NodeStore, MemoryNodeStore},
    text::{ProjectionUpdateOp, TextParityMismatch, TextProjectionMode},
    text_range::TextRangeOp,
    text_range::TextRangeAnchor,
};
#[cfg(feature = "text_projection")]
use crate::text::{TextProjection, TextProjectionDebugStats};

/// G5 — maximum gap between a node's Lamport clock and the local
/// `graph.lamport()`. Legitimate concurrent fan-out on a busy room stays
/// well under this window; anything larger is almost certainly tampering
/// (e.g. a node crafted with `lamport = u64::MAX` trying to win every LWW
/// comparison forever *and* poison the room's clock into the same range).
///
/// The bound is deliberately generous: `2^20 ≈ 1 048 576` — at one write
/// per millisecond that's ~17 minutes of pure concurrent-author divergence
/// before any peer has merged with any other, which no real app approaches.
pub const LAMPORT_SLACK: u64 = 1 << 20;

/// G5 — maximum forward skew accepted for `Transaction::wall_ms`, in
/// milliseconds. `wall_ms` is *informational* (never used for merge
/// ordering) so rejecting here doesn't affect correctness; it just closes
/// the "sort-me-to-the-top-of-the-display-timeline" class of abuse for
/// apps that render by wall clock. 24 h comfortably absorbs client clock
/// drift, timezone mistakes, and NTP stumbles.
pub const WALL_SKEW_MAX_MS: u64 = 24 * 60 * 60 * 1000;

/// The resolved, queryable state of the LWW-Map after applying all nodes.
pub type ResolvedMap = HashMap<String, Vec<u8>>;

/// Current LWW winner for one map key.
#[derive(Debug, Clone)]
struct MapWinner {
    lamport: u64,
    author: [u8; 32],
    /// `None` = winning op was a delete (key hidden, but the tombstone
    /// still occupies the LWW slot so lower-priority sets can't resurrect).
    value: Option<Vec<u8>>,
    is_blob: bool,
}

impl MapWinner {
    fn conflict_op(&self) -> ConflictOp {
        match (&self.value, self.is_blob) {
            (None, _) => ConflictOp::Delete,
            (Some(v), true) => {
                let mut hash = [0u8; 32];
                if v.len() == 32 {
                    hash.copy_from_slice(v);
                }
                ConflictOp::SetBlob { blob_hash: hash }
            }
            (Some(v), false) => ConflictOp::Set { value: v.clone() },
        }
    }
}

/// Incrementally maintained list-key state (mirrors
/// `crate::list::resolve_list_seq` semantics exactly).
#[derive(Debug, Default)]
struct ListCacheState {
    items: HashMap<ItemId, ListItemCache>,
    /// Visible items ordered by `(FracIdx, ItemId)` — legacy sort order.
    visible: BTreeSet<(FracIdx, ItemId)>,
}

#[derive(Debug, Clone)]
struct ListItemCache {
    position: Option<FracIdx>,
    pos_priority: (u64, [u8; 32]),
    saw_insert: bool,
    deleted: bool,
}

impl Default for ListItemCache {
    fn default() -> Self {
        ListItemCache {
            position: None,
            pos_priority: (0, [0u8; 32]),
            saw_insert: false,
            deleted: false,
        }
    }
}

impl ListItemCache {
    fn is_visible(&self) -> bool {
        self.saw_insert && !self.deleted && self.position.is_some()
    }
}

/// Per-`(list_key, item)` bookkeeping for the incremental conflict stream.
#[derive(Debug, Default)]
struct ListConflictState {
    pos_winner: Option<(u64, [u8; 32], ConflictOp)>,
    delete_winner: Option<(u64, [u8; 32])>,
    /// Position-setting ops seen so far, kept for loser enumeration when a
    /// delete arrives later. Capped — an item moved thousands of times
    /// before deletion reports at most this many demoted-move conflicts.
    pos_ops: Vec<(u64, [u8; 32], ConflictOp)>,
}

const LIST_CONFLICT_POS_OPS_CAP: usize = 64;

/// Ceiling for the undrained incremental conflict buffer; oldest events
/// are dropped past it (hosts drain per import, so this only guards
/// against an enabled-but-never-drained consumer).
const PENDING_CONFLICTS_CAP: usize = 16_384;

/// Incrementally maintained materialized views over the DAG: LWW map
/// winners (speculative + canonical), list projections, referenced blob
/// hashes, and the optional incremental conflict stream.
///
/// Correctness argument: `(lamport, author)` is a total order and winners
/// only improve; list deletes are absorbing; `local_node_ids` only grows;
/// nodes are never removed from a live graph (compaction builds a fresh
/// graph via `apply_remote`, repopulating these caches). So cache state is
/// an exact, order-independent function of the admitted node set — the
/// same invariant the full-replay resolvers had.
#[derive(Debug, Default)]
struct StateCaches {
    map_all: HashMap<String, MapWinner>,
    map_canonical: HashMap<String, MapWinner>,
    lists: HashMap<String, ListCacheState>,
    blob_hashes: HashSet<Hash>,
    conflict_stream_enabled: bool,
    pending_conflicts: Vec<ConflictEvent>,
    list_conflict_state: HashMap<(String, ItemId), ListConflictState>,
}

/// Outcome of [`StateGraph::apply_remote_batch`].
///
/// Every node passed in lands in exactly one of `accepted` or `rejected`.
/// Duplicates (already in the store, or repeated within the input slice)
/// are silently dropped \u2014 they appear in neither bucket.
#[derive(Debug, Default)]
pub struct BatchResult {
    pub accepted: Vec<NodeId>,
    pub rejected: Vec<(NodeId, SyncError)>,
}

/// Runtime counters for text resolve behavior.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextRuntimeCounters {
    pub projection_hits: u64,
    pub replay_fallbacks: u64,
}

/// Runtime counters for text apply/update behavior.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextApplyRuntimeCounters {
    pub projection_update_calls: u64,
    pub projection_update_total_ns: u64,
    pub projection_invalidation_count: u64,
    pub projection_rebuild_count: u64,
    pub index_maintenance_total_ns: u64,
    pub index_rebuild_total_ns: u64,
    pub dirty_range_count_total: u64,
    pub dirty_span_chars_total: u64,
    pub dirty_range_merge_count_total: u64,
}

/// Runtime lifecycle tier for a text key's projection state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TextRuntimeTemperature {
    /// No resident projection state for the key; oplog-only.
    #[default]
    Cold,
    /// Projection resident and servicing reads/writes.
    Warm,
    /// Heavily accessed key; projection + index + cached flattening are hot.
    Hot,
}

/// Thresholds controlling Warm->Hot transition classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRuntimeTemperatureThresholds {
    /// Promote to Hot when accumulated projection read calls reach this value.
    pub hot_read_calls: u64,
    /// Promote to Hot when accumulated projection write calls reach this value.
    pub hot_write_calls: u64,
}

impl Default for TextRuntimeTemperatureThresholds {
    fn default() -> Self {
        Self {
            hot_read_calls: 64,
            hot_write_calls: 2048,
        }
    }
}

/// Projection residency policy controlling eviction pressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextProjectionResidencyPolicy {
    /// Maximum number of projection-resident keys to keep materialized.
    /// `usize::MAX` means effectively unbounded.
    pub max_resident_keys: usize,
}

impl Default for TextProjectionResidencyPolicy {
    fn default() -> Self {
        Self {
            max_resident_keys: usize::MAX,
        }
    }
}

/// Verify the signatures of the nodes at `indices` within `nodes`.
///
/// Tries `ed25519_dalek::verify_batch` first \u2014 a single multi-scalar
/// multiplication that is ~2\u20133\u00d7 faster than per-node verify for chunks
/// \u2265 32. If the batch verify fails (one or more bad signatures, or a
/// malformed key/signature), falls back to per-node verification so the
/// caller can identify exactly which node was bad.
fn verify_chunk(nodes: &[SyncNode], indices: &[usize]) -> Vec<(usize, bool)> {
    use ed25519_dalek::{Signature as DalekSig, VerifyingKey};

    // Build the parallel arrays for `verify_batch`. Skip nodes whose author
    // public key fails to decode \u2014 those are unconditionally bad.
    let mut good: Vec<usize> = Vec::with_capacity(indices.len());
    let mut messages: Vec<&[u8]> = Vec::with_capacity(indices.len());
    let mut sigs: Vec<DalekSig> = Vec::with_capacity(indices.len());
    let mut keys: Vec<VerifyingKey> = Vec::with_capacity(indices.len());
    let mut out: Vec<(usize, bool)> = Vec::with_capacity(indices.len());

    for &i in indices {
        let n = &nodes[i];
        match VerifyingKey::from_bytes(&n.transaction.author) {
            Ok(vk) => {
                good.push(i);
                messages.push(n.id.as_bytes());
                sigs.push(DalekSig::from_bytes(&n.signature.0));
                keys.push(vk);
            }
            Err(_) => out.push((i, false)),
        }
    }

    if good.is_empty() {
        return out;
    }

    if ed25519_dalek::verify_batch(&messages, &sigs, &keys).is_ok() {
        for i in good {
            out.push((i, true));
        }
    } else {
        // One or more bad signatures \u2014 identify which.
        for i in good {
            let ok = nodes[i].verify_signature().is_ok();
            out.push((i, ok));
        }
    }
    out
}

/// G5 — reject nodes whose Lamport clock jumps ahead of the local
/// `graph.lamport()` by more than [`LAMPORT_SLACK`].
#[inline]
fn check_lamport_ceiling(local_lamport: u64, node: &SyncNode) -> Result<(), SyncError> {
    let ceiling = local_lamport.saturating_add(LAMPORT_SLACK);
    let nl = node.lamport();
    if nl > ceiling {
        return Err(SyncError::LamportCeiling { id: node.id, lamport: nl, ceiling });
    }
    Ok(())
}

/// G5 — reject nodes whose `wall_ms` is more than [`WALL_SKEW_MAX_MS`]
/// past `now_ms`. `wall_ms == 0` is always accepted (compaction nodes
/// and unsigned legacy nodes carry a zero wall clock).
#[inline]
fn check_wall_skew(now_ms: u64, node: &SyncNode) -> Result<(), SyncError> {
    let w = node.transaction.wall_ms;
    if w == 0 {
        return Ok(());
    }
    if w > now_ms.saturating_add(WALL_SKEW_MAX_MS) {
        return Err(SyncError::WallClockSkew { id: node.id, wall_ms: w, now_ms });
    }
    Ok(())
}

/// E3: Configuration for tick-based op batching.
///
/// When active, `set`/`delete` calls accumulate into a buffer instead of
/// immediately creating a signed node. The caller flushes the buffer either
/// on a timer (`interval_ms`) or when `max_ops_per_tick` is reached.
/// Tick boundaries are a transport optimisation only — the CRDT merge logic
/// operates on individual ops and produces identical resolved state regardless
/// of how ops are grouped into nodes.
#[derive(Clone, Debug)]
pub struct TickConfig {
    /// Desired tick window in milliseconds. Used by the JS layer to drive
    /// `setInterval`; the Rust side does not run timers.
    pub interval_ms: u64,
    /// Flush the buffer early if this many ops have accumulated.
    pub max_ops_per_tick: usize,
}

/// The Sync-Graph: an append-only DAG of `SyncNode`s, generic over the
/// node storage backend.
///
/// # CRDT semantics
/// This implements a Last-Write-Wins Map (LWW-Map) where "last" is defined
/// by the Lamport clock. When two concurrent ops `Set` the same key, the
/// one with the higher `lamport` wins. Ties are broken by author public key
/// (lexicographic), giving a deterministic total order on every peer.
///
/// # Merkle structure
/// Every node links to its parents by hash, forming a DAG identical to git's
/// commit graph. The Merkle root is computed by hashing the sorted set of all
/// leaf node IDs (nodes with no children). Two graphs are equal iff their
/// Merkle roots match, enabling O(1) "do we need to sync?" checks.
#[derive(Debug)]
pub struct StateGraph<N: NodeStore = MemoryNodeStore> {
    /// Node storage backend. Swap for IndexedDB, disk, or S3 adapters.
    nodes: N,
    /// Minimal set of "leaf" node IDs (nodes with no children yet).
    /// Updated incrementally as nodes are inserted.
    leaves: HashSet<NodeId>,
    /// The author's own Lamport clock. Incremented on each local transaction.
    lamport: u64,
    /// Room-level write policy. Enforced in `apply_remote`.
    /// Defaults to `AllowAll` (open room, identical to pre-A5 behaviour).
    policy: Policy,
    /// Serializable frontier — mirrors `leaves` and kept in sync on every
    /// insert. Used by the `hello`/`welcome` handshake (A6) and IBF (B1).
    frontier: Frontier,
    /// E2: IDs of nodes produced by `apply_local` on this peer.
    /// Canonical state excludes these — they are speculative until a remote
    /// peer (e.g. the server) re-broadcasts or confirms them.
    local_node_ids: HashSet<NodeId>,
    /// Session-scoped cache of node IDs whose Ed25519 signature has already
    /// been verified on this peer. Populated by `apply_local` (we signed it)
    /// and successful `apply_remote` / `apply_remote_batch`. Used to skip
    /// re-verification when the same node is re-broadcast (server echo,
    /// reconnect catchup, replay). Never persisted; cleared on process exit.
    verified_ids: HashSet<NodeId>,
    /// Per-key incremental text materializations.
    #[cfg(feature = "text_projection")]
    text_projections: HashMap<String, TextProjection>,
    /// Runtime projection read/parity behavior.
    text_projection_mode: TextProjectionMode,
    /// Recent parity mismatch diagnostics.
    #[cfg(feature = "text_projection")]
    text_projection_mismatches: Vec<TextParityMismatch>,
    /// Number of projection self-heal rebuilds triggered by parity mismatch.
    #[cfg(feature = "text_projection")]
    text_projection_self_heals: u64,
    /// Parity check sampling frequency in updates (1 = every update).
    #[cfg(feature = "text_projection")]
    text_parity_sample_every: u64,
    /// Number of text-updates observed since parity mode activation.
    #[cfg(feature = "text_projection")]
    text_parity_update_count: u64,
    /// Optional allow-list for parity checks. Empty => all touched keys.
    #[cfg(feature = "text_projection")]
    text_parity_selected_keys: HashSet<String>,
    /// Number of parity-check runs executed.
    #[cfg(feature = "text_projection")]
    text_parity_check_runs: u64,
    /// Count of resolve calls served by projection.
    text_projection_hits: AtomicU64,
    /// Count of resolve calls served by replay fallback.
    text_replay_fallbacks: AtomicU64,
    /// Count of projection update hook invocations in apply paths.
    text_apply_projection_update_calls: AtomicU64,
    /// Aggregate time spent in projection update hook invocations.
    text_apply_projection_update_ns: AtomicU64,
    /// Runtime tier thresholds for text key lifecycle classification.
    text_temperature_thresholds: TextRuntimeTemperatureThresholds,
    /// Residency policy for projection demotion/eviction.
    #[cfg(feature = "text_projection")]
    text_projection_residency_policy: TextProjectionResidencyPolicy,
    /// Last-touch clock per projection-resident key.
    #[cfg(feature = "text_projection")]
    text_projection_last_touch: HashMap<String, u64>,
    /// Monotonic touch clock used for LRU-style eviction ordering.
    #[cfg(feature = "text_projection")]
    text_projection_touch_clock: u64,
    /// Incremental materialized views (LWW map, lists, blob refs, conflicts).
    state_caches: StateCaches,
}

impl Default for StateGraph<MemoryNodeStore> {
    fn default() -> Self {
        StateGraph {
            nodes: MemoryNodeStore::new(),
            leaves: HashSet::new(),
            lamport: 0,
            policy: Policy::default(),
            frontier: Frontier::default(),
            local_node_ids: HashSet::new(),
            verified_ids: HashSet::new(),
            #[cfg(feature = "text_projection")]
            text_projections: HashMap::new(),
            text_projection_mode: TextProjectionMode::Enabled,
            #[cfg(feature = "text_projection")]
            text_projection_mismatches: Vec::new(),
            #[cfg(feature = "text_projection")]
            text_projection_self_heals: 0,
            #[cfg(feature = "text_projection")]
            text_parity_sample_every: 16,
            #[cfg(feature = "text_projection")]
            text_parity_update_count: 0,
            #[cfg(feature = "text_projection")]
            text_parity_selected_keys: HashSet::new(),
            #[cfg(feature = "text_projection")]
            text_parity_check_runs: 0,
            text_projection_hits: AtomicU64::new(0),
            text_replay_fallbacks: AtomicU64::new(0),
            text_apply_projection_update_calls: AtomicU64::new(0),
            text_apply_projection_update_ns: AtomicU64::new(0),
            text_temperature_thresholds: TextRuntimeTemperatureThresholds::default(),
            #[cfg(feature = "text_projection")]
            text_projection_residency_policy: TextProjectionResidencyPolicy::default(),
            #[cfg(feature = "text_projection")]
            text_projection_last_touch: HashMap::new(),
            #[cfg(feature = "text_projection")]
            text_projection_touch_clock: 0,
            state_caches: StateCaches::default(),
        }
    }
}

impl StateGraph<MemoryNodeStore> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<N: NodeStore> StateGraph<N> {
    /// Construct a `StateGraph` with a custom storage backend.
    pub fn with_store(nodes: N) -> Self {
        StateGraph {
            nodes,
            leaves: HashSet::new(),
            lamport: 0,
            policy: Policy::default(),
            frontier: Frontier::default(),
            local_node_ids: HashSet::new(),
            verified_ids: HashSet::new(),
            #[cfg(feature = "text_projection")]
            text_projections: HashMap::new(),
            text_projection_mode: TextProjectionMode::Enabled,
            #[cfg(feature = "text_projection")]
            text_projection_mismatches: Vec::new(),
            #[cfg(feature = "text_projection")]
            text_projection_self_heals: 0,
            #[cfg(feature = "text_projection")]
            text_parity_sample_every: 16,
            #[cfg(feature = "text_projection")]
            text_parity_update_count: 0,
            #[cfg(feature = "text_projection")]
            text_parity_selected_keys: HashSet::new(),
            #[cfg(feature = "text_projection")]
            text_parity_check_runs: 0,
            text_projection_hits: AtomicU64::new(0),
            text_replay_fallbacks: AtomicU64::new(0),
            text_apply_projection_update_calls: AtomicU64::new(0),
            text_apply_projection_update_ns: AtomicU64::new(0),
            text_temperature_thresholds: TextRuntimeTemperatureThresholds::default(),
            #[cfg(feature = "text_projection")]
            text_projection_residency_policy: TextProjectionResidencyPolicy::default(),
            #[cfg(feature = "text_projection")]
            text_projection_last_touch: HashMap::new(),
            #[cfg(feature = "text_projection")]
            text_projection_touch_clock: 0,
            state_caches: StateCaches::default(),
        }
    }

    /// Update text projection read/parity behavior.
    pub fn set_text_projection_mode(&mut self, mode: TextProjectionMode) {
        self.text_projection_mode = mode;
        #[cfg(feature = "text_projection")]
        {
        self.text_parity_update_count = 0;
        if mode != TextProjectionMode::Disabled {
            self.rebuild_all_text_projections();
        }
        }
    }

    /// Configure parity sampling cadence in updates (`1` = every update).
    pub fn set_text_parity_sample_every(&mut self, every: u64) {
        #[cfg(feature = "text_projection")]
        {
        self.text_parity_sample_every = every.max(1);
        }
        #[cfg(not(feature = "text_projection"))]
        {
            let _ = every;
        }
    }

    /// Configure optional parity key allow-list; empty list means "all keys".
    pub fn set_text_parity_selected_keys(&mut self, keys: Vec<String>) {
        #[cfg(feature = "text_projection")]
        {
        self.text_parity_selected_keys = keys.into_iter().collect();
        }
        #[cfg(not(feature = "text_projection"))]
        {
            let _ = keys;
        }
    }

    /// Number of parity-check runs executed.
    pub fn text_parity_check_runs(&self) -> u64 {
        #[cfg(feature = "text_projection")]
        {
        self.text_parity_check_runs
        }
        #[cfg(not(feature = "text_projection"))]
        {
            0
        }
    }

    /// Current text projection mode.
    pub fn text_projection_mode(&self) -> TextProjectionMode {
        self.text_projection_mode
    }

    /// Recent projection parity mismatches.
    pub fn text_projection_mismatches(&self) -> &[TextParityMismatch] {
        #[cfg(feature = "text_projection")]
        {
        &self.text_projection_mismatches
        }
        #[cfg(not(feature = "text_projection"))]
        {
            &[]
        }
    }

    /// Number of parity-triggered projection self-heal rebuilds.
    pub fn text_projection_self_heal_count(&self) -> u64 {
        #[cfg(feature = "text_projection")]
        {
        self.text_projection_self_heals
        }
        #[cfg(not(feature = "text_projection"))]
        {
            0
        }
    }

    /// Snapshot current runtime text resolve counters.
    pub fn text_runtime_counters(&self) -> TextRuntimeCounters {
        TextRuntimeCounters {
            projection_hits: self.text_projection_hits.load(Ordering::Relaxed),
            replay_fallbacks: self.text_replay_fallbacks.load(Ordering::Relaxed),
        }
    }

    /// Reset runtime text resolve counters to zero.
    pub fn reset_text_runtime_counters(&self) {
        self.text_projection_hits.store(0, Ordering::Relaxed);
        self.text_replay_fallbacks.store(0, Ordering::Relaxed);
    }

    /// Snapshot current runtime text apply/update counters.
    pub fn text_apply_runtime_counters(&self) -> TextApplyRuntimeCounters {
        #[cfg(feature = "text_projection")]
        {
            let mut invalidations = 0u64;
            let mut rebuilds = 0u64;
            let mut index_updates = 0u64;
            let mut index_rebuilds = 0u64;
            let mut dirty_range_count = 0u64;
            let mut dirty_span_chars = 0u64;
            let mut dirty_range_merges = 0u64;
            for projection in self.text_projections.values() {
                let stats = projection.debug_stats();
                invalidations = invalidations.saturating_add(stats.invalidation_count);
                rebuilds = rebuilds.saturating_add(stats.full_rebuild_count);
                index_updates = index_updates.saturating_add(stats.index_update_time_ns);
                index_rebuilds = index_rebuilds.saturating_add(stats.index_rebuild_time_ns);
                dirty_range_count = dirty_range_count.saturating_add(stats.dirty_range_count as u64);
                dirty_span_chars = dirty_span_chars.saturating_add(stats.dirty_span_chars as u64);
                dirty_range_merges =
                    dirty_range_merges.saturating_add(stats.dirty_range_merge_count);
            }
            return TextApplyRuntimeCounters {
                projection_update_calls: self
                    .text_apply_projection_update_calls
                    .load(Ordering::Relaxed),
                projection_update_total_ns: self
                    .text_apply_projection_update_ns
                    .load(Ordering::Relaxed),
                projection_invalidation_count: invalidations,
                projection_rebuild_count: rebuilds,
                index_maintenance_total_ns: index_updates,
                index_rebuild_total_ns: index_rebuilds,
                dirty_range_count_total: dirty_range_count,
                dirty_span_chars_total: dirty_span_chars,
                dirty_range_merge_count_total: dirty_range_merges,
            };
        }
        #[cfg(not(feature = "text_projection"))]
        {
            TextApplyRuntimeCounters {
                projection_update_calls: self
                    .text_apply_projection_update_calls
                    .load(Ordering::Relaxed),
                projection_update_total_ns: self
                    .text_apply_projection_update_ns
                    .load(Ordering::Relaxed),
                projection_invalidation_count: 0,
                projection_rebuild_count: 0,
                index_maintenance_total_ns: 0,
                index_rebuild_total_ns: 0,
                dirty_range_count_total: 0,
                dirty_span_chars_total: 0,
                dirty_range_merge_count_total: 0,
            }
        }
    }

    /// Reset runtime text apply/update counters to zero.
    pub fn reset_text_apply_runtime_counters(&self) {
        self.text_apply_projection_update_calls
            .store(0, Ordering::Relaxed);
        self.text_apply_projection_update_ns.store(0, Ordering::Relaxed);
    }

    /// Projection debug stats for a specific key (feature-gated).
    #[cfg(feature = "text_projection")]
    pub fn text_projection_debug_stats(&self, key: &str) -> Option<TextProjectionDebugStats> {
        self.text_projections.get(key).map(|p| p.debug_stats())
    }

    /// Configure runtime temperature transition thresholds.
    pub fn set_text_runtime_temperature_thresholds(
        &mut self,
        thresholds: TextRuntimeTemperatureThresholds,
    ) {
        self.text_temperature_thresholds = thresholds;
    }

    /// Return runtime temperature transition thresholds.
    pub fn text_runtime_temperature_thresholds(&self) -> TextRuntimeTemperatureThresholds {
        self.text_temperature_thresholds
    }

    /// Current runtime lifecycle temperature for one key.
    pub fn text_runtime_temperature_for_key(&self, key: &str) -> TextRuntimeTemperature {
        #[cfg(feature = "text_projection")]
        {
            let Some(stats) = self.text_projection_debug_stats(key) else {
                return TextRuntimeTemperature::Cold;
            };

            let read_calls = stats
                .resolve_seq_calls
                .saturating_add(stats.resolve_string_calls)
                .saturating_add(stats.resolve_range_calls);
            let write_calls = stats
                .insert_ops_applied
                .saturating_add(stats.delete_ops_applied)
                .saturating_add(stats.range_insert_ops_applied)
                .saturating_add(stats.range_delete_ops_applied);

            if read_calls >= self.text_temperature_thresholds.hot_read_calls
                || write_calls >= self.text_temperature_thresholds.hot_write_calls
            {
                TextRuntimeTemperature::Hot
            } else {
                TextRuntimeTemperature::Warm
            }
        }
        #[cfg(not(feature = "text_projection"))]
        {
            let _ = key;
            TextRuntimeTemperature::Cold
        }
    }

    /// Snapshot lifecycle temperature for all projection-resident keys.
    pub fn text_runtime_temperature_snapshot(&self) -> HashMap<String, TextRuntimeTemperature> {
        #[cfg(feature = "text_projection")]
        {
            let mut out = HashMap::new();
            for key in self.text_projections.keys() {
                out.insert(key.clone(), self.text_runtime_temperature_for_key(key));
            }
            out
        }
        #[cfg(not(feature = "text_projection"))]
        {
            HashMap::new()
        }
    }

    /// Configure projection residency demotion/eviction policy.
    pub fn set_text_projection_residency_policy(
        &mut self,
        policy: TextProjectionResidencyPolicy,
    ) {
        #[cfg(feature = "text_projection")]
        {
            self.text_projection_residency_policy = policy;
            self.enforce_text_projection_residency_policy();
        }
        #[cfg(not(feature = "text_projection"))]
        {
            let _ = policy;
        }
    }

    /// Current projection residency policy.
    pub fn text_projection_residency_policy(&self) -> TextProjectionResidencyPolicy {
        #[cfg(feature = "text_projection")]
        {
            self.text_projection_residency_policy
        }
        #[cfg(not(feature = "text_projection"))]
        {
            TextProjectionResidencyPolicy::default()
        }
    }

    /// Number of currently resident text projections.
    pub fn text_projection_resident_key_count(&self) -> usize {
        #[cfg(feature = "text_projection")]
        {
            self.text_projections.len()
        }
        #[cfg(not(feature = "text_projection"))]
        {
            0
        }
    }

    /// Demote one projection key to cold state by dropping resident materialization.
    pub fn demote_text_projection_key(&mut self, key: &str) -> bool {
        #[cfg(feature = "text_projection")]
        {
            self.text_projection_last_touch.remove(key);
            self.text_projections.remove(key).is_some()
        }
        #[cfg(not(feature = "text_projection"))]
        {
            let _ = key;
            false
        }
    }

    /// Ensure a projection for `key` is resident; returns `true` when rebuilt.
    pub fn ensure_text_projection_resident(&mut self, key: &str) -> bool {
        #[cfg(feature = "text_projection")]
        {
            if self.text_projection_mode == TextProjectionMode::Disabled {
                return false;
            }
            if self.text_projections.contains_key(key) {
                self.touch_text_projection_key(key);
                return false;
            }
            self.rebuild_text_projection_for_key(key);
            self.touch_text_projection_key(key);
            self.enforce_text_projection_residency_policy();
            return true;
        }
        #[cfg(not(feature = "text_projection"))]
        {
            let _ = key;
            false
        }
    }

    /// Compact resident projection storage for a key without dropping metadata.
    pub fn compact_text_projection_key(&mut self, key: &str) -> bool {
        #[cfg(feature = "text_projection")]
        {
            let compacted = self
                .text_projections
                .get_mut(key)
                .map(|projection| projection.compact_cold_storage())
                .unwrap_or(false);
            if compacted {
                self.touch_text_projection_key(key);
            }
            compacted
        }
        #[cfg(not(feature = "text_projection"))]
        {
            let _ = key;
            false
        }
    }

    /// Replace the room policy. All subsequent `apply_remote` calls will
    /// enforce the new policy. Existing nodes are not re-validated.
    pub fn set_policy(&mut self, policy: Policy) {
        self.policy = policy;
    }

    /// Return a reference to the current room policy.
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Current Lamport clock value.  The next `apply_local` call will
    /// assign `lamport + 1` — expose this so callers (e.g. the bridge's
    /// E2EE layer) can pre-compute the lamport that a forthcoming
    /// transaction will carry.
    pub fn lamport(&self) -> u64 {
        self.lamport
    }

    // -------------------------------------------------------------------------
    // Insertion
    // -------------------------------------------------------------------------

    /// Apply a locally-authored transaction, producing a new signed DAG node.
    ///
    /// `signing_key` is the author's Ed25519 signing key. The corresponding
    /// verifying key bytes are stored in `transaction.author` and used by
    /// peers to verify the signature on `apply_remote`.
    pub fn apply_local(
        &mut self,
        signing_key: &ed25519_dalek::SigningKey,
        wall_ms: u64,
        ops: Vec<Op>,
    ) -> Result<NodeId, SyncError> {
        // Lamport is a pure logical clock — never fold `wall_ms` into it.
        // `wall_ms` lives on `Transaction` as a separate informational
        // field; mixing them blows past `LAMPORT_SLACK` (G5) and poisons
        // every peer's clock with wall-clock-scale values.
        self.lamport += 1;
        let tx_lamport = self.lamport;
        let extra_insert_ids: u64 = ops
            .iter()
            .map(|op| match op {
                Op::Text(TextOp::InsertRange { text, .. }) => {
                    text.chars().count().saturating_sub(1) as u64
                }
                _ => 0,
            })
            .sum();
        self.lamport = self.lamport.saturating_add(extra_insert_ids);
        let author: [u8; 32] = signing_key.verifying_key().to_bytes();
        let parents: Vec<Hash> = self.leaves.iter().copied().collect();
        let tx = Transaction { author, lamport: tx_lamport, wall_ms, ops, parents };
        let node = SyncNode::new_signed(tx, signing_key);
        let id = node.id;
        self.insert_node(node.clone())?;
        // Speculative view only — locally-authored nodes are excluded from
        // the canonical view until a remote peer confirms/re-broadcasts.
        self.update_state_caches(&node, false);
        self.update_text_projection_from_node(&node);
        // E2: mark this node as locally authored (speculative, not yet confirmed).
        self.local_node_ids.insert(id);
        // We just signed this node ourselves; signature is valid by construction.
        self.verified_ids.insert(id);
        Ok(id)
    }

    /// Lower a range op using the current visible sequence and apply it as
    /// a single canonical persisted range op.
    pub fn apply_local_text_range_op(
        &mut self,
        signing_key: &ed25519_dalek::SigningKey,
        wall_ms: u64,
        op: TextRangeOp,
    ) -> Result<Vec<NodeId>, SyncError> {
        let key = match &op {
            TextRangeOp::Insert { key, .. } | TextRangeOp::Delete { key, .. } => key.clone(),
        };
        let raw_anchor = match &op {
            TextRangeOp::Insert { anchor, .. } | TextRangeOp::Delete { anchor, .. } => *anchor,
        };
        let canonical_anchor = match raw_anchor {
            TextRangeAnchor::Offset(_) => {
                let seq = self.resolve_text_seq_with_chars(&key);
                Self::canonicalize_range_anchor(&seq, raw_anchor)
            }
            _ => raw_anchor,
        };

        let text_op = match op {
            TextRangeOp::Insert { key, text, .. } => Op::Text(TextOp::InsertRange {
                key,
                anchor: canonical_anchor,
                text,
            }),
            TextRangeOp::Delete { key, len_chars, .. } => Op::Text(TextOp::DeleteRange {
                key,
                anchor: canonical_anchor,
                len_chars,
            }),
        };
        let id = self.apply_local(signing_key, wall_ms, vec![text_op])?;
        Ok(vec![id])
    }

    /// Insert a node received from a remote peer.
    ///
    /// Validates that:
    /// - The node's ID matches the hash of its transaction.
    /// - The Ed25519 signature is valid (unsigned/zero-sig nodes pass through).
    /// - The node's Lamport clock is within `LAMPORT_SLACK` of `self.lamport` (G5).
    /// - No parent references an unknown node (use `missing_hashes` first).
    /// - Every op key is permitted for the node's author under the room policy.
    ///
    /// Skips the wall-clock sanity check; see
    /// [`apply_remote_checked`](Self::apply_remote_checked) to enable it.
    pub fn apply_remote(&mut self, node: SyncNode) -> Result<(), SyncError> {
        self.apply_remote_checked(node, None)
    }

    /// Like [`apply_remote`](Self::apply_remote), but additionally enforces
    /// the G5 wall-clock skew ceiling when `now_ms` is `Some`. Pass the
    /// caller's current wall-clock time in milliseconds since the Unix
    /// epoch; nodes whose `wall_ms` is more than [`WALL_SKEW_MAX_MS`] past
    /// `now_ms` are rejected with [`SyncError::WallClockSkew`].
    ///
    /// `None` disables the wall-clock check (same semantics as the plain
    /// `apply_remote`) — used by the WASM client and by compaction code
    /// that doesn't have a trusted clock.
    pub fn apply_remote_checked(&mut self, node: SyncNode, now_ms: Option<u64>) -> Result<(), SyncError> {
        // Verify content-addressable integrity.
        let expected = node.transaction.hash();
        if expected != node.id {
            return Err(SyncError::HashMismatch { expected, actual: node.id });
        }
        // G5: cheap pre-crypto rejects. Run before ed25519 verify so a
        // flood of tampered nodes can't burn CPU on signatures.
        check_lamport_ceiling(self.lamport, &node)?;
        if let Some(now) = now_ms {
            check_wall_skew(now, &node)?;
        }
        // Verify Ed25519 signature (no-op for zero/unsigned nodes, and skipped
        // if we already verified this id earlier in the session).
        if !self.verified_ids.contains(&node.id) {
            node.verify_signature()?;
            self.verified_ids.insert(node.id);
        }
        self.apply_remote_verified(node)
    }

    /// Apply a node whose hash + signature have already been validated.
    /// Performs parent + policy checks and inserts. Used by both
    /// `apply_remote` and `apply_remote_batch`.
    fn apply_remote_verified(&mut self, node: SyncNode) -> Result<(), SyncError> {
        self.apply_remote_verified_no_projection(node.clone())?;
        self.update_text_projection_from_node(&node);
        Ok(())
    }

    /// Apply a node whose hash + signature have already been validated,
    /// excluding projection updates (used by coalesced batch apply paths).
    fn apply_remote_verified_no_projection(&mut self, node: SyncNode) -> Result<(), SyncError> {
        // All parents must already exist locally.
        for parent in node.parents() {
            if !self.nodes.contains(parent) {
                return Err(SyncError::MissingParent(*parent));
            }
        }
        // Enforce write policy: every op key must be permitted for this author.
        // Snapshot nodes (D3) carry \x00-prefixed system keys and bypass policy
        // so that compaction checkpoints can be accepted regardless of room rules.
        if !is_snapshot_node(&node) {
            let author = &node.transaction.author;
            for op in &node.transaction.ops {
                let key = match op {
                    Op::Map(MapOp::Set    { key, .. }) => key.as_str(),
                    Op::Map(MapOp::Delete { key })     => key.as_str(),
                    Op::Map(MapOp::SetBlob{ key, .. }) => key.as_str(),
                    Op::Text(TextOp::Insert { key, .. })
                    | Op::Text(TextOp::Delete { key, .. })
                    | Op::Text(TextOp::InsertRange { key, .. })
                    | Op::Text(TextOp::DeleteRange { key, .. }) => key.as_str(),
                    Op::List(lop) => lop.key(),
                };
                if !self.policy.can_write(key, author) {
                    return Err(SyncError::PolicyViolation {
                        author: *author,
                        key: key.to_string(),
                    });
                }
            }
        }
        self.insert_node(node.clone())?;
        self.update_state_caches(&node, true);
        Ok(())
    }

    fn canonicalize_range_anchor(seq: &[(crate::op::OpId, char)], anchor: TextRangeAnchor) -> TextRangeAnchor {
    match anchor {
        TextRangeAnchor::Offset(i) => {
            let idx = i.min(seq.len());
            if idx == 0 {
                TextRangeAnchor::Start
            } else if idx >= seq.len() {
                TextRangeAnchor::End
            } else {
                TextRangeAnchor::After(seq[idx - 1].0)
            }
        }
        other => other,
    }
}

    #[cfg(feature = "text_projection")]
    fn update_text_projection_from_node(&mut self, node: &SyncNode) {
        #[cfg(not(target_arch = "wasm32"))]
        let started = Instant::now();
        let mut touched: HashSet<String> = HashSet::new();
        for op in &node.transaction.ops {
            match op {
                Op::Text(TextOp::Insert { key, .. })
                | Op::Text(TextOp::Delete { key, .. })
                | Op::Text(TextOp::InsertRange { key, .. })
                | Op::Text(TextOp::DeleteRange { key, .. }) => {
                    self.text_projections
                        .entry(key.clone())
                        .or_default()
                        .apply_node(node, key);
                    touched.insert(key.clone());
                }
                _ => {}
            }
        }

        if self.text_projection_mode == TextProjectionMode::ParityCheck && !touched.is_empty() {
            self.text_parity_update_count = self.text_parity_update_count.saturating_add(1);
            let sample_every = self.text_parity_sample_every.max(1);
            if self.text_parity_update_count % sample_every == 0 {
                let keys_to_check: HashSet<String> = if self.text_parity_selected_keys.is_empty() {
                    touched.clone()
                } else {
                    touched
                        .iter()
                        .filter(|k| self.text_parity_selected_keys.contains(*k))
                        .cloned()
                        .collect()
                };

                if !keys_to_check.is_empty() {
                    self.text_parity_check_runs = self.text_parity_check_runs.saturating_add(1);
                    self.run_text_projection_parity_for_keys(&keys_to_check, Some(node.id));
                }
            }
        }

        for key in &touched {
            self.touch_text_projection_key(key);
        }
        self.enforce_text_projection_residency_policy();

        #[cfg(not(target_arch = "wasm32"))]
        let elapsed_ns = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        #[cfg(target_arch = "wasm32")]
        let elapsed_ns = 0;
        self.text_apply_projection_update_calls
            .fetch_add(1, Ordering::Relaxed);
        self.text_apply_projection_update_ns
            .fetch_add(elapsed_ns, Ordering::Relaxed);
    }

    #[cfg(not(feature = "text_projection"))]
    fn update_text_projection_from_node(&mut self, _node: &SyncNode) {}

    #[cfg(feature = "text_projection")]
    fn update_text_projection_from_nodes_batch(&mut self, nodes: &[SyncNode]) {
        if nodes.is_empty() {
            return;
        }

        #[cfg(not(target_arch = "wasm32"))]
        let started = Instant::now();
        let mut touched: HashSet<String> = HashSet::new();
        let mut touched_node_count = 0u64;

        let mut coalesced_updates: HashMap<String, Vec<ProjectionUpdateOp>> = HashMap::new();
        let mut fallback_range_keys: HashSet<String> = HashSet::new();

        for node in nodes {
            let mut node_touched = false;
            let tx = &node.transaction;
            let tx_id = crate::op::OpId {
                lamport: tx.lamport,
                author: tx.author,
            };
            for op in &tx.ops {
                match op {
                    Op::Text(TextOp::Insert { key, after, ch }) => {
                        coalesced_updates
                            .entry(key.clone())
                            .or_default()
                            .push(ProjectionUpdateOp::Insert {
                                id: tx_id,
                                after: *after,
                                ch: *ch,
                            });
                        touched.insert(key.clone());
                        node_touched = true;
                    }
                    Op::Text(TextOp::Delete { key, target }) => {
                        coalesced_updates
                            .entry(key.clone())
                            .or_default()
                            .push(ProjectionUpdateOp::Delete { target: *target });
                        touched.insert(key.clone());
                        node_touched = true;
                    }
                    Op::Text(TextOp::InsertRange { key, .. })
                    | Op::Text(TextOp::DeleteRange { key, .. }) => {
                        fallback_range_keys.insert(key.clone());
                        touched.insert(key.clone());
                        node_touched = true;
                    }
                    _ => {}
                }
            }
            if node_touched {
                touched_node_count = touched_node_count.saturating_add(1);
            }
        }

        for key in &touched {
            let projection = self.text_projections.entry(key.clone()).or_default();
            if fallback_range_keys.contains(key) {
                for node in nodes {
                    projection.apply_node(node, key);
                }
            } else {
                let updates = coalesced_updates
                    .get(key)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                projection.apply_updates_coalesced(updates);
            }
        }

        if self.text_projection_mode == TextProjectionMode::ParityCheck && !touched.is_empty() {
            let sample_every = self.text_parity_sample_every.max(1);
            for _ in 0..touched_node_count {
                self.text_parity_update_count = self.text_parity_update_count.saturating_add(1);
                if self.text_parity_update_count % sample_every != 0 {
                    continue;
                }
                let keys_to_check: HashSet<String> = if self.text_parity_selected_keys.is_empty() {
                    touched.clone()
                } else {
                    touched
                        .iter()
                        .filter(|k| self.text_parity_selected_keys.contains(*k))
                        .cloned()
                        .collect()
                };
                if keys_to_check.is_empty() {
                    continue;
                }
                self.text_parity_check_runs = self.text_parity_check_runs.saturating_add(1);
                self.run_text_projection_parity_for_keys(&keys_to_check, None);
            }
        }

        for key in &touched {
            self.touch_text_projection_key(key);
        }
        self.enforce_text_projection_residency_policy();

        #[cfg(not(target_arch = "wasm32"))]
        let elapsed_ns = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        #[cfg(target_arch = "wasm32")]
        let elapsed_ns = 0;
        self.text_apply_projection_update_calls
            .fetch_add(touched_node_count, Ordering::Relaxed);
        self.text_apply_projection_update_ns
            .fetch_add(elapsed_ns, Ordering::Relaxed);
    }

    #[cfg(not(feature = "text_projection"))]
    fn update_text_projection_from_nodes_batch(&mut self, _nodes: &[SyncNode]) {}

    #[cfg(feature = "text_projection")]
    fn run_text_projection_parity_for_keys(
        &mut self,
        keys: &HashSet<String>,
        trigger_node: Option<NodeId>,
    ) {
        for key in keys {
            let projected = self
                .text_projections
                .get(key)
                .map(|p| p.resolve_seq())
                .unwrap_or_default();
            let legacy = self.resolve_text_seq_legacy(key);
            if projected == legacy {
                continue;
            }

            self.text_projection_mismatches.push(TextParityMismatch {
                key: key.clone(),
                trigger_node,
                projected_len: projected.len(),
                legacy_len: legacy.len(),
                first_projected: projected.first().map(|(id, _)| *id),
                first_legacy: legacy.first().map(|(id, _)| *id),
            });

            self.rebuild_text_projection_for_key(key);
            self.text_projection_self_heals = self.text_projection_self_heals.saturating_add(1);
        }
    }

    #[cfg(feature = "text_projection")]
    fn rebuild_all_text_projections(&mut self) {
        let keys = self.collect_text_keys();
        for key in keys {
            self.rebuild_text_projection_for_key(&key);
            self.touch_text_projection_key(&key);
        }
        self.enforce_text_projection_residency_policy();
    }

    #[cfg(feature = "text_projection")]
    fn rebuild_text_projection_for_key(&mut self, key: &str) {
        let nodes = self.all_nodes_for_text();
        let projection = TextProjection::from_nodes(&nodes, key);
        self.text_projections.insert(key.to_string(), projection);
    }

    #[cfg(feature = "text_projection")]
    fn touch_text_projection_key(&mut self, key: &str) {
        self.text_projection_touch_clock = self.text_projection_touch_clock.saturating_add(1);
        self.text_projection_last_touch
            .insert(key.to_string(), self.text_projection_touch_clock);
    }

    #[cfg(feature = "text_projection")]
    fn projection_is_hot_with_stats(&self, stats: &TextProjectionDebugStats) -> bool {
        let read_calls = stats
            .resolve_seq_calls
            .saturating_add(stats.resolve_string_calls)
            .saturating_add(stats.resolve_range_calls);
        let write_calls = stats
            .insert_ops_applied
            .saturating_add(stats.delete_ops_applied)
            .saturating_add(stats.range_insert_ops_applied)
            .saturating_add(stats.range_delete_ops_applied);
        read_calls >= self.text_temperature_thresholds.hot_read_calls
            || write_calls >= self.text_temperature_thresholds.hot_write_calls
    }

    #[cfg(feature = "text_projection")]
    fn enforce_text_projection_residency_policy(&mut self) {
        let limit = self.text_projection_residency_policy.max_resident_keys;
        if self.text_projections.len() <= limit {
            return;
        }

        while self.text_projections.len() > limit {
            let mut warm_candidate: Option<(String, u64)> = None;
            let mut fallback_candidate: Option<(String, u64)> = None;

            for key in self.text_projections.keys() {
                let touched = *self.text_projection_last_touch.get(key).unwrap_or(&0);
                let stats = self
                    .text_projections
                    .get(key)
                    .map(|p| p.debug_stats())
                    .unwrap_or_default();
                let is_hot = self.projection_is_hot_with_stats(&stats);

                let update_candidate = |slot: &mut Option<(String, u64)>| {
                    if slot.as_ref().map(|(_, t)| touched < *t).unwrap_or(true) {
                        *slot = Some((key.clone(), touched));
                    }
                };

                update_candidate(&mut fallback_candidate);
                if !is_hot {
                    update_candidate(&mut warm_candidate);
                }
            }

            let victim = warm_candidate.or(fallback_candidate).map(|(k, _)| k);
            let Some(victim_key) = victim else {
                break;
            };

            self.text_projections.remove(&victim_key);
            self.text_projection_last_touch.remove(&victim_key);
        }
    }

    #[cfg(feature = "text_projection")]
    fn collect_text_keys(&self) -> HashSet<String> {
        let mut keys = HashSet::new();
        let node_ids = self.nodes.all_ids();
        for id in &node_ids {
            let Some(node) = self.nodes.get(id) else { continue };
            for op in &node.transaction.ops {
                if let Op::Text(TextOp::Insert { key, .. })
                | Op::Text(TextOp::Delete { key, .. })
                | Op::Text(TextOp::InsertRange { key, .. })
                | Op::Text(TextOp::DeleteRange { key, .. }) = op {
                    keys.insert(key.clone());
                }
            }
        }
        keys
    }

    #[cfg(feature = "text_projection")]
    fn all_nodes_for_text(&self) -> Vec<&SyncNode> {
        let mut nodes: Vec<&SyncNode> = self
            .nodes
            .all_ids()
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .collect();
        nodes.sort_unstable_by(|a, b| {
            a.transaction
                .lamport
                .cmp(&b.transaction.lamport)
                .then_with(|| a.transaction.author.cmp(&b.transaction.author))
                .then_with(|| a.id.cmp(&b.id))
        });
        nodes
    }

    #[cfg(feature = "text_projection")]
    fn resolve_projection_transient(&self, key: &str) -> TextProjection {
        let nodes = self.all_nodes_for_text();
        TextProjection::from_nodes(&nodes, key)
    }

    fn all_nodes_snapshot(&self) -> Vec<&SyncNode> {
        self.nodes
            .all_ids()
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .collect()
    }

    fn resolve_text_seq_legacy(&self, key: &str) -> Vec<(crate::op::OpId, char)> {
        let nodes = self.all_nodes_snapshot();
        crate::text::resolve_text_seq(&nodes, key)
    }

    fn resolve_text_legacy(&self, key: &str) -> String {
        let nodes = self.all_nodes_snapshot();
        crate::text::resolve_text(&nodes, key)
    }

    fn resolve_text_seq_legacy_at_lamport(
        &self,
        key: &str,
        max_lamport: u64,
    ) -> Vec<(crate::op::OpId, char)> {
        let nodes = self.all_nodes_snapshot();
        crate::text::resolve_text_seq_upto_lamport(&nodes, key, max_lamport)
    }

    fn resolve_text_legacy_at_lamport(&self, key: &str, max_lamport: u64) -> String {
        let nodes = self.all_nodes_snapshot();
        crate::text::resolve_text_upto_lamport(&nodes, key, max_lamport)
    }

    /// Bulk-ingest a batch of remote nodes with parallel batched signature
    /// verification.
    ///
    /// Pipeline (see `apply_remote` doc-comment for per-step rationale):
    ///
    /// 1. **Dedupe** — drop nodes already in the store and any duplicates
    ///    within the batch itself.
    /// 2. **Hash integrity** — cheap pre-crypto reject for tampered nodes.
    /// 3. **Batched verify** — group surviving signed nodes into chunks of
    ///    `BATCH_VERIFY_CHUNK` and run `ed25519_dalek::verify_batch` on each.
    ///    On native targets the chunks run in parallel via `rayon`; on wasm
    ///    they run sequentially. If any chunk fails as a batch, that chunk
    ///    falls back to per-node verification so we can identify which
    ///    specific node was bad without rejecting the rest.
    /// 4. **Apply in input order** — parent ordering is preserved so a node
    ///    can reference an earlier sibling within the same batch.
    ///
    /// Returns a `BatchResult` listing accepted ids and rejected `(id, reason)`
    /// pairs. The function never panics on bad input — every node is
    /// accounted for in exactly one bucket.
    pub fn apply_remote_batch(&mut self, nodes: Vec<SyncNode>) -> BatchResult {
        self.apply_remote_batch_checked(nodes, None)
    }

    /// Like [`apply_remote_batch`](Self::apply_remote_batch) but additionally
    /// enforces the G5 wall-clock skew ceiling when `now_ms` is `Some`.
    /// `None` disables the wall-clock check.
    ///
    /// Both G5 checks (Lamport ceiling + wall skew) run *before* the batch
    /// signature verification so malformed nodes are rejected for the cheap
    /// reason and never consume ed25519 verify cycles.
    pub fn apply_remote_batch_checked(&mut self, nodes: Vec<SyncNode>, now_ms: Option<u64>) -> BatchResult {
        let mut result = BatchResult::default();

        // Step 1 + 2: dedupe and hash check.
        let mut to_verify: Vec<SyncNode> = Vec::with_capacity(nodes.len());
        let mut seen_in_batch: HashSet<NodeId> = HashSet::with_capacity(nodes.len());
        for node in nodes {
            if !seen_in_batch.insert(node.id) {
                continue;
            }
            if self.nodes.contains(&node.id) {
                continue;
            }
            let expected = node.transaction.hash();
            if expected != node.id {
                result.rejected.push((node.id, SyncError::HashMismatch { expected, actual: node.id }));
                continue;
            }
            // G5: Lamport ceiling and (optionally) wall-clock skew,
            // pre-crypto.
            if let Err(e) = check_lamport_ceiling(self.lamport, &node) {
                result.rejected.push((node.id, e));
                continue;
            }
            if let Some(now) = now_ms {
                if let Err(e) = check_wall_skew(now, &node) {
                    result.rejected.push((node.id, e));
                    continue;
                }
            }
            to_verify.push(node);
        }

        // Step 3: parallel batched signature verification.
        let verify_ok = self.verify_nodes_batched(&to_verify);

        // Step 4: apply in input order.
        let mut accepted_nodes: Vec<SyncNode> = Vec::new();
        for (node, ok) in to_verify.into_iter().zip(verify_ok) {
            if !ok {
                let id = node.id;
                result.rejected.push((id, SyncError::InvalidSignature(id)));
                continue;
            }
            // Mark verified so any future re-broadcast skips the crypto check.
            self.verified_ids.insert(node.id);
            let id = node.id;
            match self.apply_remote_verified_no_projection(node.clone()) {
                Ok(()) => {
                    result.accepted.push(id);
                    accepted_nodes.push(node);
                }
                Err(e) => result.rejected.push((id, e)),
            }
        }

        self.update_text_projection_from_nodes_batch(&accepted_nodes);

        result
    }

    /// Run batched signature verification over `nodes`. Returns one bool per
    /// input slot in the same order. Zero-signature (legacy/unsigned) nodes
    /// and previously-verified ids are short-circuited to `true`.
    fn verify_nodes_batched(&self, nodes: &[SyncNode]) -> Vec<bool> {
        let mut results = vec![true; nodes.len()];
        let need: Vec<usize> = nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| !n.signature.is_zero() && !self.verified_ids.contains(&n.id))
            .map(|(i, _)| i)
            .collect();

        if need.is_empty() {
            return results;
        }

        // Empirically tuned on AMD Zen 3, 12 physical cores, ed25519-dalek 2.2
        // with the simd backend. Pippenger multi-scalar-mul efficiency climbs
        // with batch size, but so does chunk coarseness (fewer chunks = worse
        // rayon load balance + worse behaviour on small catchups).
        // Sweep on `merge_10k_batch` (10k nodes): 64 → 30.5 ms, 128 → 28.8 ms,
        // 256 → 28.6 ms, 512 → 28.2 ms (within noise of 256).  Past ~256 the
        // crypto curve flattens and the parallelism penalty starts to matter
        // on smaller catchup payloads, so 256 is the current sweet spot.
        const BATCH_VERIFY_CHUNK: usize = 256;

        #[cfg(not(target_arch = "wasm32"))]
        let chunk_outputs: Vec<Vec<(usize, bool)>> = {
            use rayon::prelude::*;
            need.par_chunks(BATCH_VERIFY_CHUNK)
                .map(|chunk| verify_chunk(nodes, chunk))
                .collect()
        };

        #[cfg(target_arch = "wasm32")]
        let chunk_outputs: Vec<Vec<(usize, bool)>> = need
            .chunks(BATCH_VERIFY_CHUNK)
            .map(|chunk| verify_chunk(nodes, chunk))
            .collect();

        for chunk in chunk_outputs {
            for (idx, ok) in chunk {
                results[idx] = ok;
            }
        }
        results
    }

    fn insert_node(&mut self, node: SyncNode) -> Result<(), SyncError> {
        if self.nodes.contains(&node.id) {
            return Err(SyncError::DuplicateNode(node.id));
        }
        // This node's parents are no longer leaves.
        for parent in node.parents() {
            self.leaves.remove(parent);
        }
        self.leaves.insert(node.id);
        // Advance the serializable frontier in lockstep with `leaves`.
        self.frontier.advance(&node);
        // Advance our own Lamport clock past anything we've seen.
        if node.lamport() > self.lamport {
            self.lamport = node.lamport();
        }
        self.nodes.put(node)?;
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Sync / Merkle diffing
    // -------------------------------------------------------------------------

    /// Compute the Merkle root: the Blake3 hash of all sorted leaf IDs.
    ///
    /// Two graphs with the same Merkle root are guaranteed to have identical
    /// state. This is the first value exchanged in the sync handshake.
    pub fn merkle_root(&self) -> Hash {
        let mut sorted: Vec<&Hash> = self.leaves.iter().collect();
        sorted.sort_unstable();
        let mut hasher = blake3::Hasher::new();
        for h in sorted {
            hasher.update(h.as_bytes());
        }
        Hash(*hasher.finalize().as_bytes())
    }

    /// Return the IDs of all nodes we have that the remote is missing.
    ///
    /// `remote_known` is the set of node IDs the remote peer already has
    /// (sent during the handshake). We return everything else.
    pub fn missing_hashes(&self, remote_known: &HashSet<NodeId>) -> Vec<NodeId> {
        self.nodes
            .all_ids()
            .into_iter()
            .filter(|id| !remote_known.contains(id))
            .collect()
    }

    /// Retrieve nodes by ID for transmission to a remote peer.
    pub fn get_nodes(&self, ids: &[NodeId]) -> Vec<&SyncNode> {
        ids.iter().filter_map(|id| self.nodes.get(id)).collect()
    }

    /// Retrieve all nodes currently present in the graph.
    ///
    /// Nodes are returned as owned clones so callers can safely sort/filter
    /// without borrowing the graph storage.
    pub fn all_nodes(&self) -> Vec<SyncNode> {
        self.nodes
            .all_ids()
            .into_iter()
            .filter_map(|id| self.nodes.get(&id).cloned())
            .collect()
    }

    // -------------------------------------------------------------------------
    // State resolution (LWW-Map CRDT)
    // -------------------------------------------------------------------------

    /// Incrementally fold one admitted node into the materialized map/list/
    /// blob caches. `canonical` is false only for locally-authored
    /// (unconfirmed) nodes, mirroring the `local_node_ids` read-time filter
    /// the full-replay resolver used.
    fn update_state_caches(&mut self, node: &SyncNode, canonical: bool) {
        let tx = &node.transaction;
        let prio = (tx.lamport, tx.author);
        // Replicates the replay scan's strict-greater rule: the replay
        // default entry was `(0, [0u8; 32], None)`, so an op carrying that
        // exact priority never won there either.
        if prio == (0, [0u8; 32]) {
            return;
        }
        for op in &tx.ops {
            match op {
                Op::Map(mop) => {
                    let (key, value, is_blob) = match mop {
                        MapOp::Set { key, value } => (key, Some(value.clone()), false),
                        MapOp::Delete { key } => (key, None, false),
                        MapOp::SetBlob { key, blob_hash } => {
                            self.state_caches.blob_hashes.insert(*blob_hash);
                            (key, Some(blob_hash.as_bytes().to_vec()), true)
                        }
                    };
                    let candidate = MapWinner {
                        lamport: tx.lamport,
                        author: tx.author,
                        value,
                        is_blob,
                    };
                    if self.state_caches.conflict_stream_enabled {
                        if let Some(current) = self.state_caches.map_all.get(key) {
                            if current.author != tx.author {
                                let event = if prio > (current.lamport, current.author) {
                                    ConflictEvent {
                                        kind: ConflictKind::MapOverwrite,
                                        key: key.clone(),
                                        winner_author: tx.author,
                                        winner_lamport: tx.lamport,
                                        winner_op: candidate.conflict_op(),
                                        loser_author: current.author,
                                        loser_lamport: current.lamport,
                                        loser_op: current.conflict_op(),
                                    }
                                } else {
                                    ConflictEvent {
                                        kind: ConflictKind::MapOverwrite,
                                        key: key.clone(),
                                        winner_author: current.author,
                                        winner_lamport: current.lamport,
                                        winner_op: current.conflict_op(),
                                        loser_author: tx.author,
                                        loser_lamport: tx.lamport,
                                        loser_op: candidate.conflict_op(),
                                    }
                                };
                                Self::push_pending_conflict(
                                    &mut self.state_caches.pending_conflicts,
                                    event,
                                );
                            }
                        }
                    }
                    Self::lww_update(&mut self.state_caches.map_all, key, &candidate);
                    if canonical {
                        Self::lww_update(&mut self.state_caches.map_canonical, key, &candidate);
                    }
                }
                Op::List(lop) => {
                    self.update_list_conflict_state(lop, prio);
                    Self::update_list_cache(&mut self.state_caches.lists, lop, prio);
                }
                Op::Text(_) => {}
            }
        }
    }

    fn lww_update(cache: &mut HashMap<String, MapWinner>, key: &str, candidate: &MapWinner) {
        match cache.get_mut(key) {
            Some(current) => {
                if (candidate.lamport, candidate.author) > (current.lamport, current.author) {
                    *current = candidate.clone();
                }
            }
            None => {
                cache.insert(key.to_string(), candidate.clone());
            }
        }
    }

    fn push_pending_conflict(pending: &mut Vec<ConflictEvent>, event: ConflictEvent) {
        if pending.len() >= PENDING_CONFLICTS_CAP {
            let overflow = pending.len() + 1 - PENDING_CONFLICTS_CAP;
            pending.drain(0..overflow);
        }
        pending.push(event);
    }

    /// Mirror of `crate::list::resolve_list_seq`'s per-op state machine,
    /// applied incrementally with the visible ordered set maintained inline.
    fn update_list_cache(
        lists: &mut HashMap<String, ListCacheState>,
        lop: &ListOp,
        prio: (u64, [u8; 32]),
    ) {
        let (list_key, item_id) = match lop {
            ListOp::Insert { list_key, item_id, .. }
            | ListOp::Move { list_key, item_id, .. }
            | ListOp::Delete { list_key, item_id } => (list_key, *item_id),
        };
        let cache = lists.entry(list_key.clone()).or_default();
        let entry = cache.items.entry(item_id).or_default();
        let was_visible_pos = if entry.is_visible() {
            entry.position.clone()
        } else {
            None
        };

        match lop {
            ListOp::Insert { position, .. } => {
                entry.saw_insert = true;
                if prio > entry.pos_priority {
                    entry.position = Some(position.clone());
                    entry.pos_priority = prio;
                }
            }
            ListOp::Move { position, .. } => {
                if prio > entry.pos_priority {
                    entry.position = Some(position.clone());
                    entry.pos_priority = prio;
                }
            }
            ListOp::Delete { .. } => {
                entry.deleted = true;
            }
        }

        let now_visible_pos = if entry.is_visible() {
            entry.position.clone()
        } else {
            None
        };
        if was_visible_pos != now_visible_pos {
            if let Some(old_pos) = was_visible_pos {
                cache.visible.remove(&(old_pos, item_id));
            }
            if let Some(new_pos) = now_visible_pos {
                cache.visible.insert((new_pos, item_id));
            }
        }
    }

    /// Incremental list-conflict bookkeeping (only when the conflict stream
    /// is enabled). Mirrors `detect_list_conflicts` pairings: losing
    /// position ops from other authors, and position ops absorbed by a
    /// delete.
    fn update_list_conflict_state(&mut self, lop: &ListOp, prio: (u64, [u8; 32])) {
        if !self.state_caches.conflict_stream_enabled {
            return;
        }
        let (lamport, author) = prio;
        match lop {
            ListOp::Insert { list_key, item_id, .. } | ListOp::Move { list_key, item_id, .. } => {
                let cop = match lop {
                    ListOp::Insert { .. } => ConflictOp::ListInsert { item_id: item_id.0 },
                    _ => ConflictOp::ListMove { item_id: item_id.0 },
                };
                let st = self
                    .state_caches
                    .list_conflict_state
                    .entry((list_key.clone(), *item_id))
                    .or_default();
                let mut event = None;
                if let Some((dl, da)) = st.delete_winner {
                    if da != author {
                        event = Some(ConflictEvent {
                            kind: ConflictKind::ListDeleteWon,
                            key: format!("{}#{}", list_key, item_id.to_hex()),
                            winner_author: da,
                            winner_lamport: dl,
                            winner_op: ConflictOp::ListDelete { item_id: item_id.0 },
                            loser_author: author,
                            loser_lamport: lamport,
                            loser_op: cop.clone(),
                        });
                    }
                } else if let Some((wl, wa, wop)) = &st.pos_winner {
                    if *wa != author {
                        event = Some(if prio > (*wl, *wa) {
                            ConflictEvent {
                                kind: ConflictKind::ListMoveLost,
                                key: format!("{}#{}", list_key, item_id.to_hex()),
                                winner_author: author,
                                winner_lamport: lamport,
                                winner_op: cop.clone(),
                                loser_author: *wa,
                                loser_lamport: *wl,
                                loser_op: wop.clone(),
                            }
                        } else {
                            ConflictEvent {
                                kind: ConflictKind::ListMoveLost,
                                key: format!("{}#{}", list_key, item_id.to_hex()),
                                winner_author: *wa,
                                winner_lamport: *wl,
                                winner_op: wop.clone(),
                                loser_author: author,
                                loser_lamport: lamport,
                                loser_op: cop.clone(),
                            }
                        });
                    }
                }
                if st.pos_ops.len() < LIST_CONFLICT_POS_OPS_CAP {
                    st.pos_ops.push((lamport, author, cop.clone()));
                }
                if st
                    .pos_winner
                    .as_ref()
                    .map(|(l, a, _)| prio > (*l, *a))
                    .unwrap_or(true)
                {
                    st.pos_winner = Some((lamport, author, cop));
                }
                if let Some(event) = event {
                    Self::push_pending_conflict(&mut self.state_caches.pending_conflicts, event);
                }
            }
            ListOp::Delete { list_key, item_id } => {
                let st = self
                    .state_caches
                    .list_conflict_state
                    .entry((list_key.clone(), *item_id))
                    .or_default();
                let mut events = Vec::new();
                if st.delete_winner.is_none() {
                    for (pl, pa, pop) in &st.pos_ops {
                        if *pa == author {
                            continue;
                        }
                        events.push(ConflictEvent {
                            kind: ConflictKind::ListDeleteWon,
                            key: format!("{}#{}", list_key, item_id.to_hex()),
                            winner_author: author,
                            winner_lamport: lamport,
                            winner_op: ConflictOp::ListDelete { item_id: item_id.0 },
                            loser_author: *pa,
                            loser_lamport: *pl,
                            loser_op: pop.clone(),
                        });
                    }
                }
                if st
                    .delete_winner
                    .map(|(l, a)| prio > (l, a))
                    .unwrap_or(true)
                {
                    st.delete_winner = Some((lamport, author));
                }
                for event in events {
                    Self::push_pending_conflict(&mut self.state_caches.pending_conflicts, event);
                }
            }
        }
    }

    /// Enable/disable the incremental conflict stream (see
    /// [`drain_pending_conflicts`](Self::drain_pending_conflicts)).
    /// Disabled by default; hosts that poll per-import enable it once at
    /// room creation. Events are only recorded for ops applied while
    /// enabled.
    pub fn set_conflict_stream_enabled(&mut self, enabled: bool) {
        self.state_caches.conflict_stream_enabled = enabled;
    }

    /// Drain conflict events recorded since the last drain.
    ///
    /// Unlike [`detect_conflicts`](Self::detect_conflicts) (a full O(history)
    /// recomputation that re-pairs every historical loser against the
    /// current winner on each call), this is O(new ops) and emits each
    /// pairing once, at the moment the losing/demoting op is applied.
    pub fn drain_pending_conflicts(&mut self) -> Vec<ConflictEvent> {
        std::mem::take(&mut self.state_caches.pending_conflicts)
    }

    /// Resolve the current value of the shared map (LWW winner per key).
    ///
    /// Returns the **speculative** view — includes local (unconfirmed) writes.
    /// Use `resolve_canonical()` for confirmed-only state (E2). Served from
    /// the incrementally-maintained winner cache: O(keys), not O(history).
    pub fn resolve(&self) -> ResolvedMap {
        self.state_caches
            .map_all
            .iter()
            .filter_map(|(k, w)| w.value.clone().map(|v| (k.clone(), v)))
            .collect()
    }

    /// Canonical state: like `resolve()` but excludes nodes that were authored
    /// locally on this peer and have not yet been confirmed by a remote peer.
    ///
    /// In Authoritative mode (E1), the server re-broadcasts signed canonical
    /// nodes; once they arrive via `apply_remote` they enter the canonical view.
    /// In AllowAll / Cooperative mode, all remote writes are canonical.
    pub fn resolve_canonical(&self) -> ResolvedMap {
        self.state_caches
            .map_canonical
            .iter()
            .filter_map(|(k, w)| w.value.clone().map(|v| (k.clone(), v)))
            .collect()
    }

    /// Per-key read from the speculative view (local + remote nodes). O(1).
    pub fn read_speculative(&self, key: &str) -> Option<Vec<u8>> {
        self.state_caches
            .map_all
            .get(key)
            .and_then(|w| w.value.clone())
    }

    /// Per-key read from the canonical view (remote nodes only). O(1).
    pub fn read_canonical(&self, key: &str) -> Option<Vec<u8>> {
        self.state_caches
            .map_canonical
            .get(key)
            .and_then(|w| w.value.clone())
    }

    /// Full-replay LWW scan — retained as the parity oracle for the
    /// incremental winner caches (tests assert cache == replay). When
    /// `skip_local` is true, nodes whose IDs are in `local_node_ids` are
    /// excluded (canonical view).
    #[cfg(test)]
    fn resolve_replay(&self, skip_local: bool) -> ResolvedMap {
        let mut per_key: BTreeMap<String, (u64, [u8; 32], Option<Vec<u8>>)> = BTreeMap::new();

        let node_ids = self.nodes.all_ids();
        for id in &node_ids {
            if skip_local && self.local_node_ids.contains(id) {
                continue; // exclude unconfirmed local writes from canonical view
            }
            let Some(node) = self.nodes.get(id) else { continue };
            let tx = &node.transaction;
            for op in &tx.ops {
                match op {
                    Op::Map(MapOp::Set { key, value }) => {
                        let entry = per_key.entry(key.clone()).or_insert((0, [0u8; 32], None));
                        if (tx.lamport, tx.author) > (entry.0, entry.1) {
                            *entry = (tx.lamport, tx.author, Some(value.clone()));
                        }
                    }
                    Op::Map(MapOp::Delete { key }) => {
                        let entry = per_key.entry(key.clone()).or_insert((0, [0u8; 32], None));
                        if (tx.lamport, tx.author) > (entry.0, entry.1) {
                            *entry = (tx.lamport, tx.author, None);
                        }
                    }
                    Op::Map(MapOp::SetBlob { key, blob_hash }) => {
                        let entry = per_key.entry(key.clone()).or_insert((0, [0u8; 32], None));
                        if (tx.lamport, tx.author) > (entry.0, entry.1) {
                            *entry = (tx.lamport, tx.author, Some(blob_hash.as_bytes().to_vec()));
                        }
                    }
                    Op::List(_) | Op::Text(_) => {} // stubs — Phase C
                }
            }
        }

        per_key
            .into_iter()
            .filter_map(|(key, (_, _, value))| value.map(|v| (key, v)))
            .collect()
    }

    /// Like `resolve()` but also returns the winning Lamport timestamp,
    /// author bytes, and a flag indicating whether the value is a blob hash
    /// (`is_blob = true`) or inline bytes (`is_blob = false`).
    ///
    /// When `is_blob` is true, `value` holds the 32-byte Blake3 blob hash;
    /// callers must look that up in the local `BlobStore` to get actual bytes.
    /// Served from the incremental winner cache: O(keys), not O(history).
    pub fn resolve_with_meta(&self) -> HashMap<String, (u64, [u8; 32], Vec<u8>, bool)> {
        self.state_caches
            .map_all
            .iter()
            .filter_map(|(k, w)| {
                w.value
                    .clone()
                    .map(|v| (k.clone(), (w.lamport, w.author, v, w.is_blob)))
            })
            .collect()
    }

    /// Return the set of all blob hashes referenced by `SetBlob` ops in the
    /// graph. Used by the bridge to determine which blobs must be fetched
    /// from peers. Served from the incremental cache: O(hashes).
    pub fn referenced_blob_hashes(&self) -> HashSet<Hash> {
        self.state_caches.blob_hashes.clone()
    }

    // -------------------------------------------------------------------------
    // Accessors
    // -------------------------------------------------------------------------

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn current_lamport(&self) -> u64 {
        self.lamport
    }

    pub fn leaf_ids(&self) -> &HashSet<NodeId> {
        &self.leaves
    }

    pub fn all_node_ids(&self) -> Vec<NodeId> {
        self.nodes.all_ids()
    }

    /// Return the current frontier (serializable DAG tips).
    pub fn frontier(&self) -> Frontier {
        self.frontier.clone()
    }

    // -------------------------------------------------------------------------
    // C1: RGA Collaborative Text
    // -------------------------------------------------------------------------

    /// Resolve the visible RGA character sequence for `key`.
    ///
    /// Returns `(OpId, char)` pairs in sequence order.  Tombstoned characters
    /// are excluded from the output but their IDs remain valid anchors for
    /// future insertions around them.
    pub fn resolve_text_seq(&self, key: &str) -> Vec<(crate::op::OpId, char)> {
        #[cfg(not(feature = "text_projection"))]
        {
            self.text_replay_fallbacks.fetch_add(1, Ordering::Relaxed);
            return self.resolve_text_seq_legacy(key);
        }

        #[cfg(feature = "text_projection")]
        match self.text_projection_mode {
            TextProjectionMode::Disabled => {
                self.text_replay_fallbacks.fetch_add(1, Ordering::Relaxed);
                self.resolve_text_seq_legacy(key)
            }
            TextProjectionMode::Enabled | TextProjectionMode::ParityCheck => self
                .text_projections
                .get(key)
                .map(|projection| {
                    self.text_projection_hits.fetch_add(1, Ordering::Relaxed);
                    projection.resolve_seq()
                })
                .unwrap_or_else(|| {
                    self.text_projection_hits.fetch_add(1, Ordering::Relaxed);
                    self.resolve_projection_transient(key).resolve_seq()
                }),
        }
    }

    /// Alias of `resolve_text_seq` — kept for bridge call-sites that want
    /// the full `(OpId, char)` pairs explicitly named.
    pub fn resolve_text_seq_with_chars(&self, key: &str) -> Vec<(crate::op::OpId, char)> {
        self.resolve_text_seq(key)
    }

    /// Resolve the RGA text for `key` as a plain UTF-8 string.
    pub fn resolve_text(&self, key: &str) -> String {
        #[cfg(not(feature = "text_projection"))]
        {
            self.text_replay_fallbacks.fetch_add(1, Ordering::Relaxed);
            return self.resolve_text_legacy(key);
        }

        #[cfg(feature = "text_projection")]
        match self.text_projection_mode {
            TextProjectionMode::Disabled => {
                self.text_replay_fallbacks.fetch_add(1, Ordering::Relaxed);
                self.resolve_text_legacy(key)
            }
            TextProjectionMode::Enabled | TextProjectionMode::ParityCheck => self
                .text_projections
                .get(key)
                .map(|projection| {
                    self.text_projection_hits.fetch_add(1, Ordering::Relaxed);
                    projection.resolve_string()
                })
                .unwrap_or_else(|| {
                    self.text_projection_hits.fetch_add(1, Ordering::Relaxed);
                    self.resolve_projection_transient(key).resolve_string()
                }),
        }
    }

    /// Resolve canonical text for `key` via replay semantics.
    ///
    /// This accessor is intended for persistence/export/audit flows that
    /// require canonical replay output independent of runtime projection mode.
    pub fn resolve_text_canonical(&self, key: &str) -> String {
        self.resolve_text_legacy(key)
    }

    /// Visible RGA sequence replayed from DAG nodes with `transaction.lamport <= max_lamport`.
    ///
    /// Always uses canonical replay (not the live projection cache) so scrubbing
    /// reflects tombstones and concurrent edits correctly.
    pub fn resolve_text_seq_at_lamport(
        &self,
        key: &str,
        max_lamport: u64,
    ) -> Vec<(crate::op::OpId, char)> {
        self.resolve_text_seq_legacy_at_lamport(key, max_lamport)
    }

    /// Plain UTF-8 text at lamport `max_lamport` (see [`Self::resolve_text_seq_at_lamport`]).
    pub fn resolve_text_at_lamport(&self, key: &str, max_lamport: u64) -> String {
        self.resolve_text_legacy_at_lamport(key, max_lamport)
    }

    /// Phase 2.5 skeleton: resolve a window of text by character offset.
    ///
    /// `start` and `len` are measured in Unicode scalar values for now.
    /// This API shape is intentionally simple and will be refined as
    /// offset/anchor semantics are finalized.
    pub fn resolve_text_range(&self, key: &str, start: usize, len: usize) -> String {
        if len == 0 {
            return String::new();
        }
        #[cfg(not(feature = "text_projection"))]
        {
            return self
                .resolve_text(key)
                .chars()
                .skip(start)
                .take(len)
                .collect();
        }

        #[cfg(feature = "text_projection")]
        {
            match self.text_projection_mode {
                TextProjectionMode::Disabled => self
                    .resolve_text(key)
                    .chars()
                    .skip(start)
                    .take(len)
                    .collect(),
                TextProjectionMode::Enabled | TextProjectionMode::ParityCheck => self
                    .text_projections
                    .get(key)
                    .map(|projection| projection.resolve_string_range(start, len))
                    .unwrap_or_else(|| self.resolve_projection_transient(key).resolve_string_range(start, len)),
            }
        }
    }

    /// Phase 3 scaffold: map a visible character offset to a stable anchor.
    pub fn resolve_text_anchor_for_offset(&self, key: &str, offset: usize) -> TextRangeAnchor {
        #[cfg(not(feature = "text_projection"))]
        {
            let seq = self.resolve_text_seq_with_chars(key);
            if offset == 0 {
                return TextRangeAnchor::Start;
            }
            if offset >= seq.len() {
                return TextRangeAnchor::End;
            }
            return TextRangeAnchor::After(seq[offset - 1].0);
        }

        #[cfg(feature = "text_projection")]
        {
            match self.text_projection_mode {
                TextProjectionMode::Enabled | TextProjectionMode::ParityCheck => self
                    .text_projections
                    .get(key)
                    .map(|projection| projection.anchor_for_offset(offset))
                    .unwrap_or_else(|| self.resolve_projection_transient(key).anchor_for_offset(offset)),
                TextProjectionMode::Disabled => {
                    let seq = self.resolve_text_seq_with_chars(key);
                    if offset == 0 {
                        TextRangeAnchor::Start
                    } else if offset >= seq.len() {
                        TextRangeAnchor::End
                    } else {
                        TextRangeAnchor::After(seq[offset - 1].0)
                    }
                }
            }
        }
    }

    /// Phase 3 scaffold: map an anchor to a visible character offset.
    pub fn resolve_text_offset_for_anchor(&self, key: &str, anchor: TextRangeAnchor) -> usize {
        #[cfg(not(feature = "text_projection"))]
        {
            let seq = self.resolve_text_seq_with_chars(key);
            return match anchor {
                TextRangeAnchor::Start => 0,
                TextRangeAnchor::End => seq.len(),
                TextRangeAnchor::Offset(i) => i.min(seq.len()),
                TextRangeAnchor::After(id) => seq
                    .iter()
                    .position(|(existing, _)| *existing == id)
                    .map(|i| i + 1)
                    .unwrap_or(seq.len()),
            };
        }

        #[cfg(feature = "text_projection")]
        {
            match self.text_projection_mode {
                TextProjectionMode::Enabled | TextProjectionMode::ParityCheck => self
                    .text_projections
                    .get(key)
                    .map(|projection| projection.offset_for_anchor(anchor))
                    .unwrap_or_else(|| self.resolve_projection_transient(key).offset_for_anchor(anchor)),
                TextProjectionMode::Disabled => {
                    let seq = self.resolve_text_seq_with_chars(key);
                    match anchor {
                        TextRangeAnchor::Start => 0,
                        TextRangeAnchor::End => seq.len(),
                        TextRangeAnchor::Offset(i) => i.min(seq.len()),
                        TextRangeAnchor::After(id) => seq
                            .iter()
                            .position(|(existing, _)| *existing == id)
                            .map(|i| i + 1)
                            .unwrap_or(seq.len()),
                    }
                }
            }
        }
    }

    // -------------------------------------------------------------------------
    // List resolution (F8 — fractional-index list CRDT)
    // -------------------------------------------------------------------------

    /// Resolve the visible items of `list_key` as `(ItemId, FracIdx)` pairs
    /// in current order. Tombstoned items are excluded; concurrent moves
    /// resolve via LWW on `(lamport, author)` per item id.
    ///
    /// Item *content* lives in the sidecar Map keyed by hex(item_id) — the
    /// SDK composes the two; core stays neutral.
    pub fn resolve_list(
        &self,
        list_key: &str,
    ) -> Vec<(crate::op::ItemId, crate::list::FracIdx)> {
        self.state_caches
            .lists
            .get(list_key)
            .map(|cache| {
                cache
                    .visible
                    .iter()
                    .map(|(pos, id)| (*id, pos.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Full-replay list resolution — retained as the parity oracle for the
    /// incremental list cache (tests assert cache == replay).
    #[cfg(test)]
    fn resolve_list_replay(
        &self,
        list_key: &str,
    ) -> Vec<(crate::op::ItemId, crate::list::FracIdx)> {
        let node_ids = self.nodes.all_ids();
        let nodes: Vec<&SyncNode> = node_ids
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .collect();
        crate::list::resolve_list_seq(&nodes, list_key)
    }

    // -------------------------------------------------------------------------
    // G9: conflict surfacing
    // -------------------------------------------------------------------------

    /// Re-derive the full set of LWW losers in the current graph state.
    ///
    /// Pure, deterministic, and idempotent — same node set produces the
    /// same `Vec<ConflictEvent>` in the same order. The bridge layer
    /// dedups across calls so a conflict is delivered to the SDK once.
    pub fn detect_conflicts(&self) -> Vec<crate::conflicts::ConflictEvent> {
        let node_ids = self.nodes.all_ids();
        let nodes: Vec<&SyncNode> = node_ids
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .collect();
        crate::conflicts::detect_conflicts(&nodes)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn key_a() -> SigningKey { SigningKey::from_bytes(&[0x0Au8; 32]) }
    fn key_b() -> SigningKey { SigningKey::from_bytes(&[0x0Bu8; 32]) }

    fn set(key: &str, val: &str) -> Op {
        Op::Map(MapOp::Set { key: key.into(), value: val.as_bytes().to_vec() })
    }

    /// G5 regression — `apply_local` must NEVER fold `wall_ms` into the
    /// Lamport counter. A pre-G5 implementation used
    /// `self.lamport = (self.lamport + 1).max(wall_ms)` which, combined
    /// with the browser passing `Date.now()` (~1.77e12) as `wall_ms`,
    /// produced nodes that blew past `LAMPORT_SLACK` on the very first
    /// local write and were then rejected by every peer with a
    /// `LamportCeiling` error. This test locks the fix in: Lamport is a
    /// pure logical clock, even when `wall_ms` is astronomical.
    #[test]
    fn apply_local_ignores_wall_ms_for_lamport() {
        let mut g = StateGraph::new();
        // Realistic JS `Date.now()` value (≈ 2026-04-24 in ms since epoch).
        let wall = 1_777_000_000_000u64;
        let id = g.apply_local(&key_a(), wall, vec![set("k", "v")]).unwrap();
        let node = g.get_nodes(&[id]).into_iter().next().unwrap();
        assert_eq!(node.transaction.lamport, 1, "lamport must be pure logical counter");
        assert_eq!(node.transaction.wall_ms, wall, "wall_ms must be preserved as informational");
        assert_eq!(g.lamport(), 1);
        // A second local write still increments by exactly 1.
        let id2 = g.apply_local(&key_a(), wall + 1, vec![set("k", "v2")]).unwrap();
        let node2 = g.get_nodes(&[id2]).into_iter().next().unwrap();
        assert_eq!(node2.transaction.lamport, 2);
        assert!(g.lamport() < LAMPORT_SLACK, "local clock must stay well under G5 ceiling");
    }

    #[test]
    fn basic_set_and_resolve() {
        let mut g = StateGraph::new();
        g.apply_local(&key_a(), 0, vec![set("name", "Alice")]).unwrap();
        let state = g.resolve();
        assert_eq!(state.get("name").map(|v| v.as_slice()), Some(b"Alice".as_slice()));
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_projection_parity_mismatch_triggers_self_heal() {
        let sk = key_a();
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::ParityCheck);
        g.set_text_parity_sample_every(1);

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
        assert_eq!(g.text_projection_self_heal_count(), 0);

        // Force a projection mismatch so parity mode exercises self-heal.
        g.text_projections
            .insert("doc".into(), crate::text::TextProjection::default());

        g.apply_local(
            &sk,
            1001,
            vec![Op::Text(TextOp::Insert {
                key: "doc".into(),
                after: None,
                ch: 'b',
            })],
        )
        .unwrap();

        assert!(
            g.text_projection_self_heal_count() >= 1,
            "parity mismatch should trigger at least one self-heal rebuild"
        );
        assert!(
            !g.text_projection_mismatches().is_empty(),
            "parity mismatch diagnostics should be captured"
        );
        assert_eq!(
            g.resolve_text("doc"),
            g.resolve_text_legacy("doc"),
            "projection output should match replay output after self-heal"
        );
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_projection_parity_sampling_every_n_updates() {
        let sk = key_a();
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::ParityCheck);
        g.set_text_parity_sample_every(2);

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
        assert_eq!(g.text_parity_check_runs(), 0);

        let id_a = crate::op::OpId {
            lamport: 1,
            author: sk.verifying_key().to_bytes(),
        };
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
        assert_eq!(g.text_parity_check_runs(), 1);

        let id_b = crate::op::OpId {
            lamport: 2,
            author: sk.verifying_key().to_bytes(),
        };
        g.apply_local(
            &sk,
            1002,
            vec![Op::Text(TextOp::Insert {
                key: "doc".into(),
                after: Some(id_b),
                ch: 'c',
            })],
        )
        .unwrap();
        assert_eq!(g.text_parity_check_runs(), 1);
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_projection_parity_selected_keys_filter_touches() {
        let sk = key_a();
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::ParityCheck);
        g.set_text_parity_sample_every(1);
        g.set_text_parity_selected_keys(vec!["doc".into()]);

        g.apply_local(
            &sk,
            1000,
            vec![Op::Text(TextOp::Insert {
                key: "other".into(),
                after: None,
                ch: 'x',
            })],
        )
        .unwrap();
        assert_eq!(g.text_parity_check_runs(), 0);

        g.apply_local(
            &sk,
            1001,
            vec![Op::Text(TextOp::Insert {
                key: "doc".into(),
                after: None,
                ch: 'y',
            })],
        )
        .unwrap();
        assert_eq!(g.text_parity_check_runs(), 1);
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_projection_parity_deterministic_corpus_has_zero_mismatches() {
        let sk = key_a();
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::ParityCheck);
        g.set_text_parity_sample_every(1);

        // Deterministic corpus mixing char and range operations across keys.
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

        let id_a = crate::op::OpId {
            lamport: 1,
            author: sk.verifying_key().to_bytes(),
        };
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

        g.apply_local(
            &sk,
            1002,
            vec![Op::Text(TextOp::InsertRange {
                key: "doc".into(),
                anchor: TextRangeAnchor::End,
                text: "cd".into(),
            })],
        )
        .unwrap();

        g.apply_local(
            &sk,
            1003,
            vec![Op::Text(TextOp::Delete {
                key: "doc".into(),
                target: id_a,
            })],
        )
        .unwrap();

        g.apply_local(
            &sk,
            1004,
            vec![Op::Text(TextOp::InsertRange {
                key: "title".into(),
                anchor: TextRangeAnchor::Start,
                text: "Hi".into(),
            })],
        )
        .unwrap();

        g.apply_local(
            &sk,
            1005,
            vec![Op::Text(TextOp::DeleteRange {
                key: "doc".into(),
                anchor: TextRangeAnchor::Offset(1),
                len_chars: 1,
            })],
        )
        .unwrap();

        assert!(
            g.text_parity_check_runs() >= 6,
            "parity should have run for each deterministic corpus update"
        );
        assert!(
            g.text_projection_mismatches().is_empty(),
            "deterministic corpus should not produce parity mismatches"
        );
        assert_eq!(
            g.text_projection_self_heal_count(),
            0,
            "zero mismatches should imply zero parity self-heals"
        );
        assert_eq!(g.resolve_text("doc"), g.resolve_text_legacy("doc"));
        assert_eq!(g.resolve_text("title"), g.resolve_text_legacy("title"));
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_projection_randomized_trace_matches_legacy_deterministically() {
        let sk = key_a();
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::Enabled);

        let key = "doc";
        let mut visible_ids: Vec<crate::op::OpId> = Vec::new();

        // Deterministic LCG so this corpus is stable across runs.
        let mut seed: u64 = 0xC0FFEE_1234_5678;
        let mut next_u64 = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            seed
        };

        for step in 0..300u64 {
            let do_insert = visible_ids.is_empty() || (next_u64() % 100) < 70;

            if do_insert {
                let pos = if visible_ids.is_empty() {
                    0usize
                } else {
                    (next_u64() as usize) % (visible_ids.len() + 1)
                };
                let after = if pos == 0 {
                    None
                } else {
                    Some(visible_ids[pos - 1])
                };
                let ch = (b'a' + (next_u64() % 26) as u8) as char;

                g.apply_local(
                    &sk,
                    10_000 + step,
                    vec![Op::Text(TextOp::Insert {
                        key: key.into(),
                        after,
                        ch,
                    })],
                )
                .unwrap();

                let inserted = crate::op::OpId {
                    lamport: g.lamport(),
                    author: sk.verifying_key().to_bytes(),
                };
                visible_ids.insert(pos, inserted);
            } else {
                let pos = (next_u64() as usize) % visible_ids.len();
                let target = visible_ids.remove(pos);
                g.apply_local(
                    &sk,
                    10_000 + step,
                    vec![Op::Text(TextOp::Delete {
                        key: key.into(),
                        target,
                    })],
                )
                .unwrap();
            }

            let projected = g.resolve_text(key);
            let legacy = g.resolve_text_legacy(key);
            assert_eq!(
                projected, legacy,
                "projection/legacy mismatch at randomized step {step}"
            );
        }
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_runtime_counters_track_hits_and_fallbacks() {
        let sk = key_a();
        let mut g = StateGraph::new();

        g.apply_local(
            &sk,
            1000,
            vec![Op::Text(TextOp::Insert {
                key: "doc".into(),
                after: None,
                ch: 'x',
            })],
        )
        .unwrap();

        g.set_text_projection_mode(TextProjectionMode::Disabled);
        g.reset_text_runtime_counters();
        let _ = g.resolve_text("doc");
        let c1 = g.text_runtime_counters();
        assert_eq!(c1.projection_hits, 0);
        assert_eq!(c1.replay_fallbacks, 1);

        g.set_text_projection_mode(TextProjectionMode::Enabled);
        g.reset_text_runtime_counters();
        let _ = g.resolve_text("doc");
        let c2 = g.text_runtime_counters();
        assert_eq!(c2.projection_hits, 1);
        assert_eq!(c2.replay_fallbacks, 0);
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_runtime_temperature_tiers_transition() {
        let sk = key_a();
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::Enabled);
        g.set_text_runtime_temperature_thresholds(TextRuntimeTemperatureThresholds {
            hot_read_calls: 4,
            hot_write_calls: 100,
        });

        assert_eq!(
            g.text_runtime_temperature_for_key("missing"),
            TextRuntimeTemperature::Cold,
            "keys without resident projection should be Cold"
        );

        g.apply_local(
            &sk,
            1000,
            vec![Op::Text(TextOp::Insert {
                key: "doc".into(),
                after: None,
                ch: 'x',
            })],
        )
        .unwrap();

        assert_eq!(
            g.text_runtime_temperature_for_key("doc"),
            TextRuntimeTemperature::Warm,
            "resident projection with low activity should be Warm"
        );

        for _ in 0..4 {
            let _ = g.resolve_text("doc");
        }

        assert_eq!(
            g.text_runtime_temperature_for_key("doc"),
            TextRuntimeTemperature::Hot,
            "repeated reads should promote Warm key to Hot"
        );
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_runtime_temperature_snapshot_reports_resident_keys() {
        let sk = key_a();
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::Enabled);
        g.set_text_runtime_temperature_thresholds(TextRuntimeTemperatureThresholds {
            hot_read_calls: 1,
            hot_write_calls: 10,
        });

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
        g.apply_local(
            &sk,
            1001,
            vec![Op::Text(TextOp::Insert {
                key: "title".into(),
                after: None,
                ch: 'b',
            })],
        )
        .unwrap();

        let _ = g.resolve_text("doc");
        let snapshot = g.text_runtime_temperature_snapshot();

        assert_eq!(snapshot.get("doc"), Some(&TextRuntimeTemperature::Hot));
        assert_eq!(snapshot.get("title"), Some(&TextRuntimeTemperature::Warm));
        assert!(!snapshot.contains_key("missing"));
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_projection_residency_policy_evicts_oldest_warm_key() {
        let sk = key_a();
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::Enabled);
        g.set_text_runtime_temperature_thresholds(TextRuntimeTemperatureThresholds {
            hot_read_calls: u64::MAX,
            hot_write_calls: u64::MAX,
        });
        g.set_text_projection_residency_policy(TextProjectionResidencyPolicy {
            max_resident_keys: 1,
        });

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
        assert!(g.text_projection_debug_stats("doc").is_some());

        g.apply_local(
            &sk,
            1001,
            vec![Op::Text(TextOp::Insert {
                key: "title".into(),
                after: None,
                ch: 'b',
            })],
        )
        .unwrap();

        assert_eq!(g.text_projection_resident_key_count(), 1);
        assert!(
            g.text_projection_debug_stats("doc").is_none(),
            "oldest warm key should be evicted under max_resident_keys=1"
        );
        assert!(g.text_projection_debug_stats("title").is_some());
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_projection_demote_and_rebuild_api_roundtrip() {
        let sk = key_a();
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::Enabled);

        g.apply_local(
            &sk,
            1000,
            vec![Op::Text(TextOp::Insert {
                key: "doc".into(),
                after: None,
                ch: 'x',
            })],
        )
        .unwrap();
        assert!(g.text_projection_debug_stats("doc").is_some());

        assert!(g.demote_text_projection_key("doc"));
        assert!(g.text_projection_debug_stats("doc").is_none());

        assert!(g.ensure_text_projection_resident("doc"));
        assert!(g.text_projection_debug_stats("doc").is_some());
        assert_eq!(g.resolve_text("doc"), "x");
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_projection_rebuild_from_oplog_reproduces_identical_run_materialization_outputs() {
        let sk = key_a();
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::Enabled);

        let key = "doc";

        // Build a deterministic mixed trace that exercises split/merge behavior.
        for i in 0..14u64 {
            let after = g.resolve_text_seq_with_chars(key).last().map(|(id, _)| *id);
            let ch = (b'a' + (i % 26) as u8) as char;
            g.apply_local(
                &sk,
                1000 + i,
                vec![Op::Text(TextOp::Insert {
                    key: key.into(),
                    after,
                    ch,
                })],
            )
            .unwrap();
        }

        for (wall, anchor_idx, ch) in [
            (3000u64, 2usize, 'X'),
            (3001u64, 2usize, 'Y'),
            (3002u64, 8usize, 'Z'),
            (3003u64, 5usize, 'Q'),
        ] {
            let anchors = g.resolve_text_seq_with_chars(key);
            let after = Some(anchors[anchor_idx.min(anchors.len().saturating_sub(1))].0);
            g.apply_local(
                &sk,
                wall,
                vec![Op::Text(TextOp::Insert {
                    key: key.into(),
                    after,
                    ch,
                })],
            )
            .unwrap();
        }

        // Delete a stable subset by current visible order to mix visible/tombstoned layout.
        let visible_before_deletes = g.resolve_text_seq_with_chars(key);
        for (step, (target, _)) in visible_before_deletes
            .iter()
            .copied()
            .enumerate()
            .filter(|(idx, _)| idx % 5 == 1)
            .take(4)
        {
            g.apply_local(
                &sk,
                4000 + step as u64,
                vec![Op::Text(TextOp::Delete {
                    key: key.into(),
                    target,
                })],
            )
            .unwrap();
        }

        let canonical_text = g.resolve_text_canonical(key);
        let prior_mode = g.text_projection_mode();
        g.set_text_projection_mode(TextProjectionMode::Disabled);
        let canonical_seq = g.resolve_text_seq_with_chars(key);
        g.set_text_projection_mode(prior_mode);

        assert!(g.demote_text_projection_key(key));
        assert!(g.text_projection_debug_stats(key).is_none());
        assert!(g.ensure_text_projection_resident(key));

        let rebuilt_text_first = g.resolve_text(key);
        let rebuilt_seq_first = g.resolve_text_seq_with_chars(key);
        let rebuilt_stats_first = g
            .text_projection_debug_stats(key)
            .expect("projection should exist after rebuild");

        assert_eq!(rebuilt_text_first, canonical_text);
        assert_eq!(rebuilt_seq_first, canonical_seq);

        // Rebuild a second time and require deterministic materialization shape/output.
        assert!(g.demote_text_projection_key(key));
        assert!(g.ensure_text_projection_resident(key));

        let rebuilt_text_second = g.resolve_text(key);
        let rebuilt_seq_second = g.resolve_text_seq_with_chars(key);
        let rebuilt_stats_second = g
            .text_projection_debug_stats(key)
            .expect("projection should exist after second rebuild");

        assert_eq!(rebuilt_text_second, rebuilt_text_first);
        assert_eq!(rebuilt_seq_second, rebuilt_seq_first);
        assert_eq!(
            rebuilt_stats_second.visible_len,
            rebuilt_stats_first.visible_len
        );
        assert_eq!(
            rebuilt_stats_second.index_weights_len,
            rebuilt_stats_first.index_weights_len,
            "run-level index weight count should match after rebuild"
        );
        assert_eq!(
            rebuilt_stats_second.index_fenwick_len,
            rebuilt_stats_first.index_fenwick_len
        );
        assert_eq!(
            rebuilt_stats_second.metadata_entries,
            rebuilt_stats_first.metadata_entries
        );
        assert_eq!(
            rebuilt_stats_second.tombstone_count,
            rebuilt_stats_first.tombstone_count
        );
        assert_eq!(
            rebuilt_stats_second.tombstone_span_count,
            rebuilt_stats_first.tombstone_span_count
        );
        assert_eq!(g.resolve_text_canonical(key), rebuilt_text_second);
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_projection_cold_compaction_preserves_semantics() {
        let sk = key_a();
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::Enabled);

        // Grow projection buffers, then shrink visible set to leave slack.
        for i in 0..2048u64 {
            let after = if i == 0 {
                None
            } else {
                Some(crate::op::OpId {
                    lamport: i,
                    author: sk.verifying_key().to_bytes(),
                })
            };
            g.apply_local(
                &sk,
                1000 + i,
                vec![Op::Text(TextOp::Insert {
                    key: "doc".into(),
                    after,
                    ch: 'a',
                })],
            )
            .unwrap();
        }

        let seq = g.resolve_text_seq_with_chars("doc");
        for (idx, (id, _)) in seq.iter().take(2000).copied().enumerate() {
            g.apply_local(
                &sk,
                5000 + idx as u64,
                vec![Op::Text(TextOp::Delete {
                    key: "doc".into(),
                    target: id,
                })],
            )
            .unwrap();
        }

        let expected = g.resolve_text("doc");
        let expected_seq = g.resolve_text_seq_with_chars("doc");
        let before = g
            .text_projection_debug_stats("doc")
            .expect("projection should exist before compaction");
        assert!(before.visible_len > 0);

        let _ = g.compact_text_projection_key("doc");

        let after = g
            .text_projection_debug_stats("doc")
            .expect("projection should exist after compaction");
        assert_eq!(g.resolve_text("doc"), expected);
        assert_eq!(
            g.resolve_text_seq_with_chars("doc"),
            expected_seq,
            "compaction must preserve canonical visible OpId identity ordering"
        );
        assert_eq!(
            after.metadata_entries, before.metadata_entries,
            "compaction must not squash historical metadata entries"
        );
        assert_eq!(
            after.tombstone_count, before.tombstone_count,
            "compaction must not drop tombstone history"
        );
        assert_eq!(
            after.tombstone_span_count, before.tombstone_span_count,
            "compaction must not merge semantic tombstone spans permanently"
        );
        assert!(
            after.visible_string_capacity <= before.visible_string_capacity,
            "compaction should not increase visible string capacity"
        );
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_projection_enabled_miss_does_not_use_replay_fallback() {
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::Enabled);
        g.reset_text_runtime_counters();

        assert_eq!(g.resolve_text("missing"), "");
        let counters = g.text_runtime_counters();
        assert_eq!(counters.projection_hits, 1);
        assert_eq!(counters.replay_fallbacks, 0);

        let _ = g.resolve_text_anchor_for_offset("missing", 0);
        let _ = g.resolve_text_offset_for_anchor("missing", TextRangeAnchor::Start);
        let counters_after = g.text_runtime_counters();
        assert_eq!(
            counters_after.replay_fallbacks, 0,
            "anchor/offset mapping in enabled mode should not invoke replay fallback"
        );
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_projection_debug_stats_expose_telemetry_counters() {
        let sk = key_a();
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::Enabled);

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
        let id_a = crate::op::OpId {
            lamport: 1,
            author: sk.verifying_key().to_bytes(),
        };
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
        g.apply_local(
            &sk,
            1002,
            vec![Op::Text(TextOp::Delete {
                key: "doc".into(),
                target: id_a,
            })],
        )
        .unwrap();

        let _ = g.resolve_text_seq("doc");
        let _ = g.resolve_text("doc");
        let _ = g.resolve_text_range("doc", 0, 1);

        let stats = g
            .text_projection_debug_stats("doc")
            .expect("debug stats should exist for key");
        assert!(stats.insert_ops_applied >= 2);
        assert!(stats.delete_ops_applied >= 1);
        assert!(stats.resolve_seq_calls >= 1);
        assert!(stats.resolve_string_calls >= 1);
        assert!(stats.resolve_range_calls >= 1);
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_projection_repeated_reads_without_writes_do_not_rebuild() {
        let sk = key_a();
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::Enabled);

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
        let id_a = crate::op::OpId {
            lamport: 1,
            author: sk.verifying_key().to_bytes(),
        };
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

        let before = g
            .text_projection_debug_stats("doc")
            .expect("projection stats should exist for key")
            .full_rebuild_count;

        for _ in 0..64 {
            let _ = g.resolve_text("doc");
            let _ = g.resolve_text_range("doc", 0, 2);
            let _ = g.resolve_text_seq("doc");
        }

        let after = g
            .text_projection_debug_stats("doc")
            .expect("projection stats should exist for key")
            .full_rebuild_count;

        assert_eq!(
            after, before,
            "repeated reads with no writes should not trigger projection rebuilds"
        );
    }

    #[test]
    fn resolve_text_canonical_is_mode_independent() {
        let sk = key_a();
        let mut g = StateGraph::new();

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
        let id_a = crate::op::OpId {
            lamport: 1,
            author: sk.verifying_key().to_bytes(),
        };
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

        g.set_text_projection_mode(TextProjectionMode::Enabled);
        let enabled_canonical = g.resolve_text_canonical("doc");
        assert_eq!(enabled_canonical, "ab");

        g.set_text_projection_mode(TextProjectionMode::Disabled);
        let disabled_canonical = g.resolve_text_canonical("doc");
        assert_eq!(disabled_canonical, "ab");

        assert_eq!(enabled_canonical, disabled_canonical);

        let counters_before = g.text_runtime_counters();
        let _ = g.resolve_text_canonical("doc");
        let counters_after = g.text_runtime_counters();
        assert_eq!(counters_after.projection_hits, counters_before.projection_hits);
        assert_eq!(
            counters_after.replay_fallbacks, counters_before.replay_fallbacks,
            "canonical resolver should not affect runtime hot-path counters"
        );
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_projection_out_of_order_remote_insert_application() {
        let sk = key_a();
        let mut src = StateGraph::new();
        src.set_text_projection_mode(TextProjectionMode::Enabled);

        let n1 = src
            .apply_local(
                &sk,
                1000,
                vec![Op::Text(TextOp::Insert {
                    key: "doc".into(),
                    after: None,
                    ch: 'a',
                })],
            )
            .unwrap();
        let id_a = src.resolve_text_seq_with_chars("doc")[0].0;

        let n2 = src
            .apply_local(
                &sk,
                1001,
                vec![Op::Text(TextOp::Insert {
                    key: "doc".into(),
                    after: Some(id_a),
                    ch: 'b',
                })],
            )
            .unwrap();

        let exported: Vec<SyncNode> = src.get_nodes(&[n1, n2]).into_iter().cloned().collect();
        let node_a = exported[0].clone();
        let node_b = exported[1].clone();

        let mut dst = StateGraph::new();
        dst.set_text_projection_mode(TextProjectionMode::Enabled);

        // Out-of-order delivery: dependent node arrives first and is retried after parent arrival.
        assert!(matches!(dst.apply_remote(node_b.clone()), Err(SyncError::MissingParent(_))));
        dst.apply_remote(node_a).unwrap();
        dst.apply_remote(node_b).unwrap();

        assert_eq!(dst.resolve_text("doc"), "ab");
        assert_eq!(dst.resolve_text("doc"), dst.resolve_text_legacy("doc"));
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_projection_multi_key_isolation_with_reordered_remote_batch() {
        let sk = key_a();
        let author = sk.verifying_key().to_bytes();

        let mut exported = vec![
            SyncNode::new_signed(
                Transaction {
                    author,
                    lamport: 1,
                    wall_ms: 1000,
                    ops: vec![Op::Text(TextOp::InsertRange {
                        key: "title".into(),
                        anchor: TextRangeAnchor::Start,
                        text: "Hi".into(),
                    })],
                    parents: vec![],
                },
                &sk,
            ),
            SyncNode::new_signed(
                Transaction {
                    author,
                    lamport: 2,
                    wall_ms: 1001,
                    ops: vec![Op::Text(TextOp::InsertRange {
                        key: "body".into(),
                        anchor: TextRangeAnchor::Start,
                        text: "XY".into(),
                    })],
                    parents: vec![],
                },
                &sk,
            ),
        ];
        exported.reverse();

        let mut dst = StateGraph::new();
        dst.set_text_projection_mode(TextProjectionMode::Enabled);
        let batch = dst.apply_remote_batch(exported);

        assert_eq!(batch.accepted.len(), 2);
        assert!(batch.rejected.is_empty());

        assert_eq!(dst.resolve_text("title"), "Hi");
        assert_eq!(dst.resolve_text("body"), "XY");
        assert_eq!(dst.resolve_text("title"), dst.resolve_text_legacy("title"));
        assert_eq!(dst.resolve_text("body"), dst.resolve_text_legacy("body"));
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_projection_tombstone_interactions_with_descendants() {
        let sk = key_a();
        let author = sk.verifying_key().to_bytes();
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::Enabled);

        let id_a = crate::op::OpId { lamport: 1, author };
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

        let id_b = crate::op::OpId { lamport: 2, author };
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

        let id_c = crate::op::OpId { lamport: 3, author };
        g.apply_local(
            &sk,
            1002,
            vec![Op::Text(TextOp::Insert {
                key: "doc".into(),
                after: Some(id_b),
                ch: 'c',
            })],
        )
        .unwrap();

        g.apply_local(
            &sk,
            1003,
            vec![Op::Text(TextOp::Delete {
                key: "doc".into(),
                target: id_b,
            })],
        )
        .unwrap();

        assert_eq!(g.resolve_text("doc"), "ac");
        assert_eq!(g.resolve_text("doc"), g.resolve_text_legacy("doc"));

        let seq = g.resolve_text_seq_with_chars("doc");
        assert_eq!(seq.len(), 2);
        assert_eq!(seq[0].0, id_a);
        assert_eq!(seq[1].0, id_c);

        g.apply_local(
            &sk,
            1004,
            vec![Op::Text(TextOp::Delete {
                key: "doc".into(),
                target: id_a,
            })],
        )
        .unwrap();

        assert_eq!(g.resolve_text("doc"), "c");
        assert_eq!(g.resolve_text("doc"), g.resolve_text_legacy("doc"));
    }

    #[cfg(feature = "text_projection")]
    #[test]
    fn text_projection_sibling_ordering_under_identical_after() {
        let sk_a = key_a();
        let sk_b = key_b();
        let author_a = sk_a.verifying_key().to_bytes();
        let author_b = sk_b.verifying_key().to_bytes();

        let base = SyncNode::new_signed(
            Transaction {
                author: author_a,
                lamport: 1,
                wall_ms: 1000,
                ops: vec![Op::Text(TextOp::Insert {
                    key: "doc".into(),
                    after: None,
                    ch: 'a',
                })],
                parents: vec![],
            },
            &sk_a,
        );
        let base_id = crate::op::OpId {
            lamport: 1,
            author: author_a,
        };

        let sibling_a = SyncNode::new_signed(
            Transaction {
                author: author_a,
                lamport: 2,
                wall_ms: 1001,
                ops: vec![Op::Text(TextOp::Insert {
                    key: "doc".into(),
                    after: Some(base_id),
                    ch: 'x',
                })],
                parents: vec![],
            },
            &sk_a,
        );
        let sibling_b = SyncNode::new_signed(
            Transaction {
                author: author_b,
                lamport: 2,
                wall_ms: 1001,
                ops: vec![Op::Text(TextOp::Insert {
                    key: "doc".into(),
                    after: Some(base_id),
                    ch: 'y',
                })],
                parents: vec![],
            },
            &sk_b,
        );

        let mut g1 = StateGraph::new();
        g1.set_text_projection_mode(TextProjectionMode::Enabled);
        g1.apply_remote(base.clone()).unwrap();
        g1.apply_remote(sibling_a.clone()).unwrap();
        g1.apply_remote(sibling_b.clone()).unwrap();

        let mut g2 = StateGraph::new();
        g2.set_text_projection_mode(TextProjectionMode::Enabled);
        g2.apply_remote(base).unwrap();
        g2.apply_remote(sibling_b).unwrap();
        g2.apply_remote(sibling_a).unwrap();

        let t1 = g1.resolve_text("doc");
        let t2 = g2.resolve_text("doc");
        assert_eq!(t1, t2, "sibling ordering must be deterministic across delivery order");
        assert_eq!(t1, g1.resolve_text_legacy("doc"));
        assert_eq!(t2, g2.resolve_text_legacy("doc"));

        let expected = if author_a > author_b { "axy" } else { "ayx" };
        assert_eq!(t1, expected);
    }

    #[test]
    fn resolve_text_range_skeleton_returns_char_window() {
        let sk = key_a();
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::Enabled);

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
        let id_a = crate::op::OpId {
            lamport: 1,
            author: sk.verifying_key().to_bytes(),
        };
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

        assert_eq!(g.resolve_text("doc"), "ab");
        assert_eq!(g.resolve_text_range("doc", 0, 1), "a");
        assert_eq!(g.resolve_text_range("doc", 1, 1), "b");
        assert_eq!(g.resolve_text_range("doc", 2, 3), "");
    }

    #[test]
    fn phase3_offset_anchor_scaffold_roundtrip() {
        let sk = key_a();
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::Enabled);

        // Build "abc"
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
        let id_a = crate::op::OpId {
            lamport: 1,
            author: sk.verifying_key().to_bytes(),
        };
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
        let id_b = crate::op::OpId {
            lamport: 2,
            author: sk.verifying_key().to_bytes(),
        };
        g.apply_local(
            &sk,
            1002,
            vec![Op::Text(TextOp::Insert {
                key: "doc".into(),
                after: Some(id_b),
                ch: 'c',
            })],
        )
        .unwrap();

        assert_eq!(g.resolve_text_anchor_for_offset("doc", 0), TextRangeAnchor::Start);
        assert_eq!(g.resolve_text_anchor_for_offset("doc", 1), TextRangeAnchor::After(id_a));
        assert_eq!(g.resolve_text_anchor_for_offset("doc", 3), TextRangeAnchor::End);
        assert_eq!(g.resolve_text_anchor_for_offset("doc", 99), TextRangeAnchor::End);

        assert_eq!(g.resolve_text_offset_for_anchor("doc", TextRangeAnchor::Start), 0);
        assert_eq!(g.resolve_text_offset_for_anchor("doc", TextRangeAnchor::After(id_a)), 1);
        assert_eq!(g.resolve_text_offset_for_anchor("doc", TextRangeAnchor::After(id_b)), 2);
        assert_eq!(g.resolve_text_offset_for_anchor("doc", TextRangeAnchor::End), 3);
    }

    #[test]
    fn phase3_cursor_mapping_randomized_trace_roundtrips() {
        let sk = key_a();
        let mut g = StateGraph::new();
        g.set_text_projection_mode(TextProjectionMode::Enabled);
        let key = "doc";

        let mut visible_ids: Vec<crate::op::OpId> = Vec::new();
        let mut seed: u64 = 0xD00D_F00D_1234_5678;
        let mut next_u64 = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            seed
        };

        for step in 0..300u64 {
            let do_insert = visible_ids.is_empty() || (next_u64() % 100) < 70;
            if do_insert {
                let pos = if visible_ids.is_empty() {
                    0usize
                } else {
                    (next_u64() as usize) % (visible_ids.len() + 1)
                };
                let after = if pos == 0 {
                    None
                } else {
                    Some(visible_ids[pos - 1])
                };
                let ch = (b'a' + (next_u64() % 26) as u8) as char;
                g.apply_local(
                    &sk,
                    20_000 + step,
                    vec![Op::Text(TextOp::Insert {
                        key: key.into(),
                        after,
                        ch,
                    })],
                )
                .unwrap();
                visible_ids.insert(
                    pos,
                    crate::op::OpId {
                        lamport: g.lamport(),
                        author: sk.verifying_key().to_bytes(),
                    },
                );
            } else {
                let pos = (next_u64() as usize) % visible_ids.len();
                let target = visible_ids.remove(pos);
                g.apply_local(
                    &sk,
                    20_000 + step,
                    vec![Op::Text(TextOp::Delete {
                        key: key.into(),
                        target,
                    })],
                )
                .unwrap();
            }

            let seq = g.resolve_text_seq_with_chars(key);
            let len = seq.len();
            let probes = [
                0usize,
                len / 2,
                len,
                len.saturating_add(5),
            ];

            for offset in probes {
                let clamped = offset.min(len);
                let anchor = g.resolve_text_anchor_for_offset(key, offset);
                let roundtrip = g.resolve_text_offset_for_anchor(key, anchor);
                assert_eq!(
                    roundtrip, clamped,
                    "offset->anchor->offset mismatch at step {step}: offset={offset}, len={len}"
                );
            }

            for (idx, (id, _)) in seq.iter().enumerate() {
                let off = g.resolve_text_offset_for_anchor(key, TextRangeAnchor::After(*id));
                assert_eq!(
                    off,
                    idx + 1,
                    "after(id)->offset mismatch at step {step}: idx={idx}"
                );
            }
        }
    }

    #[test]
    fn lww_last_write_wins() {
        let mut g = StateGraph::new();
        g.apply_local(&key_a(), 0, vec![Op::Map(MapOp::Set { key: "score".into(), value: b"10".to_vec() })]).unwrap();
        g.apply_local(&key_a(), 1, vec![Op::Map(MapOp::Set { key: "score".into(), value: b"20".to_vec() })]).unwrap();
        let state = g.resolve();
        assert_eq!(state.get("score").map(|v| v.as_slice()), Some(b"20".as_slice()));
    }

    #[test]
    fn delete_removes_key() {
        let mut g = StateGraph::new();
        g.apply_local(&key_a(), 0, vec![set("x", "1")]).unwrap();
        g.apply_local(&key_a(), 1, vec![Op::Map(MapOp::Delete { key: "x".into() })]).unwrap();
        let state = g.resolve();
        assert!(!state.contains_key("x"));
    }

    #[test]
    fn merkle_root_changes_on_new_node() {
        let mut g = StateGraph::new();
        let root0 = g.merkle_root();
        g.apply_local(&key_a(), 0, vec![set("k", "v")]).unwrap();
        let root1 = g.merkle_root();
        assert_ne!(root0, root1);
    }

    #[test]
    fn signature_verified_on_apply_remote() {
        let ka = key_a();
        let mut g = StateGraph::new();
        let id = g.apply_local(&ka, 0, vec![set("x", "1")]).unwrap();
        // Extract the signed node and re-apply to a fresh graph — should pass.
        let node = g.get_nodes(&[id])[0].clone();
        let mut g2 = StateGraph::new();
        g2.apply_remote(node).unwrap();
        assert_eq!(g2.node_count(), 1);
    }

    #[test]
    fn tampered_signature_rejected() {
        let ka = key_a();
        let mut g = StateGraph::new();
        let id = g.apply_local(&ka, 0, vec![set("x", "1")]).unwrap();
        let mut node = g.get_nodes(&[id])[0].clone();
        node.signature.0[0] ^= 0xff; // corrupt first byte
        let mut g2 = StateGraph::new();
        assert!(matches!(g2.apply_remote(node), Err(SyncError::InvalidSignature(_))));
    }

    #[test]
    fn concurrent_conflict_resolved_deterministically() {
        let ka = key_a();
        let kb = key_b();
        // Build two signed nodes with distinct lamport clocks.
        let tx_a = Transaction {
            author: ka.verifying_key().to_bytes(),
            lamport: 1,
            wall_ms: 1000,
            ops: vec![Op::Map(MapOp::Set { key: "val".into(), value: b"from_A".to_vec() })],
            parents: vec![],
        };
        let node_a = SyncNode::new_signed(tx_a, &ka);

        let tx_b = Transaction {
            author: kb.verifying_key().to_bytes(),
            lamport: 2,
            wall_ms: 999,
            ops: vec![Op::Map(MapOp::Set { key: "val".into(), value: b"from_B".to_vec() })],
            parents: vec![],
        };
        let node_b = SyncNode::new_signed(tx_b, &kb);

        let mut g1 = StateGraph::new();
        g1.apply_remote(node_a.clone()).unwrap();
        g1.apply_remote(node_b.clone()).unwrap();

        let mut g2 = StateGraph::new();
        g2.apply_remote(node_b).unwrap();
        g2.apply_remote(node_a).unwrap();

        let r1 = g1.resolve();
        let r2 = g2.resolve();
        assert_eq!(r1.get("val"), r2.get("val"));
        assert_eq!(r1.get("val").map(|v| v.as_slice()), Some(b"from_B".as_slice()));
    }

    #[test]
    fn missing_hashes_for_sync() {
        let ka = key_a();
        let mut g_server = StateGraph::new();
        g_server.apply_local(&ka, 0, vec![set("k1", "v1")]).unwrap();
        g_server.apply_local(&ka, 1, vec![set("k2", "v2")]).unwrap();

        let client_known: HashSet<NodeId> = HashSet::new();
        let missing = g_server.missing_hashes(&client_known);
        assert_eq!(missing.len(), 2);
    }

    // -------------------------------------------------------------------------
    // A5 policy enforcement tests
    // -------------------------------------------------------------------------

    fn make_node(key: &str, val: &str, signing_key: &SigningKey) -> SyncNode {
        let tx = Transaction {
            author: signing_key.verifying_key().to_bytes(),
            lamport: 1,
            wall_ms: 0,
            ops: vec![Op::Map(MapOp::Set { key: key.into(), value: val.as_bytes().to_vec() })],
            parents: vec![],
        };
        SyncNode::new_signed(tx, signing_key)
    }

    #[test]
    fn policy_open_room_allows_any_author() {
        // Default policy (AllowAll) — no rules — any author may write anything.
        let mut g = StateGraph::new();
        let node = make_node("world/pos", "up", &key_a());
        g.apply_remote(node).unwrap();
        assert_eq!(g.node_count(), 1);
    }

    #[test]
    fn policy_deny_unauthorized_author() {
        use crate::policy::{Policy, PolicyDefault, PolicyRule};

        let ka = key_a();
        let kb = key_b();
        let authorized = ka.verifying_key().to_bytes();

        let policy = Policy {
            rules: vec![PolicyRule {
                path_glob: "world/**".into(),
                can_write: vec![authorized],
                can_read: vec![],
                can_derive: vec![],
            }],
            default: PolicyDefault::DenyAll,
        };

        let mut g = StateGraph::new();
        g.set_policy(policy);

        // key_a is authorized — should succeed.
        let good_node = make_node("world/pos", "up", &ka);
        g.apply_remote(good_node).unwrap();

        // key_b is NOT authorized — should be rejected.
        let bad_node = make_node("world/pos", "down", &kb);
        assert!(matches!(
            g.apply_remote(bad_node),
            Err(SyncError::PolicyViolation { .. })
        ));

        // Only the authorized node was stored.
        assert_eq!(g.node_count(), 1);
    }

    #[test]
    fn policy_wildcard_allows_matching_paths() {
        use crate::policy::{Policy, PolicyDefault, PolicyRule};

        let ka = key_a();
        let kb = key_b();
        let pub_a = ka.verifying_key().to_bytes();
        let pub_b = kb.verifying_key().to_bytes();

        let policy = Policy {
            rules: vec![
                // Only A may write to world/**
                PolicyRule {
                    path_glob: "world/**".into(),
                    can_write: vec![pub_a],
                    can_read: vec![],
                    can_derive: vec![],
                },
                // Both A and B may write to intent/*
                PolicyRule {
                    path_glob: "intent/*".into(),
                    can_write: vec![pub_a, pub_b],
                    can_read: vec![],
                    can_derive: vec![],
                },
            ],
            default: PolicyDefault::DenyAll,
        };

        let mut g = StateGraph::new();
        g.set_policy(policy);

        // A writes to world/** — allowed
        g.apply_remote(make_node("world/player/pos", "1,2", &ka)).unwrap();

        // B writes to world/** — denied
        assert!(matches!(
            g.apply_remote(make_node("world/player/pos", "3,4", &kb)),
            Err(SyncError::PolicyViolation { .. })
        ));

        // B writes to intent/* — allowed
        g.apply_remote(make_node("intent/move", "up", &kb)).unwrap();

        // unmatched path — DenyAll default blocks both
        assert!(matches!(
            g.apply_remote(make_node("other/key", "x", &ka)),
            Err(SyncError::PolicyViolation { .. })
        ));

        assert_eq!(g.node_count(), 2);
    }

    // -------------------------------------------------------------------------
    // E2: Speculative vs. Canonical State
    // -------------------------------------------------------------------------

    #[test]
    fn local_write_is_speculative_not_canonical() {
        let mut g = StateGraph::new();
        g.apply_local(&key_a(), 0, vec![set("pos", "local")]).unwrap();

        // Speculative view (all nodes) sees the local write.
        assert_eq!(
            g.resolve().get("pos").map(|v| v.as_slice()),
            Some(b"local".as_slice())
        );
        assert_eq!(g.read_speculative("pos").as_deref(), Some(b"local".as_ref()));

        // Canonical view excludes local-only nodes — key is absent.
        assert!(g.resolve_canonical().get("pos").is_none());
        assert!(g.read_canonical("pos").is_none());
    }

    #[test]
    fn remote_write_is_canonical_and_speculative() {
        let tx = Transaction {
            author: key_a().verifying_key().to_bytes(),
            lamport: 1,
            wall_ms: 0,
            ops: vec![Op::Map(MapOp::Set { key: "pos".into(), value: b"remote".to_vec() })],
            parents: vec![],
        };
        let node = SyncNode::new_signed(tx, &key_a());
        let mut g = StateGraph::new();
        g.apply_remote(node).unwrap();

        // Both views see the remote write.
        assert_eq!(g.read_speculative("pos").as_deref(), Some(b"remote".as_ref()));
        assert_eq!(g.read_canonical("pos").as_deref(), Some(b"remote".as_ref()));
    }

    #[test]
    fn snap_back_canonical_wins_on_conflict() {
        // Client writes locally (speculative).
        let ka = key_a();
        let kb = key_b();
        let mut g = StateGraph::new();
        g.apply_local(&ka, 1, vec![set("pos", "client")]).unwrap();

        // Speculative = client value; canonical = absent.
        assert_eq!(g.read_speculative("pos").as_deref(), Some(b"client".as_ref()));
        assert!(g.read_canonical("pos").is_none());

        // Server sends an authoritative node with higher lamport → wins LWW.
        let server_tx = Transaction {
            author: kb.verifying_key().to_bytes(),
            lamport: 5,
            wall_ms: 0,
            ops: vec![Op::Map(MapOp::Set { key: "pos".into(), value: b"server".to_vec() })],
            parents: vec![],
        };
        let server_node = SyncNode::new_signed(server_tx, &kb);
        g.apply_remote(server_node).unwrap();

        // Canonical view now shows server value.
        assert_eq!(g.read_canonical("pos").as_deref(), Some(b"server".as_ref()));
        // Speculative also resolves to server value (higher lamport wins LWW).
        assert_eq!(g.read_speculative("pos").as_deref(), Some(b"server".as_ref()));
    }

    #[test]
    fn same_ops_different_tick_groupings_converge() {
        // E3 determinism: ops in 1 node vs ops in 2 nodes → same resolved state.
        let ka = key_a();

        // One batch: both ops in a single node.
        let mut g1 = StateGraph::new();
        g1.apply_local(&ka, 1, vec![set("x", "1"), set("y", "2")]).unwrap();

        // Two batches: each op in its own node.
        let mut g2 = StateGraph::new();
        g2.apply_local(&ka, 1, vec![set("x", "1")]).unwrap();
        g2.apply_local(&ka, 2, vec![set("y", "2")]).unwrap();

        // Canonical from g1's perspective (no remote nodes) is empty, but
        // speculative state must be identical for the same content.
        assert_eq!(g1.read_speculative("x"), g2.read_speculative("x"));
        assert_eq!(g1.read_speculative("y"), g2.read_speculative("y"));
    }

    // -------------------------------------------------------------------------
    // G5 — Lamport ceiling & wall-clock skew tests
    // -------------------------------------------------------------------------

    /// Build a signed node with a caller-specified lamport and wall_ms,
    /// empty parents, and a single benign `Set` op. Used to probe G5
    /// sanity checks without going through `apply_local` (which would
    /// clamp the lamport to sensible values).
    fn g5_node(sk: &SigningKey, lamport: u64, wall_ms: u64) -> SyncNode {
        let tx = Transaction {
            author: sk.verifying_key().to_bytes(),
            lamport,
            wall_ms,
            ops: vec![Op::Map(MapOp::Set { key: "k".into(), value: b"v".to_vec() })],
            parents: vec![],
        };
        SyncNode::new_signed(tx, sk)
    }

    #[test]
    fn g5_lamport_ceiling_rejects_wild_future_clock() {
        let mut g = StateGraph::new();
        // Local clock starts at 0, ceiling = LAMPORT_SLACK.
        let bad = g5_node(&key_a(), LAMPORT_SLACK + 1, 0);
        let err = g.apply_remote(bad).unwrap_err();
        assert!(matches!(err, SyncError::LamportCeiling { .. }), "got {err:?}");
        assert_eq!(g.node_count(), 0);
    }

    #[test]
    fn g5_lamport_ceiling_allows_boundary() {
        let mut g = StateGraph::new();
        // Exactly at the ceiling must pass.
        let ok = g5_node(&key_a(), LAMPORT_SLACK, 0);
        g.apply_remote(ok).expect("boundary lamport must be accepted");
        assert_eq!(g.node_count(), 1);
    }

    #[test]
    fn g5_lamport_ceiling_batch_rejects_only_the_bad_node() {
        let mut g = StateGraph::new();
        let ka = key_a();
        let good = g5_node(&ka, 1, 0);
        let bad = g5_node(&ka, LAMPORT_SLACK + 42, 0);
        let good_id = good.id;
        let bad_id = bad.id;
        let res = g.apply_remote_batch(vec![good, bad]);
        assert_eq!(res.accepted, vec![good_id]);
        assert_eq!(res.rejected.len(), 1);
        assert_eq!(res.rejected[0].0, bad_id);
        assert!(matches!(res.rejected[0].1, SyncError::LamportCeiling { .. }));
    }

    #[test]
    fn g5_wall_skew_rejects_far_future_when_now_supplied() {
        let mut g = StateGraph::new();
        let now: u64 = 1_700_000_000_000; // arbitrary, 2023-ish
        let too_far = now + WALL_SKEW_MAX_MS + 1;
        let bad = g5_node(&key_a(), 1, too_far);
        let err = g.apply_remote_checked(bad, Some(now)).unwrap_err();
        assert!(matches!(err, SyncError::WallClockSkew { .. }), "got {err:?}");
        assert_eq!(g.node_count(), 0);
    }

    #[test]
    fn g5_wall_skew_ignored_when_now_is_none() {
        // Bridge / WASM path: no trusted clock, so skew check must be a no-op.
        let mut g = StateGraph::new();
        let too_far = 10 * WALL_SKEW_MAX_MS;
        let n = g5_node(&key_a(), 1, too_far);
        g.apply_remote_checked(n, None).expect("no clock → no check");
        assert_eq!(g.node_count(), 1);
    }

    #[test]
    fn g5_wall_skew_accepts_zero_wall_ms() {
        // Compaction nodes and legacy unsigned clients carry wall_ms=0;
        // those must never be flagged even when `now_ms` is supplied.
        let mut g = StateGraph::new();
        let now: u64 = 1_700_000_000_000;
        let n = g5_node(&key_a(), 1, 0);
        g.apply_remote_checked(n, Some(now)).unwrap();
        assert_eq!(g.node_count(), 1);
    }

    #[test]
    fn g5_wall_skew_batch_rejects_only_the_bad_node() {
        let mut g = StateGraph::new();
        let ka = key_a();
        let now: u64 = 1_700_000_000_000;
        let good = g5_node(&ka, 1, now); // right now — fine
        let bad  = g5_node(&ka, 2, now + WALL_SKEW_MAX_MS + 1);
        let good_id = good.id;
        let bad_id = bad.id;
        let res = g.apply_remote_batch_checked(vec![good, bad], Some(now));
        assert_eq!(res.accepted, vec![good_id]);
        assert_eq!(res.rejected.len(), 1);
        assert_eq!(res.rejected[0].0, bad_id);
        assert!(matches!(res.rejected[0].1, SyncError::WallClockSkew { .. }));
    }

    // -------------------------------------------------------------------
    // Incremental state-cache parity (map/list/blob caches vs full replay)
    // -------------------------------------------------------------------

    /// Deterministic xorshift so the parity tests are reproducible.
    fn xorshift(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    #[test]
    fn map_cache_matches_replay_under_randomized_local_and_remote_writes() {
        let ka = key_a();
        let kb = key_b();
        let mut g_a = StateGraph::new();
        let mut g_b = StateGraph::new();
        let mut rng = 0x1234_5678_9abc_def0u64;
        let keys = ["alpha", "beta", "gamma", "delta"];

        for step in 0..200 {
            let key = keys[(xorshift(&mut rng) % keys.len() as u64) as usize];
            let roll = xorshift(&mut rng) % 10;
            let (graph, sk) = if xorshift(&mut rng) % 2 == 0 {
                (&mut g_a, &ka)
            } else {
                (&mut g_b, &kb)
            };
            let op = if roll < 6 {
                Op::Map(MapOp::Set {
                    key: key.into(),
                    value: format!("v{step}").into_bytes(),
                })
            } else if roll < 8 {
                Op::Map(MapOp::Delete { key: key.into() })
            } else {
                Op::Map(MapOp::SetBlob {
                    key: key.into(),
                    blob_hash: Hash::of(format!("blob{step}").as_bytes()),
                })
            };
            graph.apply_local(sk, step, vec![op]).unwrap();

            // Periodically cross-sync so remote/canonical paths are exercised.
            if step % 17 == 0 {
                let b_nodes: Vec<SyncNode> = g_b.all_nodes();
                for n in b_nodes {
                    let _ = g_a.apply_remote(n);
                }
                let a_nodes: Vec<SyncNode> = g_a.all_nodes();
                for n in a_nodes {
                    let _ = g_b.apply_remote(n);
                }
            }
        }

        for g in [&g_a, &g_b] {
            assert_eq!(g.resolve(), g.resolve_replay(false), "speculative cache != replay");
            assert_eq!(
                g.resolve_canonical(),
                g.resolve_replay(true),
                "canonical cache != replay"
            );
            // resolve_with_meta must agree with resolve() on keys/values.
            let meta = g.resolve_with_meta();
            let plain = g.resolve();
            assert_eq!(meta.len(), plain.len());
            for (k, v) in &plain {
                assert_eq!(&meta.get(k).unwrap().2, v);
            }
            // Blob-hash cache parity vs manual scan.
            let mut scanned = HashSet::new();
            for node in g.all_nodes() {
                for op in &node.transaction.ops {
                    if let Op::Map(MapOp::SetBlob { blob_hash, .. }) = op {
                        scanned.insert(*blob_hash);
                    }
                }
            }
            assert_eq!(g.referenced_blob_hashes(), scanned);
        }
    }

    #[test]
    fn list_cache_matches_replay_under_randomized_ops() {
        use crate::list::FracIdx;
        use crate::op::{ItemId, ListOp};

        let ka = key_a();
        let kb = key_b();
        let mut g_a = StateGraph::new();
        let mut g_b = StateGraph::new();
        let mut rng = 0xfeed_beef_cafe_0001u64;
        let item_pool: Vec<ItemId> = (0..8u8).map(|i| ItemId([i; 16])).collect();
        let lists = ["L1", "L2"];

        for step in 0..200 {
            let list_key = lists[(xorshift(&mut rng) % 2) as usize];
            let item_id = item_pool[(xorshift(&mut rng) % item_pool.len() as u64) as usize];
            let pos = FracIdx::new(format!(
                "{}{}",
                char::from(b'A' + (xorshift(&mut rng) % 26) as u8),
                char::from(b'a' + (xorshift(&mut rng) % 26) as u8)
            ));
            let roll = xorshift(&mut rng) % 10;
            let op = if roll < 5 {
                Op::List(ListOp::Insert {
                    list_key: list_key.into(),
                    item_id,
                    position: pos,
                })
            } else if roll < 8 {
                Op::List(ListOp::Move {
                    list_key: list_key.into(),
                    item_id,
                    position: pos,
                })
            } else {
                Op::List(ListOp::Delete {
                    list_key: list_key.into(),
                    item_id,
                })
            };
            let (graph, sk) = if xorshift(&mut rng) % 2 == 0 {
                (&mut g_a, &ka)
            } else {
                (&mut g_b, &kb)
            };
            graph.apply_local(sk, step, vec![op]).unwrap();

            if step % 13 == 0 {
                let b_nodes: Vec<SyncNode> = g_b.all_nodes();
                for n in b_nodes {
                    let _ = g_a.apply_remote(n);
                }
                let a_nodes: Vec<SyncNode> = g_a.all_nodes();
                for n in a_nodes {
                    let _ = g_b.apply_remote(n);
                }
            }
        }
        for g in [&g_a, &g_b] {
            for list_key in lists {
                assert_eq!(
                    g.resolve_list(list_key),
                    g.resolve_list_replay(list_key),
                    "list cache != replay for {list_key}"
                );
            }
        }
    }

    #[test]
    fn conflict_stream_emits_map_lww_losses_once() {
        let ka = key_a();
        let kb = key_b();
        let author_a = ka.verifying_key().to_bytes();
        let author_b = kb.verifying_key().to_bytes();

        let mut g = StateGraph::new();
        g.set_conflict_stream_enabled(true);

        // A writes, then B's later write demotes it: one MapOverwrite event.
        g.apply_local(&ka, 0, vec![set("k", "from-a")]).unwrap();
        g.apply_local(&kb, 0, vec![set("k", "from-b")]).unwrap();

        let events = g.drain_pending_conflicts();
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.kind, crate::conflicts::ConflictKind::MapOverwrite);
        assert_eq!(e.key, "k");
        assert_eq!(e.winner_author, author_b);
        assert_eq!(e.loser_author, author_a);

        // Drained — no repeats.
        assert!(g.drain_pending_conflicts().is_empty());

        // Same-author overwrite: refinement, not conflict.
        g.apply_local(&kb, 0, vec![set("k", "from-b-2")]).unwrap();
        assert!(g.drain_pending_conflicts().is_empty());
    }

    #[test]
    fn conflict_stream_emits_list_delete_won() {
        use crate::list::FracIdx;
        use crate::op::{ItemId, ListOp};

        let ka = key_a();
        let kb = key_b();
        let mut g = StateGraph::new();
        g.set_conflict_stream_enabled(true);

        let item = ItemId([7u8; 16]);
        g.apply_local(&ka, 0, vec![Op::List(ListOp::Insert {
            list_key: "L".into(),
            item_id: item,
            position: FracIdx::new("M"),
        })]).unwrap();
        g.apply_local(&kb, 0, vec![Op::List(ListOp::Delete {
            list_key: "L".into(),
            item_id: item,
        })]).unwrap();

        let events = g.drain_pending_conflicts();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, crate::conflicts::ConflictKind::ListDeleteWon);
        // Delete is absorbing: item is gone despite the earlier insert.
        assert!(g.resolve_list("L").is_empty());
    }
}
