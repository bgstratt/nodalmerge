use std::cmp::min;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::contracts::{AdminPinStore, AssetInventoryStore, BlobObjectStore, GcRunStore, LiveHashSource};
use crate::errors::{GcError, GcResult};
use crate::types::{AssetState, GcRunDelta, GcRunFinish, GcRunMode, GcRunStart, GcRunStatus};

#[derive(Debug, Clone)]
pub struct GcCoordinatorConfig {
    pub grace_window: Duration,
    pub max_deletes_per_run: u64,
    pub require_head_before_delete: bool,
}

impl Default for GcCoordinatorConfig {
    fn default() -> Self {
        Self {
            grace_window: Duration::from_secs(24 * 60 * 60),
            max_deletes_per_run: 100,
            require_head_before_delete: true,
        }
    }
}

pub struct GcCoordinator<L, I, R, P, B>
where
    L: LiveHashSource,
    I: AssetInventoryStore,
    R: GcRunStore,
    P: AdminPinStore,
    B: BlobObjectStore,
{
    live_hashes: Arc<L>,
    inventory: Arc<I>,
    runs: Arc<R>,
    pins: Arc<P>,
    objects: Arc<B>,
    cfg: GcCoordinatorConfig,
}

impl<L, I, R, P, B> GcCoordinator<L, I, R, P, B>
where
    L: LiveHashSource,
    I: AssetInventoryStore,
    R: GcRunStore,
    P: AdminPinStore,
    B: BlobObjectStore,
{
    pub fn new(
        live_hashes: Arc<L>,
        inventory: Arc<I>,
        runs: Arc<R>,
        pins: Arc<P>,
        objects: Arc<B>,
        cfg: GcCoordinatorConfig,
    ) -> Self {
        Self {
            live_hashes,
            inventory,
            runs,
            pins,
            objects,
            cfg,
        }
    }

    pub fn run_once(&self, mode: GcRunMode, now: SystemTime) -> GcResult<GcRunDelta> {
        let run_id = self.runs.start_run(GcRunStart { mode, started_at: now })?;
        let result = self.run_inner(&run_id, mode, now);
        let status = if result.is_ok() {
            GcRunStatus::Succeeded
        } else {
            GcRunStatus::Failed
        };
        let finish = GcRunFinish {
            status,
            finished_at: now,
            notes: None,
        };
        let _ = self.runs.finish_run(&run_id, finish);
        result
    }

    fn run_inner(&self, run_id: &str, mode: GcRunMode, now: SystemTime) -> GcResult<GcRunDelta> {
        let mut delta = GcRunDelta::default();

        let live = self.live_hashes.collect_live_hashes()?;
        for hash in &live {
            self.inventory.upsert_active_seen(run_id, hash, now)?;
            delta.marked_count += 1;
        }

        if matches!(mode, GcRunMode::DryRun | GcRunMode::MarkOnly) {
            self.runs.apply_delta(run_id, delta.clone())?;
            return Ok(delta);
        }

        self.sweep_soft(run_id, now, &mut delta)?;

        if matches!(mode, GcRunMode::SweepHard) {
            self.sweep_hard(now, &mut delta)?;
        }

        self.runs.apply_delta(run_id, delta.clone())?;
        Ok(delta)
    }

    fn sweep_soft(&self, run_id: &str, now: SystemTime, delta: &mut GcRunDelta) -> GcResult<()> {
        let candidates = self.inventory.iter_unmarked_candidates(run_id)?;
        for rec in candidates {
            // Preserve existing tombstone age; soft sweep should only create
            // tombstones for newly-unreferenced records.
            if rec.pending_delete_at.is_some() || matches!(rec.state, AssetState::PendingDelete) {
                continue;
            }
            if rec.is_admin_pinned || self.pins.is_pinned(&rec.hash)? {
                delta.skipped_pinned_count += 1;
                continue;
            }
            self.inventory.set_pending_delete(&rec.hash, now)?;
            delta.newly_pending_count += 1;
        }
        Ok(())
    }

    fn sweep_hard(&self, now: SystemTime, delta: &mut GcRunDelta) -> GcResult<()> {
        let cutoff = now
            .checked_sub(self.cfg.grace_window)
            .ok_or_else(|| GcError::Invariant("failed to compute grace cutoff".to_string()))?;
        let candidates = self.inventory.iter_pending_older_than(cutoff)?;
        let mut remaining = min(self.cfg.max_deletes_per_run, u64::MAX);
        for rec in candidates {
            if remaining == 0 {
                break;
            }
            if rec.is_admin_pinned || self.pins.is_pinned(&rec.hash)? {
                delta.skipped_pinned_count += 1;
                continue;
            }
            if self.cfg.require_head_before_delete {
                let exists = self.objects.head(&rec.bucket, &rec.object_key)?;
                if !exists {
                    self.inventory.set_deleted(&rec.hash, now)?;
                    continue;
                }
            }
            self.objects.delete(&rec.bucket, &rec.object_key)?;
            self.inventory.set_deleted(&rec.hash, now)?;
            delta.hard_deleted_count += 1;
            remaining -= 1;
        }
        Ok(())
    }
}
