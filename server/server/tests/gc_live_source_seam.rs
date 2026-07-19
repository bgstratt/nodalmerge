//! Slice 7.5 — the "generic crate stands alone" pin: `gc_service` runs a
//! full staged GC against a live-hash source that has nothing to do with
//! the studio domain.
//!
//! **This file was impossible to write before the 7.5 refactor.** The old
//! entry point was
//!
//! ```text
//! pub async fn run_new_coordinator_once<B: BlobObjectStore>(
//!     rooms: &Rooms,
//!     mode: GcRunMode,
//!     cfg: &GcServiceConfig,
//!     inventory: Arc<SqliteGcStore>,
//!     pins: Arc<StaticPinStore>,
//!     objects: Arc<B>,
//! ) -> GcResult<GcRunDelta>
//! ```
//!
//! — no parameter accepts a live-hash source at all; the body called
//! `studio_live_hashes::collect_studio_live_hashes(rooms,
//! cfg.retain_intermediate_days)` unconditionally (gc_service.rs:148–149
//! pre-refactor), so the ONLY live set the service could ever compute was
//! the studio-domain one. Post-refactor the source arrives by injection
//! ([`gc_service::LiveHashCollector`]), the binaries' composition supplies
//! the studio collector explicitly, and this suite supplies trivial
//! non-studio ones. Note what is deliberately absent below: no
//! `studio_live_hashes` import, no `Rooms`, no room state of any kind.
//!
//! Behavior asserted here is the coordinator contract the studio_gc suite
//! already pins through the composed path — mark protects, sweepsoft
//! tombstones, sweephard deletes after grace, a failed source fails the
//! run closed — reached through a non-studio source for the first time.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use nodalmerge_core::Hash;
use nodalmerge_gc::contracts::AssetInventoryStore;
use nodalmerge_gc::types::{AssetState, GcRunMode};
use nodalmerge_gc::{GcError, GcResult};
use nodalmerge_server::gc_blob_objects::LocalBlobObjectStore;
use nodalmerge_server::gc_pin_store::StaticPinStore;
use nodalmerge_server::gc_service::{
    self, GcMode, GcServiceConfig, LiveHashCollector, UnionLiveHashCollector,
};
use nodalmerge_server::gc_store::{local_key_scheme, SqliteGcStore};
use nodalmerge_server::store::{BlobPersistence, DirPersistence};

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("nodalmerge-7.5-{tag}-{nanos}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// The trivial non-studio source: a fixed set of live hashes, decided by
/// the test, with no product-domain parsing anywhere behind it.
struct StaticLiveSource(HashSet<String>);

impl StaticLiveSource {
    fn of(hashes: &[&Hash]) -> Self {
        Self(hashes.iter().map(|h| h.to_hex()).collect())
    }
}

impl LiveHashCollector for StaticLiveSource {
    fn collect_live_hashes(
        &self,
    ) -> Pin<Box<dyn Future<Output = GcResult<HashSet<String>>> + Send + '_>> {
        let set = self.0.clone();
        Box::pin(async move { Ok(set) })
    }
}

/// A source whose collection fails — the run must fail closed.
struct FailingLiveSource;

impl LiveHashCollector for FailingLiveSource {
    fn collect_live_hashes(
        &self,
    ) -> Pin<Box<dyn Future<Output = GcResult<HashSet<String>>> + Send + '_>> {
        Box::pin(async { Err(GcError::Backend("live source unavailable".to_string())) })
    }
}

/// Same seeding the studio_gc suite uses: a sweep candidate must have been
/// seen live by a prior mark pass (or the upload-time hook) to hold an
/// inventory row at all.
fn seed_prior_active(inventory: &SqliteGcStore, hash: &Hash) {
    let past = std::time::SystemTime::now() - Duration::from_secs(7 * 24 * 3600);
    inventory.upsert_active_seen("prior-run", &hash.to_hex(), past).unwrap();
}

fn gc_cfg(mode: GcMode, grace: Duration) -> GcServiceConfig {
    GcServiceConfig { mode, grace, max_deletes_per_run: 100, require_head_before_delete: true }
}

#[tokio::test]
async fn generic_service_runs_full_staged_gc_with_a_non_studio_source() {
    let dir = tmpdir("static-source");
    let persistence = DirPersistence::open(&dir).unwrap();

    let (live, live_bytes) = (Hash::of(b"seam-live-blob"), b"seam-live-blob".to_vec());
    let (orphan, orphan_bytes) = (Hash::of(b"seam-orphan-blob"), b"seam-orphan-blob".to_vec());
    persistence.persist_blob(&live, &live_bytes).unwrap();
    persistence.persist_blob(&orphan, &orphan_bytes).unwrap();

    let inventory = Arc::new(SqliteGcStore::open(&dir, local_key_scheme()).unwrap());
    seed_prior_active(&inventory, &live);
    seed_prior_active(&inventory, &orphan);
    let pins = Arc::new(StaticPinStore::new(std::iter::empty()));
    let objects = Arc::new(LocalBlobObjectStore::new(&dir));

    // The live set comes from the test, not from any room or studio map.
    let source = StaticLiveSource::of(&[&live]);

    let grace = Duration::from_millis(50);
    let cfg_soft = gc_cfg(GcMode::New(GcRunMode::SweepSoft), grace);
    let delta = gc_service::run_new_coordinator_once(
        &source,
        GcRunMode::SweepSoft,
        &cfg_soft,
        Arc::clone(&inventory),
        Arc::clone(&pins),
        Arc::clone(&objects),
    )
    .await
    .expect("sweepsoft must succeed");
    assert_eq!(delta.newly_pending_count, 1, "exactly the orphan is tombstoned");
    assert_eq!(inventory.asset_state(&orphan.to_hex()), Some(AssetState::PendingDelete));
    assert_eq!(inventory.asset_state(&live.to_hex()), Some(AssetState::Active));

    tokio::time::sleep(Duration::from_millis(80)).await; // elapse grace

    let cfg_hard = gc_cfg(GcMode::New(GcRunMode::SweepHard), grace);
    let delta = gc_service::run_new_coordinator_once(
        &source,
        GcRunMode::SweepHard,
        &cfg_hard,
        Arc::clone(&inventory),
        Arc::clone(&pins),
        Arc::clone(&objects),
    )
    .await
    .expect("sweephard must succeed");
    assert_eq!(delta.hard_deleted_count, 1, "the orphan is reclaimed");

    let blake3_dir = dir.join("blobs").join("blake3");
    assert!(!blake3_dir.join(orphan.to_hex()).is_file(), "orphan deleted");
    assert!(blake3_dir.join(live.to_hex()).is_file(), "source-protected blob survives");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn failing_source_fails_the_run_closed_through_the_service() {
    // The precompute-then-wrap path must carry an injected source's Err all
    // the way into `GcCoordinator::run_once`'s fail-closed behavior: no
    // tombstone, no delete, run reports the failure.
    let dir = tmpdir("failing-source");
    let persistence = DirPersistence::open(&dir).unwrap();

    let (orphan, orphan_bytes) = (Hash::of(b"seam-fail-orphan"), b"seam-fail-orphan".to_vec());
    persistence.persist_blob(&orphan, &orphan_bytes).unwrap();

    let inventory = Arc::new(SqliteGcStore::open(&dir, local_key_scheme()).unwrap());
    seed_prior_active(&inventory, &orphan);
    let pins = Arc::new(StaticPinStore::new(std::iter::empty()));
    let objects = Arc::new(LocalBlobObjectStore::new(&dir));

    let cfg = gc_cfg(GcMode::New(GcRunMode::SweepHard), Duration::from_millis(1));
    let result = gc_service::run_new_coordinator_once(
        &FailingLiveSource,
        GcRunMode::SweepHard,
        &cfg,
        Arc::clone(&inventory),
        pins,
        objects,
    )
    .await;
    assert!(result.is_err(), "a failed live source must fail the run");
    assert_eq!(
        inventory.asset_state(&orphan.to_hex()),
        Some(AssetState::Active),
        "no state transition on a failed run"
    );
    assert!(
        dir.join("blobs").join("blake3").join(orphan.to_hex()).is_file(),
        "nothing deleted on a failed run"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn union_of_sources_composes_through_the_service() {
    // The 1.2 shape, end-to-end with placeholder members: two sources, each
    // protecting one blob; the union protects both through sweepsoft AND
    // sweephard, and only the orphan is reclaimed. (Slice 1.2 shipped: the
    // binaries' members are now the studio collector + room.rs's
    // `RoomDagLiveHashCollector`. This file stays deliberately
    // studio-free — see the module docs — so the members here remain
    // trivial stand-ins shaped like them; the same both-classes-survive
    // pin through the REAL collectors is studio_gc.rs's
    // `ordinary_setblob_blob_must_survive_sweepsoft_then_sweephard` +
    // `sweepsoft_then_sweephard_reclaims_orphan_and_respects_max_deletes`.)
    let dir = tmpdir("union-source");
    let persistence = DirPersistence::open(&dir).unwrap();

    let (a, a_bytes) = (Hash::of(b"seam-union-a"), b"seam-union-a".to_vec());
    let (b, b_bytes) = (Hash::of(b"seam-union-b"), b"seam-union-b".to_vec());
    let (orphan, orphan_bytes) = (Hash::of(b"seam-union-orphan"), b"seam-union-orphan".to_vec());
    persistence.persist_blob(&a, &a_bytes).unwrap();
    persistence.persist_blob(&b, &b_bytes).unwrap();
    persistence.persist_blob(&orphan, &orphan_bytes).unwrap();

    let inventory = Arc::new(SqliteGcStore::open(&dir, local_key_scheme()).unwrap());
    seed_prior_active(&inventory, &a);
    seed_prior_active(&inventory, &b);
    seed_prior_active(&inventory, &orphan);
    let pins = Arc::new(StaticPinStore::new(std::iter::empty()));
    let objects = Arc::new(LocalBlobObjectStore::new(&dir));

    let union = UnionLiveHashCollector::new(vec![
        Arc::new(StaticLiveSource::of(&[&a])) as Arc<dyn LiveHashCollector>,
        Arc::new(StaticLiveSource::of(&[&b])),
    ]);

    let grace = Duration::from_millis(50);
    let cfg = gc_cfg(GcMode::New(GcRunMode::SweepSoft), grace);
    let delta = gc_service::run_new_coordinator_once(
        &union,
        GcRunMode::SweepSoft,
        &cfg,
        Arc::clone(&inventory),
        Arc::clone(&pins),
        Arc::clone(&objects),
    )
    .await
    .expect("sweepsoft must succeed");
    assert_eq!(delta.newly_pending_count, 1, "only the orphan is tombstoned");
    assert_eq!(inventory.asset_state(&a.to_hex()), Some(AssetState::Active));
    assert_eq!(inventory.asset_state(&b.to_hex()), Some(AssetState::Active));
    assert_eq!(inventory.asset_state(&orphan.to_hex()), Some(AssetState::PendingDelete));

    tokio::time::sleep(Duration::from_millis(80)).await; // elapse grace

    // Slice 1.2 — extended through the hard sweep: each member's blob must
    // survive physical deletion (a member's contribution is honored even
    // when the OTHER member has never heard of the hash), and the orphan
    // must actually be reclaimed — the union composes protections without
    // becoming an everything-is-live-forever source.
    let cfg_hard = gc_cfg(GcMode::New(GcRunMode::SweepHard), grace);
    let delta = gc_service::run_new_coordinator_once(
        &union,
        GcRunMode::SweepHard,
        &cfg_hard,
        Arc::clone(&inventory),
        pins,
        objects,
    )
    .await
    .expect("sweephard must succeed");
    assert_eq!(delta.hard_deleted_count, 1, "exactly the orphan is reclaimed");

    let blake3_dir = dir.join("blobs").join("blake3");
    assert!(blake3_dir.join(a.to_hex()).is_file(), "member 1's blob survives the hard sweep");
    assert!(blake3_dir.join(b.to_hex()).is_file(), "member 2's blob survives the hard sweep");
    assert!(!blake3_dir.join(orphan.to_hex()).is_file(), "the orphan is physically deleted");

    let _ = std::fs::remove_dir_all(&dir);
}
