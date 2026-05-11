use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use activesync_gc::contracts::{
    AdminPinStore, AssetInventoryStore, BlobObjectStore, GcRunStore, LiveHashSource,
};
use activesync_gc::types::{AssetRecord, AssetState, GcRunDelta, GcRunFinish, GcRunMode, GcRunStart, GcRunStatus};
use activesync_gc::{GcCoordinator, GcCoordinatorConfig, GcError, GcResult};

#[derive(Default)]
struct TestLiveSource {
    live: Mutex<HashSet<String>>,
}
impl LiveHashSource for TestLiveSource {
    fn collect_live_hashes(&self) -> GcResult<HashSet<String>> {
        Ok(self.live.lock().expect("live mutex").clone())
    }
}

#[derive(Default)]
struct TestPins {
    pins: Mutex<HashSet<String>>,
}
impl AdminPinStore for TestPins {
    fn is_pinned(&self, hash: &str) -> GcResult<bool> {
        Ok(self.pins.lock().expect("pins mutex").contains(hash))
    }
}

#[derive(Default)]
struct TestRuns {
    started: Mutex<Vec<GcRunStart>>,
    deltas: Mutex<Vec<GcRunDelta>>,
    finished: Mutex<Vec<GcRunFinish>>,
}
impl GcRunStore for TestRuns {
    fn start_run(&self, start: GcRunStart) -> GcResult<String> {
        self.started.lock().expect("started mutex").push(start);
        Ok(format!("run-{}", self.started.lock().expect("started mutex").len()))
    }

    fn apply_delta(&self, _run_id: &str, delta: GcRunDelta) -> GcResult<()> {
        self.deltas.lock().expect("deltas mutex").push(delta);
        Ok(())
    }

    fn finish_run(&self, _run_id: &str, finish: GcRunFinish) -> GcResult<()> {
        self.finished.lock().expect("finished mutex").push(finish);
        Ok(())
    }
}

#[derive(Default)]
struct TestObjects {
    existing: Mutex<HashSet<(String, String)>>,
    deleted: Mutex<Vec<(String, String)>>,
}
impl BlobObjectStore for TestObjects {
    fn head(&self, bucket: &str, key: &str) -> GcResult<bool> {
        Ok(self
            .existing
            .lock()
            .expect("existing mutex")
            .contains(&(bucket.to_string(), key.to_string())))
    }

    fn delete(&self, bucket: &str, key: &str) -> GcResult<()> {
        self.deleted
            .lock()
            .expect("deleted mutex")
            .push((bucket.to_string(), key.to_string()));
        self.existing
            .lock()
            .expect("existing mutex")
            .remove(&(bucket.to_string(), key.to_string()));
        Ok(())
    }
}

#[derive(Default)]
struct TestInventory {
    records: Mutex<HashMap<String, AssetRecord>>,
}
impl TestInventory {
    fn seed(&self, hash: &str, state: AssetState, pending_delete_at: Option<SystemTime>, pinned: bool) {
        let now = SystemTime::now();
        let rec = AssetRecord {
            hash: hash.to_string(),
            object_key: format!("k/{hash}"),
            bucket: "b".to_string(),
            namespace: "n".to_string(),
            first_seen_at: now,
            last_seen_at: now,
            state,
            pending_delete_at,
            deleted_at: None,
            last_marked_run_id: None,
            mark_count: 0,
            size_bytes: None,
            content_type: None,
            is_admin_pinned: pinned,
            pin_reason: None,
            updated_at: now,
        };
        self.records.lock().expect("records mutex").insert(hash.to_string(), rec);
    }

    fn state(&self, hash: &str) -> Option<AssetState> {
        self.records.lock().expect("records mutex").get(hash).map(|r| r.state)
    }
}

impl AssetInventoryStore for TestInventory {
    fn upsert_active_seen(&self, run_id: &str, hash: &str, now: SystemTime) -> GcResult<()> {
        let mut records = self.records.lock().expect("records mutex");
        let rec = records.entry(hash.to_string()).or_insert_with(|| AssetRecord {
            hash: hash.to_string(),
            object_key: format!("k/{hash}"),
            bucket: "b".to_string(),
            namespace: "n".to_string(),
            first_seen_at: now,
            last_seen_at: now,
            state: AssetState::Active,
            pending_delete_at: None,
            deleted_at: None,
            last_marked_run_id: None,
            mark_count: 0,
            size_bytes: None,
            content_type: None,
            is_admin_pinned: false,
            pin_reason: None,
            updated_at: now,
        });
        rec.last_seen_at = now;
        rec.updated_at = now;
        rec.state = AssetState::Active;
        rec.pending_delete_at = None;
        rec.last_marked_run_id = Some(run_id.to_string());
        rec.mark_count += 1;
        Ok(())
    }

    fn iter_unmarked_candidates(
        &self,
        run_id: &str,
    ) -> GcResult<Box<dyn Iterator<Item = AssetRecord> + Send>> {
        let run = run_id.to_string();
        let snapshot: Vec<AssetRecord> = self
            .records
            .lock()
            .expect("records mutex")
            .values()
            .filter(|r| r.state != AssetState::Deleted)
            .filter(|r| r.last_marked_run_id.as_deref() != Some(run.as_str()))
            .cloned()
            .collect();
        Ok(Box::new(snapshot.into_iter()))
    }

    fn iter_pending_older_than(
        &self,
        cutoff: SystemTime,
    ) -> GcResult<Box<dyn Iterator<Item = AssetRecord> + Send>> {
        let snapshot: Vec<AssetRecord> = self
            .records
            .lock()
            .expect("records mutex")
            .values()
            .filter(|r| r.pending_delete_at.map(|t| t <= cutoff).unwrap_or(false))
            .cloned()
            .collect();
        Ok(Box::new(snapshot.into_iter()))
    }

    fn set_pending_delete(&self, hash: &str, at: SystemTime) -> GcResult<()> {
        let mut records = self.records.lock().expect("records mutex");
        let rec = records
            .get_mut(hash)
            .ok_or_else(|| GcError::Backend(format!("missing record: {hash}")))?;
        rec.pending_delete_at = Some(at);
        rec.state = AssetState::PendingDelete;
        rec.updated_at = at;
        Ok(())
    }

    fn set_deleted(&self, hash: &str, at: SystemTime) -> GcResult<()> {
        let mut records = self.records.lock().expect("records mutex");
        let rec = records
            .get_mut(hash)
            .ok_or_else(|| GcError::Backend(format!("missing record: {hash}")))?;
        rec.state = AssetState::Deleted;
        rec.deleted_at = Some(at);
        rec.updated_at = at;
        Ok(())
    }

    fn clear_pending_delete(&self, hash: &str) -> GcResult<()> {
        if let Some(rec) = self.records.lock().expect("records mutex").get_mut(hash) {
            rec.pending_delete_at = None;
        }
        Ok(())
    }
}

fn mk_coordinator(
    live: Arc<TestLiveSource>,
    inventory: Arc<TestInventory>,
    runs: Arc<TestRuns>,
    pins: Arc<TestPins>,
    objects: Arc<TestObjects>,
    cfg: GcCoordinatorConfig,
) -> GcCoordinator<TestLiveSource, TestInventory, TestRuns, TestPins, TestObjects> {
    GcCoordinator::new(live, inventory, runs, pins, objects, cfg)
}

#[test]
fn dry_run_marks_without_pending_or_delete() {
    let now = SystemTime::now();
    let live = Arc::new(TestLiveSource::default());
    live.live.lock().expect("live mutex").insert("h1".to_string());

    let inventory = Arc::new(TestInventory::default());
    let runs = Arc::new(TestRuns::default());
    let pins = Arc::new(TestPins::default());
    let objects = Arc::new(TestObjects::default());
    let gc = mk_coordinator(live, Arc::clone(&inventory), Arc::clone(&runs), pins, objects, GcCoordinatorConfig::default());

    let delta = gc.run_once(GcRunMode::DryRun, now).expect("run dry");
    assert_eq!(delta.marked_count, 1);
    assert_eq!(delta.newly_pending_count, 0);
    assert_eq!(delta.hard_deleted_count, 0);
    assert_eq!(inventory.state("h1"), Some(AssetState::Active));
}

#[test]
fn sweep_soft_moves_unmarked_and_skips_pinned() {
    let now = SystemTime::now();
    let live = Arc::new(TestLiveSource::default());
    live.live.lock().expect("live mutex").insert("live".to_string());

    let inventory = Arc::new(TestInventory::default());
    inventory.seed("live", AssetState::Active, None, false);
    inventory.seed("stale", AssetState::Active, None, false);
    inventory.seed("pinned", AssetState::Active, None, true);

    let runs = Arc::new(TestRuns::default());
    let pins = Arc::new(TestPins::default());
    let objects = Arc::new(TestObjects::default());
    let gc = mk_coordinator(live, Arc::clone(&inventory), runs, pins, objects, GcCoordinatorConfig::default());

    let delta = gc.run_once(GcRunMode::SweepSoft, now).expect("run soft");
    assert_eq!(delta.marked_count, 1);
    assert_eq!(delta.newly_pending_count, 1);
    assert_eq!(delta.skipped_pinned_count, 1);
    assert_eq!(inventory.state("stale"), Some(AssetState::PendingDelete));
    assert_eq!(inventory.state("pinned"), Some(AssetState::Active));
}

#[test]
fn sweep_hard_enforces_grace_and_head_gate() {
    let now = SystemTime::now();
    let old = now - Duration::from_secs(3600);

    let live = Arc::new(TestLiveSource::default());
    let inventory = Arc::new(TestInventory::default());
    inventory.seed("ready", AssetState::PendingDelete, Some(old), false);
    inventory.seed("young", AssetState::PendingDelete, Some(now), false);

    let runs = Arc::new(TestRuns::default());
    let pins = Arc::new(TestPins::default());
    let objects = Arc::new(TestObjects::default());
    objects
        .existing
        .lock()
        .expect("existing mutex")
        .insert(("b".to_string(), "k/ready".to_string()));

    let cfg = GcCoordinatorConfig {
        grace_window: Duration::from_secs(300),
        max_deletes_per_run: 100,
        require_head_before_delete: true,
    };
    let gc = mk_coordinator(live, Arc::clone(&inventory), runs, pins, Arc::clone(&objects), cfg);

    let delta = gc.run_once(GcRunMode::SweepHard, now).expect("run hard");
    assert_eq!(delta.hard_deleted_count, 1);
    assert_eq!(inventory.state("ready"), Some(AssetState::Deleted));
    assert_eq!(inventory.state("young"), Some(AssetState::PendingDelete));
    assert_eq!(objects.deleted.lock().expect("deleted mutex").len(), 1);
}

#[test]
fn live_again_cancels_pending_and_restores_active() {
    let now = SystemTime::now();
    let old = now - Duration::from_secs(3600);

    let live = Arc::new(TestLiveSource::default());
    live.live.lock().expect("live mutex").insert("h-live-again".to_string());

    let inventory = Arc::new(TestInventory::default());
    inventory.seed("h-live-again", AssetState::PendingDelete, Some(old), false);

    let runs = Arc::new(TestRuns::default());
    let pins = Arc::new(TestPins::default());
    let objects = Arc::new(TestObjects::default());
    let gc = mk_coordinator(live, Arc::clone(&inventory), runs, pins, objects, GcCoordinatorConfig::default());

    let delta = gc.run_once(GcRunMode::MarkOnly, now).expect("run markonly");
    assert_eq!(delta.marked_count, 1);
    assert_eq!(inventory.state("h-live-again"), Some(AssetState::Active));
}

#[test]
fn run_status_is_recorded_as_succeeded() {
    let now = SystemTime::now();
    let live = Arc::new(TestLiveSource::default());
    let inventory = Arc::new(TestInventory::default());
    let runs = Arc::new(TestRuns::default());
    let pins = Arc::new(TestPins::default());
    let objects = Arc::new(TestObjects::default());
    let gc = mk_coordinator(live, inventory, Arc::clone(&runs), pins, objects, GcCoordinatorConfig::default());

    let _ = gc.run_once(GcRunMode::MarkOnly, now).expect("run");
    let finished = runs.finished.lock().expect("finished mutex");
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].status, GcRunStatus::Succeeded);
}
