use std::collections::HashSet;
use std::time::SystemTime;

use crate::errors::GcResult;
use crate::types::{AssetRecord, GcRunDelta, GcRunFinish, GcRunStart};

/// Authoritative mark input from NodalMerge state.
pub trait LiveHashSource: Send + Sync {
    fn collect_live_hashes(&self) -> GcResult<HashSet<String>>;
}

/// Persistent inventory and state transitions for blob lifecycle records.
pub trait AssetInventoryStore: Send + Sync {
    fn upsert_active_seen(&self, run_id: &str, hash: &str, now: SystemTime) -> GcResult<()>;

    /// Mark a whole live set in one call (blob-cas-remediation.md slice 6.3).
    ///
    /// The default loops [`Self::upsert_active_seen`] — semantically
    /// identical to what the coordinator's mark pass did before this method
    /// existed, just unbatched. That makes this default **correct, not a
    /// trap**: unlike a default that *answers differently* than a real
    /// override would (the 2.1 `RemoteBlobLinkAggregator` hole), an
    /// implementor that never overrides this only pays the old per-row
    /// cost, it never gets a wrong answer. Stores with transactional
    /// semantics should override to amortize commit cost across the batch
    /// (`SqliteGcStore` collapses 100k autocommits into one transaction).
    ///
    /// Error contract: `Err` means the batch is **not known to be fully
    /// applied** — the coordinator treats that exactly like a failed
    /// per-row upsert (the run fails, nothing sweeps this run). A partial
    /// application left behind by a non-transactional default is harmless
    /// in that world: marks only ever *protect*, and the failed run never
    /// reaches its sweep phase.
    fn upsert_active_seen_batch(
        &self,
        run_id: &str,
        hashes: &[&str],
        now: SystemTime,
    ) -> GcResult<()> {
        for hash in hashes {
            self.upsert_active_seen(run_id, hash, now)?;
        }
        Ok(())
    }

    fn iter_unmarked_candidates(
        &self,
        run_id: &str,
    ) -> GcResult<Box<dyn Iterator<Item = AssetRecord> + Send>>;

    fn iter_pending_older_than(
        &self,
        cutoff: SystemTime,
    ) -> GcResult<Box<dyn Iterator<Item = AssetRecord> + Send>>;

    fn set_pending_delete(&self, hash: &str, at: SystemTime) -> GcResult<()>;
    fn set_deleted(&self, hash: &str, at: SystemTime) -> GcResult<()>;
    fn clear_pending_delete(&self, hash: &str) -> GcResult<()>;
}

/// Run ledger for GC orchestration and auditability.
pub trait GcRunStore: Send + Sync {
    fn start_run(&self, start: GcRunStart) -> GcResult<String>;
    fn apply_delta(&self, run_id: &str, delta: GcRunDelta) -> GcResult<()>;
    fn finish_run(&self, run_id: &str, finish: GcRunFinish) -> GcResult<()>;
}

/// Optional pin provider that can prevent deletion.
pub trait AdminPinStore: Send + Sync {
    fn is_pinned(&self, hash: &str) -> GcResult<bool>;
}

/// Storage execution surface used by the coordinator for object checks/deletes.
pub trait BlobObjectStore: Send + Sync {
    fn head(&self, bucket: &str, key: &str) -> GcResult<bool>;
    fn delete(&self, bucket: &str, key: &str) -> GcResult<()>;
}

/// Optional fast-path for hosts that emit old/new reference deltas.
///
/// Mark remains authoritative; delta is acceleration only.
pub trait ReferenceDeltaSink: Send + Sync {
    fn apply_delta(
        &self,
        scope: &str,
        added_hashes: &[String],
        removed_hashes: &[String],
        observed_at: SystemTime,
    ) -> GcResult<()>;
}
