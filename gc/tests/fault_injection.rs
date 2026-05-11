use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use activesync_gc::contracts::{
    AdminPinStore, AssetInventoryStore, BlobObjectStore, GcRunStore, LiveHashSource,
};
use activesync_gc::types::{AssetRecord, GcRunDelta, GcRunFinish, GcRunMode, GcRunStart, GcRunStatus};
use activesync_gc::{GcCoordinator, GcCoordinatorConfig, GcError, GcResult};

struct FailingLive;
impl LiveHashSource for FailingLive {
    fn collect_live_hashes(&self) -> GcResult<HashSet<String>> {
        Err(GcError::Backend("live source unavailable".to_string()))
    }
}

#[derive(Default)]
struct EmptyInventory;
impl AssetInventoryStore for EmptyInventory {
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

#[derive(Default)]
struct TestRuns {
    finished: Mutex<Vec<GcRunFinish>>,
}
impl GcRunStore for TestRuns {
    fn start_run(&self, _start: GcRunStart) -> GcResult<String> {
        Ok("run-1".to_string())
    }

    fn apply_delta(&self, _run_id: &str, _delta: GcRunDelta) -> GcResult<()> {
        Ok(())
    }

    fn finish_run(&self, _run_id: &str, finish: GcRunFinish) -> GcResult<()> {
        self.finished.lock().expect("finished mutex").push(finish);
        Ok(())
    }
}

#[derive(Default)]
struct NoPins;
impl AdminPinStore for NoPins {
    fn is_pinned(&self, _hash: &str) -> GcResult<bool> {
        Ok(false)
    }
}

#[derive(Default)]
struct NoObjects;
impl BlobObjectStore for NoObjects {
    fn head(&self, _bucket: &str, _key: &str) -> GcResult<bool> {
        Ok(false)
    }

    fn delete(&self, _bucket: &str, _key: &str) -> GcResult<()> {
        Ok(())
    }
}

#[test]
fn failed_run_sets_failed_status() {
    let runs = Arc::new(TestRuns::default());
    let gc = GcCoordinator::new(
        Arc::new(FailingLive),
        Arc::new(EmptyInventory),
        Arc::clone(&runs),
        Arc::new(NoPins),
        Arc::new(NoObjects),
        GcCoordinatorConfig::default(),
    );

    let err = gc
        .run_once(GcRunMode::MarkOnly, SystemTime::now())
        .expect_err("should fail from live source");
    assert!(matches!(err, GcError::Backend(_)));

    let finished = runs.finished.lock().expect("finished mutex");
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].status, GcRunStatus::Failed);
}
