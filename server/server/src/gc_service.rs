//! S5.3 — staged GC coordinator wiring + scheduling.
//!
//! Ties together an injected [`LiveHashCollector`] (slice 7.5 — the
//! composition layer supplies it; the studio-domain one lives in
//! `studio_live_hashes.rs` and this module never names it), the real
//! `AssetInventoryStore`/`GcRunStore` (`gc_store.rs`), the config-driven
//! `AdminPinStore` (`gc_pin_store.rs`), and a `BlobObjectStore`
//! (`gc_blob_objects.rs` locally, or `nodalmerge-s3-blobs`'s S3 variant —
//! this module is generic over that piece so it never needs to know which)
//! into one schedulable GC service, coexisting with the legacy
//! `Rooms::sweep_blobs` path behind a single `--gc-mode` flag.
//!
//! ## Mode coexistence (`GcMode`)
//!
//! One flag governs everything, reusing the existing `--blob-gc-interval`
//! scheduling loop rather than adding a second one:
//!
//! * `off` — GC fully disabled (neither path runs). Useful for tests that
//!   want zero background activity.
//! * `legacy` (**default**) — today's exact behavior, unchanged: the no-op
//!   `MarkOnly` preflight (`gc_adapter::run_mark_only_preflight`, still
//!   exercising the shared coordinator contracts against noop stores) plus
//!   the real deletion path, `BlobPersistence::blob_gc_sweep`'s
//!   tombstone-then-grace-then-delete dance. Chosen as the default
//!   specifically so landing this slice changes nothing about production
//!   behavior until an operator opts in.
//! * `dryrun` / `markonly` / `sweepsoft` / `sweephard` — the new
//!   coordinator, driven by real stores, fully replaces the legacy sweep for
//!   that tick (both own "does this need deleting" now, so running both
//!   would double up bookkeeping without adding safety).
//!
//! `GcMode::parse` is the single source of truth for the flag's string
//! values; both `main.rs` and `server-s3/main.rs` call it.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use nodalmerge_gc::contracts::{BlobObjectStore, LiveHashSource};
use nodalmerge_gc::types::{GcRunDelta, GcRunMode};
use nodalmerge_gc::{GcCoordinator, GcCoordinatorConfig, GcError, GcResult};

use crate::gc_pin_store::StaticPinStore;
use crate::gc_store::SqliteGcStore;
use crate::room::Rooms;

/// Slice 7.5 — the pluggable live-set factory. The coordinator's own
/// `LiveHashSource` trait is sync (it runs inside `run_once`), but every
/// real live-set computation on this server is async (room reads, cold
/// replays, tree walks through the 6.1 bridge), so what the composition
/// layer injects is this async twin: called once per tick, up front, and
/// the result wrapped in [`PrecomputedLiveHashSource`] — the same
/// precompute-then-wrap dance S5.3 introduced, minus the hard-wired callee.
///
/// Threaded through [`run_new_coordinator_once`]/[`spawn_gc_sweeper`] the
/// way `BlobObjectStore` already is: a generic `Arc` the binaries supply
/// (`main.rs`/`server-s3/main.rs` build the slice-1.2 union of the
/// studio-domain collector from `studio_live_hashes.rs` and the room-DAG
/// collector from `room.rs`; tests hand in whatever they like). This
/// module has no compile-time knowledge of any concrete source.
pub trait LiveHashCollector: Send + Sync {
    /// Compute the full live set for one coordinator run. An `Err` is a
    /// failed run: `GcCoordinator::run_once` fail-closed semantics apply
    /// (no mutation, run ledger records `Failed`).
    fn collect_live_hashes(
        &self,
    ) -> Pin<Box<dyn Future<Output = GcResult<HashSet<String>>> + Send + '_>>;
}

/// Slice 7.5 — set-union composition of live-hash sources, built for 1.2's
/// "studio hashes ∪ room-DAG blob references" (7.5 shipped the combinator;
/// 1.2 wired `room.rs`'s `RoomDagLiveHashCollector` in as the second member
/// in both binaries).
///
/// Fail-closed on BOTH axes, per the GC discipline everywhere else in this
/// plan:
///
/// * **any member errors → the union errors.** A source that fails must not
///   silently contribute nothing — "nothing" here means "nothing is live",
///   i.e. delete it all (1.3 filed the fail-open family; this refuses to be
///   the next instance).
/// * **zero members → error.** An empty union's honest answer is the empty
///   set, and an empty live set tells sweephard to reclaim every blob on
///   the server. No real composition wants that; it's a wiring bug, so
///   refuse loudly rather than report it as truth (same posture as 1.1's
///   "can't enumerate → refuse to delete").
pub struct UnionLiveHashCollector {
    sources: Vec<Arc<dyn LiveHashCollector>>,
}

impl UnionLiveHashCollector {
    pub fn new(sources: Vec<Arc<dyn LiveHashCollector>>) -> Self {
        Self { sources }
    }
}

impl LiveHashCollector for UnionLiveHashCollector {
    fn collect_live_hashes(
        &self,
    ) -> Pin<Box<dyn Future<Output = GcResult<HashSet<String>>> + Send + '_>> {
        Box::pin(async move {
            if self.sources.is_empty() {
                return Err(GcError::Invariant(
                    "union of zero live-hash sources (an empty live set would mark \
                     everything reclaimable; this is a wiring bug, not a live set)"
                        .to_string(),
                ));
            }
            let mut union: HashSet<String> = HashSet::new();
            for (idx, source) in self.sources.iter().enumerate() {
                // Sequential on purpose: sources share the room locks and
                // the blocking pool, and the first failure aborts the run —
                // there is nothing to win by racing them.
                let set = source.collect_live_hashes().await.map_err(|e| {
                    GcError::Backend(format!("live-hash source {idx}: {e}"))
                })?;
                union.extend(set);
            }
            Ok(union)
        })
    }
}

/// The `--gc-mode` flag's value space. See module docs for how the two
/// deletion paths coexist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcMode {
    Off,
    Legacy,
    New(GcRunMode),
}

impl GcMode {
    /// Parse one of `off | legacy | dryrun | markonly | sweepsoft |
    /// sweephard`. `None` on anything else (callers should warn + fall back
    /// to the default, matching every other CLI flag's error posture here).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(GcMode::Off),
            "legacy" => Some(GcMode::Legacy),
            "dryrun" => Some(GcMode::New(GcRunMode::DryRun)),
            "markonly" => Some(GcMode::New(GcRunMode::MarkOnly)),
            "sweepsoft" => Some(GcMode::New(GcRunMode::SweepSoft)),
            "sweephard" => Some(GcMode::New(GcRunMode::SweepHard)),
            _ => None,
        }
    }
}

impl Default for GcMode {
    fn default() -> Self {
        GcMode::Legacy
    }
}

/// Runtime configuration for the new staged coordinator (ignored entirely
/// in `Legacy`/`Off` modes, where the legacy grace window — `--blob-gc-grace`
/// — governs instead).
#[derive(Debug, Clone)]
pub struct GcServiceConfig {
    pub mode: GcMode,
    /// Grace window between soft-tombstone and hard-delete eligibility.
    /// Default 24 h per `docs/delegated-storage-gc.md`'s safety defaults;
    /// tests override this to something small.
    pub grace: Duration,
    pub max_deletes_per_run: u64,
    pub require_head_before_delete: bool,
}

impl Default for GcServiceConfig {
    fn default() -> Self {
        Self {
            mode: GcMode::default(),
            grace: Duration::from_secs(24 * 60 * 60),
            max_deletes_per_run: 100,
            require_head_before_delete: true,
        }
    }
}

/// Bridges the coordinator's sync `LiveHashSource` trait to the
/// asynchronously-computed studio live-hash set. The live set (or its
/// failure) is computed once, up front (see [`run_new_coordinator_once`]),
/// avoiding any sync↔async bridging inside the trait method itself — no
/// nested-runtime, no `block_on`/`blocking_read` risk. A stored `Err` still
/// propagates through `collect_live_hashes` exactly as a live failure would,
/// so `GcCoordinator::run_once`'s existing fail-closed behavior (no mutation,
/// run ledger records `Failed`) is preserved unchanged.
struct PrecomputedLiveHashSource {
    result: Mutex<Option<GcResult<HashSet<String>>>>,
}

impl PrecomputedLiveHashSource {
    fn new(result: GcResult<HashSet<String>>) -> Self {
        Self { result: Mutex::new(Some(result)) }
    }
}

impl LiveHashSource for PrecomputedLiveHashSource {
    fn collect_live_hashes(&self) -> GcResult<HashSet<String>> {
        self.result
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| Err(GcError::Backend("live hash set already consumed".to_string())))
    }
}

/// Run the new staged coordinator once, in `mode`, against real stores.
/// Generic over the blob-object backend so this crate never needs to know
/// about S3 (`nodalmerge-s3-blobs` supplies its own `BlobObjectStore` impl
/// and calls this the same way `main.rs`/`server-s3/main.rs` do for local
/// disk), and — slice 7.5 — generic over the live-set factory for the same
/// reason: the studio-domain source is a composition-layer choice, not this
/// module's business.
pub async fn run_new_coordinator_once<L: LiveHashCollector + ?Sized, B: BlobObjectStore>(
    live: &L,
    mode: GcRunMode,
    cfg: &GcServiceConfig,
    inventory: Arc<SqliteGcStore>,
    pins: Arc<StaticPinStore>,
    objects: Arc<B>,
) -> GcResult<GcRunDelta> {
    let live_result = live.collect_live_hashes().await;
    let live_source = Arc::new(PrecomputedLiveHashSource::new(live_result));
    let coordinator_cfg = GcCoordinatorConfig {
        grace_window: cfg.grace,
        max_deletes_per_run: cfg.max_deletes_per_run,
        require_head_before_delete: cfg.require_head_before_delete,
    };
    // `inventory` doubles as both the `AssetInventoryStore` and `GcRunStore`
    // generic slots — `SqliteGcStore` implements both.
    let runs = Arc::clone(&inventory);
    let coordinator = GcCoordinator::new(live_source, inventory, runs, pins, objects, coordinator_cfg);
    coordinator.run_once(mode, SystemTime::now())
}

async fn run_gc_tick<L: LiveHashCollector + ?Sized, B: BlobObjectStore>(
    rooms: &Rooms,
    cfg: &GcServiceConfig,
    live: &Arc<L>,
    inventory: &Arc<SqliteGcStore>,
    pins: &Arc<StaticPinStore>,
    objects: &Arc<B>,
) {
    match cfg.mode {
        GcMode::Off => {}
        GcMode::Legacy => {
            let _deleted = rooms.sweep_blobs(cfg.grace).await;
        }
        GcMode::New(mode) => {
            match run_new_coordinator_once(
                live.as_ref(),
                mode,
                cfg,
                Arc::clone(inventory),
                Arc::clone(pins),
                Arc::clone(objects),
            )
            .await
            {
                Ok(delta) => {
                    tracing::info!(
                        ?mode,
                        marked = delta.marked_count,
                        newly_pending = delta.newly_pending_count,
                        hard_deleted = delta.hard_deleted_count,
                        skipped_pinned = delta.skipped_pinned_count,
                        errors = delta.error_count,
                        "gc coordinator run complete"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        ?mode,
                        error = %e,
                        "gc coordinator run failed (fail-closed; no deletes performed this run)"
                    );
                }
            }
        }
    }
}

/// Spawn the unified GC background task — one interval, dispatching to
/// either the legacy sweep or the new coordinator per `cfg.mode` on every
/// tick (see module docs). `interval.is_zero()` disables scheduling
/// entirely (returns `None`), mirroring every other sweeper in `room.rs`.
/// `rooms` still rides along for the legacy `sweep_blobs` arm; the new
/// coordinator only ever sees the injected `live` collector.
pub fn spawn_gc_sweeper<
    L: LiveHashCollector + ?Sized + 'static,
    B: BlobObjectStore + Send + Sync + 'static,
>(
    rooms: Rooms,
    interval: Duration,
    cfg: GcServiceConfig,
    live: Arc<L>,
    inventory: Arc<SqliteGcStore>,
    pins: Arc<StaticPinStore>,
    objects: Arc<B>,
) -> Option<tokio::task::JoinHandle<()>> {
    if interval.is_zero() {
        return None;
    }
    Some(tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await; // skip the immediate t=0 tick
        loop {
            ticker.tick().await;
            run_gc_tick(&rooms, &cfg, &live, &inventory, &pins, &objects).await;
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gc_mode_parses_every_documented_value() {
        assert_eq!(GcMode::parse("off"), Some(GcMode::Off));
        assert_eq!(GcMode::parse("legacy"), Some(GcMode::Legacy));
        assert_eq!(GcMode::parse("dryrun"), Some(GcMode::New(GcRunMode::DryRun)));
        assert_eq!(GcMode::parse("markonly"), Some(GcMode::New(GcRunMode::MarkOnly)));
        assert_eq!(GcMode::parse("sweepsoft"), Some(GcMode::New(GcRunMode::SweepSoft)));
        assert_eq!(GcMode::parse("sweephard"), Some(GcMode::New(GcRunMode::SweepHard)));
        assert_eq!(GcMode::parse("bogus"), None);
    }

    #[test]
    fn default_mode_is_legacy() {
        assert_eq!(GcServiceConfig::default().mode, GcMode::Legacy);
    }

    // ─── Slice 7.5 — UnionLiveHashCollector semantics ──────────────────────

    /// Fixed-set collector for the union tests below (and nothing else —
    /// the end-to-end "generic service runs a non-studio source" pin lives
    /// in tests/gc_live_source_seam.rs).
    struct StaticCollector(HashSet<String>);

    impl LiveHashCollector for StaticCollector {
        fn collect_live_hashes(
            &self,
        ) -> Pin<Box<dyn Future<Output = GcResult<HashSet<String>>> + Send + '_>> {
            let set = self.0.clone();
            Box::pin(async move { Ok(set) })
        }
    }

    /// Always-failing collector — stands in for a source whose room reads /
    /// cold replays / tree walks fell over mid-collection.
    struct FailingCollector;

    impl LiveHashCollector for FailingCollector {
        fn collect_live_hashes(
            &self,
        ) -> Pin<Box<dyn Future<Output = GcResult<HashSet<String>>> + Send + '_>> {
            Box::pin(async { Err(GcError::Backend("collector exploded".to_string())) })
        }
    }

    fn set_of(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn union_is_the_set_union_of_its_members() {
        // Overlap on "b" on purpose: set semantics, not concatenation.
        let union = UnionLiveHashCollector::new(vec![
            Arc::new(StaticCollector(set_of(&["a", "b"]))),
            Arc::new(StaticCollector(set_of(&["b", "c"]))),
        ]);
        let live = union.collect_live_hashes().await.expect("both members succeed");
        assert_eq!(live, set_of(&["a", "b", "c"]));
    }

    #[tokio::test]
    async fn union_fails_closed_when_any_member_fails() {
        // Both orders: a failure must poison the union whether it comes
        // before or after a successful member — a source that errors must
        // not silently contribute nothing (1.3's fail-open family; this
        // combinator refuses to be the next instance).
        let fail_first = UnionLiveHashCollector::new(vec![
            Arc::new(FailingCollector) as Arc<dyn LiveHashCollector>,
            Arc::new(StaticCollector(set_of(&["a"]))),
        ]);
        assert!(fail_first.collect_live_hashes().await.is_err());

        let fail_last = UnionLiveHashCollector::new(vec![
            Arc::new(StaticCollector(set_of(&["a"]))) as Arc<dyn LiveHashCollector>,
            Arc::new(FailingCollector),
        ]);
        assert!(fail_last.collect_live_hashes().await.is_err());
    }

    #[tokio::test]
    async fn union_of_zero_sources_is_an_error_not_an_empty_live_set() {
        // An empty live set says "reclaim everything"; an empty union is a
        // wiring bug. Refuse loudly (same posture as 1.1's can't-enumerate
        // guard), never answer with the dangerous truth-shaped default.
        let union = UnionLiveHashCollector::new(Vec::new());
        assert!(union.collect_live_hashes().await.is_err());
    }
}
