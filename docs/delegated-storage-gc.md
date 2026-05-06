# Delegated Storage GC Interfaces (storage-agnostic)

This spec defines the minimum contracts for safe CAS garbage collection
across ActiveSync deployments without coupling core GC logic to any specific
state database (Mongo/Postgres/SQLite/KV).

Use this when you run ActiveSync with delegated blob storage (S3/R2/MinIO)
and need deterministic reclaim of unreferenced blob objects.

## Goals

1. Keep GC logic backend-agnostic.
2. Make reachability authoritative from ActiveSync room state.
3. Avoid daily dependence on ListBucket.
4. Keep app/product field naming out of shared GC core.

## Architecture

1. `LiveHashSource`: authoritative mark input from ActiveSync state.
2. `AssetInventoryStore`: persistent inventory of known blob objects.
3. `GcRunStore`: run ledger and counters for audit/rerun safety.
4. `BlobObjectStore`: object HEAD/DELETE (LIST optional, low-frequency drift job only).
5. `GcCoordinator`: orchestrates mark -> soft sweep -> hard sweep.

## Rust trait contracts

```rust
use std::collections::HashSet;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetState {
    Active,
    PendingDelete,
    Deleted,
    Quarantined,
}

#[derive(Debug, Clone)]
pub struct AssetRecord {
    pub hash: String,
    pub object_key: String,
    pub bucket: String,
    pub namespace: String,
    pub first_seen_at: SystemTime,
    pub last_seen_at: SystemTime,
    pub state: AssetState,
    pub pending_delete_at: Option<SystemTime>,
    pub deleted_at: Option<SystemTime>,
    pub last_marked_run_id: Option<String>,
    pub mark_count: u64,
    pub size_bytes: Option<u64>,
    pub content_type: Option<String>,
    pub is_admin_pinned: bool,
    pub pin_reason: Option<String>,
    pub updated_at: SystemTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcRunMode {
    DryRun,
    MarkOnly,
    SweepSoft,
    SweepHard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcRunStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone)]
pub struct GcRunStart {
    pub mode: GcRunMode,
    pub started_at: SystemTime,
}

#[derive(Debug, Default, Clone)]
pub struct GcRunDelta {
    pub marked_count: u64,
    pub newly_pending_count: u64,
    pub hard_deleted_count: u64,
    pub skipped_pinned_count: u64,
    pub error_count: u64,
}

#[derive(Debug, Clone)]
pub struct GcRunFinish {
    pub status: GcRunStatus,
    pub finished_at: SystemTime,
    pub notes: Option<String>,
}

pub trait LiveHashSource: Send + Sync {
    /// Return all live blob hashes reachable from authoritative room state.
    fn collect_live_hashes(&self) -> anyhow::Result<HashSet<String>>;
}

pub trait AssetInventoryStore: Send + Sync {
    fn upsert_active_seen(
        &self,
        run_id: &str,
        hash: &str,
        now: SystemTime,
    ) -> anyhow::Result<()>;

    fn iter_unmarked_candidates(
        &self,
        run_id: &str,
    ) -> anyhow::Result<Box<dyn Iterator<Item = AssetRecord> + Send>>;

    fn iter_pending_older_than(
        &self,
        cutoff: SystemTime,
    ) -> anyhow::Result<Box<dyn Iterator<Item = AssetRecord> + Send>>;

    fn set_pending_delete(
        &self,
        hash: &str,
        at: SystemTime,
    ) -> anyhow::Result<()>;

    fn set_deleted(
        &self,
        hash: &str,
        at: SystemTime,
    ) -> anyhow::Result<()>;

    fn clear_pending_delete(&self, hash: &str) -> anyhow::Result<()>;
}

pub trait GcRunStore: Send + Sync {
    fn start_run(&self, start: GcRunStart) -> anyhow::Result<String>;
    fn apply_delta(&self, run_id: &str, delta: GcRunDelta) -> anyhow::Result<()>;
    fn finish_run(&self, run_id: &str, finish: GcRunFinish) -> anyhow::Result<()>;
}

pub trait AdminPinStore: Send + Sync {
    fn is_pinned(&self, hash: &str) -> anyhow::Result<bool>;
}

pub trait BlobObjectStore: Send + Sync {
    fn head(&self, bucket: &str, key: &str) -> anyhow::Result<bool>;
    fn delete(&self, bucket: &str, key: &str) -> anyhow::Result<()>;
}
```

Notes:

1. Adapters may be implemented with SQL, document, KV, or embedded stores.
2. `LiveHashSource` must be product-configurable; shared GC code must not hard-code product field names.
3. `BlobObjectStore` intentionally excludes `list` for daily GC safety.

## HTTP fallback contract

Use this when GC scheduler and ActiveSync state access are split across services.

### Endpoint

`POST /internal/gc/live-hashes`

### Request

```json
{
  "run_id": "2026-05-05T02:00:00Z-8d2d",
  "selectors": {
    "rooms": ["account/*", "board/*"],
    "namespaces": ["meta", "buttons"],
    "fields": ["iconHash", "imageHash", "soundHash"]
  }
}
```

### Response

```json
{
  "hashes": [
    "blake3hex...",
    "blake3hex..."
  ],
  "source": {
    "rooms_scanned": 124,
    "namespaces_scanned": 2,
    "generated_at_unix": 1770000000
  }
}
```

### Error response

```json
{
  "error": "reachability scan failed",
  "retryable": true
}
```

Rules:

1. Endpoint is internal-only (private network / mTLS / shared secret).
2. Return deduplicated lowercase hex hashes.
3. On partial scan failure, return error and fail closed (no hard delete run).

## Mark and sweep semantics

### Mark

1. Start run in `GcRunStore`.
2. Collect live hashes from `LiveHashSource` or HTTP fallback.
3. For each hash: upsert inventory as `Active`, set `last_marked_run_id = run_id`, clear pending delete.

### Soft sweep

1. Query inventory where not marked in current run and not pinned.
2. Transition `Active -> PendingDelete` with `pending_delete_at = now`.
3. Leave already pending rows untouched.

### Hard sweep

1. Query `PendingDelete` older than grace window and not pinned.
2. Optional `HEAD` safety check.
3. `DELETE` object and transition inventory row to `Deleted`.

## Safety defaults

1. `grace_window`: 24h production, 1h staging.
2. `max_deletes_per_run`: start small (for example 100).
3. `require_head_before_delete`: enabled during rollout.
4. Run modes behind feature flags: `DryRun`, `MarkOnly`, `SweepSoft`, `SweepHard`.

## Portability checklist

1. No Mongo-only assumptions in coordinator code.
2. No SpeechSlate-specific field names in shared crates.
3. Do not require object listing permission for nightly GC.
4. Keep policy hooks (admin pinning, room scope) injectable per product.
