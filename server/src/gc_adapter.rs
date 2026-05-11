use std::collections::HashSet;
use std::sync::Arc;
use std::time::SystemTime;

use activesync_gc::contracts::{
    AdminPinStore, AssetInventoryStore, BlobObjectStore, GcRunStore, LiveHashSource,
};
use activesync_gc::types::{AssetRecord, GcRunDelta, GcRunFinish, GcRunMode, GcRunStart};
use activesync_gc::{GcCoordinator, GcCoordinatorConfig, GcResult};

/// Compatibility bridge for PR-05: run the shared GC coordinator in MarkOnly
/// mode from the server's existing per-room sweep loop without changing delete
/// behavior. Deletion still flows through legacy `BlobPersistence::blob_gc_sweep`.
pub fn run_mark_only_preflight(room_id: &str, live_hashes: &HashSet<activesync_core::Hash>) -> GcResult<GcRunDelta> {
    let live = Arc::new(LiveFromSet {
        hashes: live_hashes.iter().map(|h| h.to_hex()).collect(),
    });
    let inventory = Arc::new(NoopInventory);
    let runs = Arc::new(NoopRunStore {
        room_id: room_id.to_string(),
    });
    let pins = Arc::new(NoopPins);
    let objects = Arc::new(NoopObjects);

    let gc = GcCoordinator::new(
        live,
        inventory,
        runs,
        pins,
        objects,
        GcCoordinatorConfig::default(),
    );
    gc.run_once(GcRunMode::MarkOnly, SystemTime::now())
}

struct LiveFromSet {
    hashes: HashSet<String>,
}

impl LiveHashSource for LiveFromSet {
    fn collect_live_hashes(&self) -> GcResult<HashSet<String>> {
        Ok(self.hashes.clone())
    }
}

struct NoopInventory;

impl AssetInventoryStore for NoopInventory {
    fn upsert_active_seen(&self, _run_id: &str, _hash: &str, _now: SystemTime) -> GcResult<()> {
        Ok(())
    }

    fn iter_unmarked_candidates(
        &self,
        _run_id: &str,
    ) -> GcResult<Box<dyn Iterator<Item = AssetRecord> + Send>> {
        Ok(Box::new(Vec::<AssetRecord>::new().into_iter()))
    }

    fn iter_pending_older_than(
        &self,
        _cutoff: SystemTime,
    ) -> GcResult<Box<dyn Iterator<Item = AssetRecord> + Send>> {
        Ok(Box::new(Vec::<AssetRecord>::new().into_iter()))
    }

    fn set_pending_delete(&self, _hash: &str, _at: SystemTime) -> GcResult<()> {
        Ok(())
    }

    fn set_deleted(&self, _hash: &str, _at: SystemTime) -> GcResult<()> {
        Ok(())
    }

    fn clear_pending_delete(&self, _hash: &str) -> GcResult<()> {
        Ok(())
    }
}

struct NoopRunStore {
    room_id: String,
}

impl GcRunStore for NoopRunStore {
    fn start_run(&self, _start: GcRunStart) -> GcResult<String> {
        Ok(format!("server-markonly-{}", self.room_id))
    }

    fn apply_delta(&self, _run_id: &str, _delta: GcRunDelta) -> GcResult<()> {
        Ok(())
    }

    fn finish_run(&self, _run_id: &str, _finish: GcRunFinish) -> GcResult<()> {
        Ok(())
    }
}

struct NoopPins;

impl AdminPinStore for NoopPins {
    fn is_pinned(&self, _hash: &str) -> GcResult<bool> {
        Ok(false)
    }
}

struct NoopObjects;

impl BlobObjectStore for NoopObjects {
    fn head(&self, _bucket: &str, _key: &str) -> GcResult<bool> {
        Ok(false)
    }

    fn delete(&self, _bucket: &str, _key: &str) -> GcResult<()> {
        Ok(())
    }
}
