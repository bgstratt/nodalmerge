//! blob-cas-remediation.md slice 6.3 — GC mark-pass batching.
//!
//! The coordinator's mark pass used to issue one autocommit SQLite
//! transaction per live hash (`gc_store.rs`'s `upsert_active_seen`): 100k
//! hashes = 100k txns. Slice 6.3 routes the whole pass through
//! `AssetInventoryStore::upsert_active_seen_batch`, which `SqliteGcStore`
//! implements as ONE transaction over one cached prepared statement.
//!
//! Two kinds of test here, per the slice's validation shape:
//!
//! * plain `cargo test`-runnable smokes asserting **behavior equality** —
//!   the batched pass must leave the inventory in exactly the state the
//!   per-row pass would, driven through the real `GcCoordinator` (the code
//!   path production takes), not just the store method;
//! * `#[ignore]`d bench-style tests with a deliberately **loose** sanity
//!   bound (batched ≥ 3x faster). Timing tests with tight bounds are flaky
//!   by construction, so CI runs only the smokes; the benches exist to be
//!   run by hand (`cargo test --test gc_io_batching -- --ignored --nocapture`)
//!   and their measured numbers are recorded in the slice report.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use nodalmerge_gc::contracts::{AssetInventoryStore, LiveHashSource};
use nodalmerge_gc::types::{AssetRecord, AssetState, GcRunMode};
use nodalmerge_gc::{GcCoordinator, GcCoordinatorConfig, GcResult};
use nodalmerge_server::gc_blob_objects::LocalBlobObjectStore;
use nodalmerge_server::gc_pin_store::StaticPinStore;
use nodalmerge_server::gc_store::{local_key_scheme, SqliteGcStore};

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("nodalmerge-6.3-{tag}-{nanos}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Fixed live set — the coordinator's mark input, without needing rooms.
struct FixedLive(HashSet<String>);

impl LiveHashSource for FixedLive {
    fn collect_live_hashes(&self) -> GcResult<HashSet<String>> {
        Ok(self.0.clone())
    }
}

fn fake_hashes(n: usize) -> Vec<String> {
    // Deterministic 64-hex strings, distinct per index. Content is
    // irrelevant to the inventory — it stores them as opaque keys.
    (0..n).map(|i| format!("{i:064x}")).collect()
}

/// Sorted full records via the sentinel enumeration (the same load-bearing
/// use of `iter_unmarked_candidates` slice 1.3 documented).
fn all_records(store: &SqliteGcStore) -> Vec<AssetRecord> {
    let mut v: Vec<AssetRecord> = store
        .iter_unmarked_candidates("_gc-io-batching-scan")
        .unwrap()
        .collect();
    v.sort_by(|a, b| a.hash.cmp(&b.hash));
    v
}

/// The real coordinator's MarkOnly pass over `live`, against a fresh store.
fn run_mark_only(dir: &std::path::Path, live: &[String]) -> (Arc<SqliteGcStore>, u64) {
    let inventory = Arc::new(SqliteGcStore::open(dir, local_key_scheme()).unwrap());
    let runs = Arc::clone(&inventory);
    let coordinator = GcCoordinator::new(
        Arc::new(FixedLive(live.iter().cloned().collect())),
        Arc::clone(&inventory),
        runs,
        Arc::new(StaticPinStore::new([])),
        Arc::new(LocalBlobObjectStore::new(dir)),
        GcCoordinatorConfig::default(),
    );
    let delta = coordinator
        .run_once(GcRunMode::MarkOnly, SystemTime::now())
        .expect("mark-only run succeeds");
    (inventory, delta.marked_count)
}

/// Behavior-equality smoke: the coordinator's (now batched) mark pass must
/// leave the inventory row-for-row identical to a hand-rolled per-row pass
/// with the same run id and timestamp, and report the same marked_count.
#[test]
fn coordinator_mark_pass_is_row_identical_to_a_per_row_pass() {
    let live = fake_hashes(500);
    let now = SystemTime::now();

    // Batched (production path): through the real coordinator.
    let (batched_store, marked) = run_mark_only(&tmpdir("batched"), &live);
    assert_eq!(marked, 500, "every live hash must be counted as marked");
    let batched = all_records(&batched_store);

    // Reference: the pre-6.3 shape, one upsert_active_seen per hash. Not
    // through the coordinator (it batches now — that's the point), but with
    // the batched run's own id and a fixed timestamp so rows are comparable
    // field-for-field modulo the run timestamp.
    let per_row_store = SqliteGcStore::open(tmpdir("per-row"), local_key_scheme()).unwrap();
    let run_id = batched[0]
        .last_marked_run_id
        .clone()
        .expect("marked rows carry the run id");
    for h in &live {
        per_row_store.upsert_active_seen(&run_id, h, now).unwrap();
    }
    let per_row = all_records(&per_row_store);

    assert_eq!(batched.len(), per_row.len());
    for (b, p) in batched.iter().zip(per_row.iter()) {
        assert_eq!(b.hash, p.hash);
        assert_eq!(b.object_key, p.object_key);
        assert_eq!(b.bucket, p.bucket);
        assert_eq!(b.state, p.state);
        assert_eq!(b.last_marked_run_id, p.last_marked_run_id);
        assert_eq!(b.mark_count, p.mark_count);
        assert_eq!(b.is_admin_pinned, p.is_admin_pinned);
    }
}

/// The mark's *protective* semantics must survive batching: a second run
/// that still sees a hash as live re-marks it (conflict arm), and a hash
/// the second run does NOT see becomes an unmarked candidate — the exact
/// mechanism sweeps select victims by.
#[test]
fn batched_marks_keep_mark_run_semantics_across_runs() {
    let dir = tmpdir("across-runs");
    let live_run1 = fake_hashes(20);
    let (inventory, _) = run_mark_only(&dir, &live_run1);

    // Second run over the same store: half the set is still live.
    let still_live: Vec<String> = live_run1[..10].to_vec();
    let runs = Arc::clone(&inventory);
    let coordinator = GcCoordinator::new(
        Arc::new(FixedLive(still_live.iter().cloned().collect())),
        Arc::clone(&inventory),
        runs,
        Arc::new(StaticPinStore::new([])),
        Arc::new(LocalBlobObjectStore::new(&dir)),
        GcCoordinatorConfig::default(),
    );
    let delta = coordinator
        .run_once(GcRunMode::MarkOnly, SystemTime::now())
        .expect("second mark-only run succeeds");
    assert_eq!(delta.marked_count, 10);

    let run2_id = all_records(&inventory)
        .iter()
        .find(|r| still_live.contains(&r.hash))
        .and_then(|r| r.last_marked_run_id.clone())
        .expect("run-2 id on a re-marked row");

    // Candidates for run 2 = exactly the 10 hashes it did not mark.
    let candidates: HashSet<String> = inventory
        .iter_unmarked_candidates(&run2_id)
        .unwrap()
        .map(|r| r.hash)
        .collect();
    let expected: HashSet<String> = live_run1[10..].iter().cloned().collect();
    assert_eq!(candidates, expected, "unmarked-candidate selection must be unchanged by batching");

    // Re-marked rows bumped their mark_count (conflict arm ran).
    for rec in all_records(&inventory) {
        let expected_count = if still_live.contains(&rec.hash) { 2 } else { 1 };
        assert_eq!(rec.mark_count, expected_count, "hash {}", rec.hash);
        assert_eq!(rec.state, AssetState::Active);
    }
}

/// Bench (not CI-gating): per-row vs batched mark pass at 100k hashes.
/// Loose ≥3x sanity bound only — the real numbers go in the slice report.
#[test]
#[ignore = "bench: run by hand with --ignored --nocapture (slice 6.3 perf validation)"]
fn bench_mark_pass_batched_vs_per_row_100k() {
    const N: usize = 100_000;
    let hashes = fake_hashes(N);
    let hash_refs: Vec<&str> = hashes.iter().map(String::as_str).collect();
    let now = SystemTime::now();

    let per_row_store = SqliteGcStore::open(tmpdir("bench-per-row"), local_key_scheme()).unwrap();
    let t0 = Instant::now();
    for h in &hash_refs {
        per_row_store.upsert_active_seen("bench-run", h, now).unwrap();
    }
    let per_row = t0.elapsed();

    let batched_store = SqliteGcStore::open(tmpdir("bench-batched"), local_key_scheme()).unwrap();
    let t0 = Instant::now();
    batched_store
        .upsert_active_seen_batch("bench-run", &hash_refs, now)
        .unwrap();
    let batched = t0.elapsed();

    println!(
        "mark pass over {N} hashes: per-row {per_row:?} ({:.0}/s), batched {batched:?} ({:.0}/s), speedup {:.1}x",
        N as f64 / per_row.as_secs_f64(),
        N as f64 / batched.as_secs_f64(),
        per_row.as_secs_f64() / batched.as_secs_f64(),
    );

    // Both must have actually marked everything.
    assert_eq!(all_records(&per_row_store).len(), N);
    assert_eq!(all_records(&batched_store).len(), N);

    assert!(
        batched < per_row / 3,
        "batched mark pass should be at least ~3x faster (loose sanity bound, not a \
         CI gate): per-row {per_row:?} vs batched {batched:?}"
    );
}
