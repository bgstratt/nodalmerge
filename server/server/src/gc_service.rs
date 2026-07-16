//! S5.3 — staged GC coordinator wiring + scheduling.
//!
//! Ties together the studio-domain `LiveHashSource`
//! (`studio_live_hashes.rs`), the real `AssetInventoryStore`/`GcRunStore`
//! (`gc_store.rs`), the config-driven `AdminPinStore` (`gc_pin_store.rs`),
//! and a `BlobObjectStore` (`gc_blob_objects.rs` locally, or
//! `nodalmerge-s3-blobs`'s S3 variant — this module is generic over that
//! last piece so it never needs to know which) into one schedulable GC
//! service, coexisting with the legacy `Rooms::sweep_blobs` path behind a
//! single `--gc-mode` flag.
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
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use nodalmerge_gc::contracts::{BlobObjectStore, LiveHashSource};
use nodalmerge_gc::types::{GcRunDelta, GcRunMode};
use nodalmerge_gc::{GcCoordinator, GcCoordinatorConfig, GcError, GcResult};

use crate::gc_pin_store::StaticPinStore;
use crate::gc_store::SqliteGcStore;
use crate::room::Rooms;
use crate::studio_live_hashes;

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
    /// Mirrors `RetentionPolicyOptions.RetainIntermediateDays` (default 30).
    pub retain_intermediate_days: i64,
}

impl Default for GcServiceConfig {
    fn default() -> Self {
        Self {
            mode: GcMode::default(),
            grace: Duration::from_secs(24 * 60 * 60),
            max_deletes_per_run: 100,
            require_head_before_delete: true,
            retain_intermediate_days: studio_live_hashes::DEFAULT_RETAIN_INTERMEDIATE_DAYS,
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
/// disk).
pub async fn run_new_coordinator_once<B: BlobObjectStore>(
    rooms: &Rooms,
    mode: GcRunMode,
    cfg: &GcServiceConfig,
    inventory: Arc<SqliteGcStore>,
    pins: Arc<StaticPinStore>,
    objects: Arc<B>,
) -> GcResult<GcRunDelta> {
    let live_result =
        studio_live_hashes::collect_studio_live_hashes(rooms, cfg.retain_intermediate_days).await;
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

async fn run_gc_tick<B: BlobObjectStore>(
    rooms: &Rooms,
    cfg: &GcServiceConfig,
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
                rooms,
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
pub fn spawn_gc_sweeper<B: BlobObjectStore + Send + Sync + 'static>(
    rooms: Rooms,
    interval: Duration,
    cfg: GcServiceConfig,
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
            run_gc_tick(&rooms, &cfg, &inventory, &pins, &objects).await;
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
}
