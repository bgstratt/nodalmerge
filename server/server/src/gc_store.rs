//! S5.3 — durable `AssetInventoryStore` + `GcRunStore`, backed by SQLite.
//!
//! Grace windows span process restarts (a blob tombstoned today must still
//! be eligible for hard delete after a restart 24 h later), so the noop
//! in-memory bridge `gc_adapter.rs` used for the MarkOnly preflight isn't
//! enough for real staged sweeps. This follows `store::DirPersistence`'s own
//! pattern (a single `Mutex<rusqlite::Connection>`, WAL mode, a store root
//! passed in at construction) rather than inventing a new storage mechanism
//! — see the S5.3 final report for why this is "the simplest honest option"
//! per the plan (Mongo/Postgres adapters are an explicit non-goal here; the
//! seam is the same `AssetInventoryStore`/`GcRunStore` traits, so a future
//! adapter is a new struct, not a change to this one).
//!
//! One SQLite file (`gc.db`), separate from `DirPersistence`'s own
//! `nodalmerge.db` — this keeps the GC ledger decoupled from the node/blob
//! store's own connection lifecycle (no invasive change to `DirPersistence`
//! needed to expose its private `Connection`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nodalmerge_gc::contracts::{AssetInventoryStore, GcRunStore};
use nodalmerge_gc::types::{AssetRecord, AssetState, GcRunDelta, GcRunFinish, GcRunMode, GcRunStart, GcRunStatus};
use nodalmerge_gc::{GcError, GcResult};
use rusqlite::{params, Connection};

/// How a hash maps onto `(bucket, object_key)` for whichever
/// `BlobObjectStore` backend is in play — local disk vs. S3 derive this
/// differently, and the ledger itself is backend-agnostic (per
/// `docs/delegated-storage-gc.md`'s portability checklist), so it's supplied
/// as an injected function rather than hard-coded here.
pub type KeyScheme = std::sync::Arc<dyn Fn(&str) -> (String, String) + Send + Sync>;

/// `(bucket, object_key) = ("local", "<hex-hash>")` — the convention
/// `gc_blob_objects::LocalBlobObjectStore` expects (it tries both the
/// identity and `.zst` on-disk forms for a given bare hash itself).
pub fn local_key_scheme() -> KeyScheme {
    std::sync::Arc::new(|hash: &str| ("local".to_string(), hash.to_string()))
}

static RUN_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

fn generate_run_id() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let n = RUN_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("gc-{}-{:x}", now.as_millis(), n)
}

fn to_unix_millis(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(e) => -(e.duration().as_millis() as i64),
    }
}

fn from_unix_millis(ms: i64) -> SystemTime {
    if ms >= 0 {
        UNIX_EPOCH + Duration::from_millis(ms as u64)
    } else {
        UNIX_EPOCH - Duration::from_millis((-ms) as u64)
    }
}

fn state_from_str(s: &str) -> AssetState {
    match s {
        "Uploading" => AssetState::Uploading,
        "Grace" => AssetState::Grace,
        "SweepCandidate" => AssetState::SweepCandidate,
        "Pinned" => AssetState::Pinned,
        "PendingDelete" => AssetState::PendingDelete,
        "Deleted" => AssetState::Deleted,
        "Quarantined" => AssetState::Quarantined,
        _ => AssetState::Active,
    }
}

fn mode_to_str(m: GcRunMode) -> &'static str {
    match m {
        GcRunMode::DryRun => "DryRun",
        GcRunMode::MarkOnly => "MarkOnly",
        GcRunMode::SweepSoft => "SweepSoft",
        GcRunMode::SweepHard => "SweepHard",
    }
}

fn status_to_str(s: GcRunStatus) -> &'static str {
    match s {
        GcRunStatus::Running => "Running",
        GcRunStatus::Succeeded => "Succeeded",
        GcRunStatus::Failed => "Failed",
    }
}

/// One statement shared by [`AssetInventoryStore::upsert_active_seen`] and
/// its slice-6.3 batch override — the batch is the same row write in a
/// transaction, and a drift between the two would mean a marked hash's row
/// depends on *which* code path marked it.
const UPSERT_ACTIVE_SEEN_SQL: &str = "INSERT INTO gc_assets
   (hash, object_key, bucket, namespace, first_seen_at, last_seen_at, state,
    pending_delete_at, deleted_at, last_marked_run_id, mark_count, is_admin_pinned, updated_at)
 VALUES (?1, ?2, ?3, 'blobs', ?4, ?4, 'Active', NULL, NULL, ?5, 1, 0, ?4)
 ON CONFLICT(hash) DO UPDATE SET
   last_seen_at = ?4,
   state = 'Active',
   pending_delete_at = NULL,
   last_marked_run_id = ?5,
   mark_count = gc_assets.mark_count + 1,
   updated_at = ?4";

/// SQLite-backed `AssetInventoryStore` + `GcRunStore`.
pub struct SqliteGcStore {
    conn: Mutex<Connection>,
    key_scheme: KeyScheme,
}

impl std::fmt::Debug for SqliteGcStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteGcStore").finish_non_exhaustive()
    }
}

impl SqliteGcStore {
    /// Open (or create) `<root>/gc.db`. `root` is the same store root
    /// `DirPersistence` uses (`<root>/nodalmerge.db`, `<root>/blobs/…`) —
    /// this just adds a sibling file, not a new directory tree.
    pub fn open(root: impl AsRef<Path>, key_scheme: KeyScheme) -> std::io::Result<Self> {
        let root: PathBuf = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let db_path = root.join("gc.db");
        let conn = Connection::open(&db_path)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS gc_assets (
               hash TEXT PRIMARY KEY,
               object_key TEXT NOT NULL,
               bucket TEXT NOT NULL,
               namespace TEXT NOT NULL,
               first_seen_at INTEGER NOT NULL,
               last_seen_at INTEGER NOT NULL,
               state TEXT NOT NULL,
               pending_delete_at INTEGER,
               deleted_at INTEGER,
               last_marked_run_id TEXT,
               mark_count INTEGER NOT NULL DEFAULT 0,
               size_bytes INTEGER,
               content_type TEXT,
               is_admin_pinned INTEGER NOT NULL DEFAULT 0,
               pin_reason TEXT,
               updated_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_gc_assets_pending ON gc_assets(pending_delete_at);
             CREATE INDEX IF NOT EXISTS idx_gc_assets_marked_run ON gc_assets(last_marked_run_id);
             CREATE TABLE IF NOT EXISTS gc_runs (
               run_id TEXT PRIMARY KEY,
               mode TEXT NOT NULL,
               started_at INTEGER NOT NULL,
               finished_at INTEGER,
               status TEXT,
               marked_count INTEGER NOT NULL DEFAULT 0,
               newly_pending_count INTEGER NOT NULL DEFAULT 0,
               hard_deleted_count INTEGER NOT NULL DEFAULT 0,
               skipped_pinned_count INTEGER NOT NULL DEFAULT 0,
               error_count INTEGER NOT NULL DEFAULT 0,
               notes TEXT
             );",
        )
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        Ok(Self { conn: Mutex::new(conn), key_scheme })
    }

    /// Test/observability helper: current status string of a run, if known.
    pub fn run_status(&self, run_id: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT status FROM gc_runs WHERE run_id = ?1",
            params![run_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    }

    /// Test/observability helper: current state of a hash's inventory row.
    pub fn asset_state(&self, hash: &str) -> Option<AssetState> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT state FROM gc_assets WHERE hash = ?1",
            params![hash],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .map(|s| state_from_str(&s))
    }

    fn row_to_record(
        hash: String,
        object_key: String,
        bucket: String,
        namespace: String,
        first_seen_at: i64,
        last_seen_at: i64,
        state: String,
        pending_delete_at: Option<i64>,
        deleted_at: Option<i64>,
        last_marked_run_id: Option<String>,
        mark_count: i64,
        size_bytes: Option<i64>,
        content_type: Option<String>,
        is_admin_pinned: i64,
        pin_reason: Option<String>,
        updated_at: i64,
    ) -> AssetRecord {
        AssetRecord {
            hash,
            object_key,
            bucket,
            namespace,
            first_seen_at: from_unix_millis(first_seen_at),
            last_seen_at: from_unix_millis(last_seen_at),
            state: state_from_str(&state),
            pending_delete_at: pending_delete_at.map(from_unix_millis),
            deleted_at: deleted_at.map(from_unix_millis),
            last_marked_run_id,
            mark_count: mark_count.max(0) as u64,
            size_bytes: size_bytes.map(|v| v.max(0) as u64),
            content_type,
            is_admin_pinned: is_admin_pinned != 0,
            pin_reason,
            updated_at: from_unix_millis(updated_at),
        }
    }
}

impl AssetInventoryStore for SqliteGcStore {
    fn upsert_active_seen(&self, run_id: &str, hash: &str, now: SystemTime) -> GcResult<()> {
        let now_ms = to_unix_millis(now);
        let (bucket, object_key) = (self.key_scheme)(hash);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            UPSERT_ACTIVE_SEEN_SQL,
            params![hash, object_key, bucket, now_ms, run_id],
        )
        .map_err(|e| GcError::Backend(e.to_string()))?;
        Ok(())
    }

    /// blob-cas-remediation.md slice 6.3 — the whole mark pass as ONE
    /// SQLite transaction with one cached prepared statement, instead of
    /// one autocommit per hash (100k hashes = 100k txns before this).
    ///
    /// Deliberately a *single* transaction rather than chunked ones: SQLite
    /// commits a 100k-row upsert txn comfortably (WAL, one journal sync at
    /// commit), and all-or-nothing is the easier invariant to reason about
    /// on a GC path — a mark pass that fails midway leaves the previous
    /// run's marks exactly as they were, the coordinator surfaces `Err`,
    /// and the run fails closed before any sweep phase. Chunking would buy
    /// nothing but a new partial-application state to think about.
    ///
    /// Holding the connection mutex for the whole batch is intentional too:
    /// the mark pass IS the GC's own critical section, and the only other
    /// writers on this file are the upload-time upserts in `blob_http.rs`,
    /// which merely block for the commit's duration (well under a second at
    /// 100k rows — measured in `tests/gc_io_batching.rs`).
    fn upsert_active_seen_batch(
        &self,
        run_id: &str,
        hashes: &[&str],
        now: SystemTime,
    ) -> GcResult<()> {
        if hashes.is_empty() {
            return Ok(());
        }
        let now_ms = to_unix_millis(now);
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| GcError::Backend(e.to_string()))?;
        {
            let mut stmt = tx
                .prepare_cached(UPSERT_ACTIVE_SEEN_SQL)
                .map_err(|e| GcError::Backend(e.to_string()))?;
            for hash in hashes {
                let (bucket, object_key) = (self.key_scheme)(hash);
                stmt.execute(params![hash, object_key, bucket, now_ms, run_id])
                    .map_err(|e| GcError::Backend(e.to_string()))?;
            }
        }
        tx.commit().map_err(|e| GcError::Backend(e.to_string()))?;
        Ok(())
    }

    fn iter_unmarked_candidates(
        &self,
        run_id: &str,
    ) -> GcResult<Box<dyn Iterator<Item = AssetRecord> + Send>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT hash, object_key, bucket, namespace, first_seen_at, last_seen_at, state,
                        pending_delete_at, deleted_at, last_marked_run_id, mark_count, size_bytes,
                        content_type, is_admin_pinned, pin_reason, updated_at
                 FROM gc_assets
                 WHERE state != 'Deleted'
                   AND (last_marked_run_id IS NULL OR last_marked_run_id != ?1)",
            )
            .map_err(|e| GcError::Backend(e.to_string()))?;
        let rows = stmt
            .query_map(params![run_id], |r| {
                Ok(Self::row_to_record(
                    r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?,
                    r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?, r.get(11)?, r.get(12)?, r.get(13)?,
                    r.get(14)?, r.get(15)?,
                ))
            })
            .map_err(|e| GcError::Backend(e.to_string()))?;
        let out: Vec<AssetRecord> = rows.flatten().collect();
        Ok(Box::new(out.into_iter()))
    }

    fn iter_pending_older_than(
        &self,
        cutoff: SystemTime,
    ) -> GcResult<Box<dyn Iterator<Item = AssetRecord> + Send>> {
        let cutoff_ms = to_unix_millis(cutoff);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT hash, object_key, bucket, namespace, first_seen_at, last_seen_at, state,
                        pending_delete_at, deleted_at, last_marked_run_id, mark_count, size_bytes,
                        content_type, is_admin_pinned, pin_reason, updated_at
                 FROM gc_assets
                 WHERE state = 'PendingDelete' AND pending_delete_at IS NOT NULL AND pending_delete_at <= ?1",
            )
            .map_err(|e| GcError::Backend(e.to_string()))?;
        let rows = stmt
            .query_map(params![cutoff_ms], |r| {
                Ok(Self::row_to_record(
                    r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?,
                    r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?, r.get(11)?, r.get(12)?, r.get(13)?,
                    r.get(14)?, r.get(15)?,
                ))
            })
            .map_err(|e| GcError::Backend(e.to_string()))?;
        let out: Vec<AssetRecord> = rows.flatten().collect();
        Ok(Box::new(out.into_iter()))
    }

    fn set_pending_delete(&self, hash: &str, at: SystemTime) -> GcResult<()> {
        let at_ms = to_unix_millis(at);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE gc_assets SET state = 'PendingDelete', pending_delete_at = ?1, updated_at = ?1 WHERE hash = ?2",
            params![at_ms, hash],
        )
        .map_err(|e| GcError::Backend(e.to_string()))?;
        Ok(())
    }

    fn set_deleted(&self, hash: &str, at: SystemTime) -> GcResult<()> {
        let at_ms = to_unix_millis(at);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE gc_assets SET state = 'Deleted', deleted_at = ?1, updated_at = ?1 WHERE hash = ?2",
            params![at_ms, hash],
        )
        .map_err(|e| GcError::Backend(e.to_string()))?;
        Ok(())
    }

    fn clear_pending_delete(&self, hash: &str) -> GcResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE gc_assets SET pending_delete_at = NULL, state = 'Active' WHERE hash = ?1",
            params![hash],
        )
        .map_err(|e| GcError::Backend(e.to_string()))?;
        Ok(())
    }
}

impl GcRunStore for SqliteGcStore {
    fn start_run(&self, start: GcRunStart) -> GcResult<String> {
        let run_id = generate_run_id();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO gc_runs (run_id, mode, started_at) VALUES (?1, ?2, ?3)",
            params![run_id, mode_to_str(start.mode), to_unix_millis(start.started_at)],
        )
        .map_err(|e| GcError::Backend(e.to_string()))?;
        Ok(run_id)
    }

    fn apply_delta(&self, run_id: &str, delta: GcRunDelta) -> GcResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE gc_runs SET
               marked_count = marked_count + ?1,
               newly_pending_count = newly_pending_count + ?2,
               hard_deleted_count = hard_deleted_count + ?3,
               skipped_pinned_count = skipped_pinned_count + ?4,
               error_count = error_count + ?5
             WHERE run_id = ?6",
            params![
                delta.marked_count as i64,
                delta.newly_pending_count as i64,
                delta.hard_deleted_count as i64,
                delta.skipped_pinned_count as i64,
                delta.error_count as i64,
                run_id,
            ],
        )
        .map_err(|e| GcError::Backend(e.to_string()))?;
        Ok(())
    }

    fn finish_run(&self, run_id: &str, finish: GcRunFinish) -> GcResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE gc_runs SET finished_at = ?1, status = ?2, notes = ?3 WHERE run_id = ?4",
            params![
                to_unix_millis(finish.finished_at),
                status_to_str(finish.status),
                finish.notes,
                run_id,
            ],
        )
        .map_err(|e| GcError::Backend(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("nodalmerge-gcstore-test-{nanos}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn mark_then_soft_then_hard_lifecycle() {
        let dir = tmpdir();
        let store = SqliteGcStore::open(&dir, local_key_scheme()).unwrap();
        let now = SystemTime::now();

        let run1 = store.start_run(GcRunStart { mode: GcRunMode::MarkOnly, started_at: now }).unwrap();
        store.upsert_active_seen(&run1, "h1", now).unwrap();
        assert_eq!(store.asset_state("h1"), Some(AssetState::Active));

        // Soft sweep for a run that does NOT mark h1 → should show up as an
        // unmarked candidate.
        let run2 = store.start_run(GcRunStart { mode: GcRunMode::SweepSoft, started_at: now }).unwrap();
        let candidates: Vec<_> = store.iter_unmarked_candidates(&run2).unwrap().collect();
        assert!(candidates.iter().any(|r| r.hash == "h1"));
        store.set_pending_delete("h1", now).unwrap();
        assert_eq!(store.asset_state("h1"), Some(AssetState::PendingDelete));

        let past_grace = now + Duration::from_secs(3600);
        let pending: Vec<_> = store.iter_pending_older_than(past_grace).unwrap().collect();
        assert!(pending.iter().any(|r| r.hash == "h1"));
        store.set_deleted("h1", past_grace).unwrap();
        assert_eq!(store.asset_state("h1"), Some(AssetState::Deleted));
    }

    #[test]
    fn restart_durability_survives_reopen() {
        let dir = tmpdir();
        let now = SystemTime::now();
        {
            let store = SqliteGcStore::open(&dir, local_key_scheme()).unwrap();
            let run1 = store.start_run(GcRunStart { mode: GcRunMode::MarkOnly, started_at: now }).unwrap();
            store.upsert_active_seen(&run1, "h-restart", now).unwrap();
            store.set_pending_delete("h-restart", now).unwrap();
        }
        // Reopen a fresh store instance against the same directory.
        let store2 = SqliteGcStore::open(&dir, local_key_scheme()).unwrap();
        assert_eq!(store2.asset_state("h-restart"), Some(AssetState::PendingDelete));
        let pending: Vec<_> = store2
            .iter_pending_older_than(now + Duration::from_secs(1))
            .unwrap()
            .collect();
        assert!(pending.iter().any(|r| r.hash == "h-restart"));
    }

    /// Slice 6.3 — the batch override must leave rows byte-for-byte
    /// equivalent to the per-hash path (same statement, same bindings), for
    /// both the insert arm and the on-conflict re-mark arm. Compared via
    /// `iter_unmarked_candidates` with a sentinel (full `AssetRecord`s),
    /// not just `asset_state`.
    #[test]
    fn batch_upsert_rows_match_per_hash_upsert_rows() {
        let now = SystemTime::now();
        let hashes = ["h-a", "h-b", "h-c"];

        let dir_loop = tmpdir();
        let store_loop = SqliteGcStore::open(&dir_loop, local_key_scheme()).unwrap();
        let dir_batch = tmpdir();
        let store_batch = SqliteGcStore::open(&dir_batch, local_key_scheme()).unwrap();

        // Insert arm, then a second run re-marks (conflict arm).
        for run in ["run-1", "run-2"] {
            for h in &hashes {
                store_loop.upsert_active_seen(run, h, now).unwrap();
            }
            store_batch.upsert_active_seen_batch(run, &hashes, now).unwrap();
        }

        let recs = |s: &SqliteGcStore| -> Vec<AssetRecord> {
            let mut v: Vec<AssetRecord> =
                s.iter_unmarked_candidates("_no-such-run").unwrap().collect();
            v.sort_by(|a, b| a.hash.cmp(&b.hash));
            v
        };
        let (a, b) = (recs(&store_loop), recs(&store_batch));
        assert_eq!(a.len(), 3);
        for (l, r) in a.iter().zip(b.iter()) {
            assert_eq!(l.hash, r.hash);
            assert_eq!(l.object_key, r.object_key);
            assert_eq!(l.bucket, r.bucket);
            assert_eq!(l.state, r.state);
            assert_eq!(l.last_marked_run_id, r.last_marked_run_id);
            assert_eq!(l.mark_count, r.mark_count, "conflict arm must bump mark_count identically");
            assert_eq!(l.last_seen_at, r.last_seen_at);
            assert_eq!(l.first_seen_at, r.first_seen_at);
        }
    }

    #[test]
    fn run_ledger_records_failed_status() {
        let dir = tmpdir();
        let store = SqliteGcStore::open(&dir, local_key_scheme()).unwrap();
        let now = SystemTime::now();
        let run_id = store.start_run(GcRunStart { mode: GcRunMode::MarkOnly, started_at: now }).unwrap();
        store
            .finish_run(&run_id, GcRunFinish { status: GcRunStatus::Failed, finished_at: now, notes: Some("boom".into()) })
            .unwrap();
        assert_eq!(store.run_status(&run_id), Some("Failed".to_string()));
    }
}
