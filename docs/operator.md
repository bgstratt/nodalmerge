# Operator runbook

Steady-state operation of a deployed `nodalmerge-server`. Covers every
CLI flag, every metric, backup/restore for each supported persistence
backend, and the rolling-restart procedure.

> First-time readers: see [quickstart.md](./quickstart.md) and
> [self-host.md](./self-host.md). For integration shapes, see
> [integration.md](./integration.md). For version upgrades, see
> [migration.md](./migration.md). For staged rollout templates, see
> [MIGRATION_COOKBOOK.md](./MIGRATION_COOKBOOK.md) and
> [MIGRATION_ANTI_PATTERNS_CHECKLIST.md](./MIGRATION_ANTI_PATTERNS_CHECKLIST.md). For cross-surface operation inventory and
> gap analysis, see [operations-inventory.md](./operations-inventory.md).

---

## Config reference

All configuration is via CLI flags on `nodalmerge-server`. There is no
config file. Flags accept both `--flag value` and `--flag=value` form.

| Flag | Default | Meaning |
|---|---|---|
| `--store <path>` | *(in-memory)* | Durable persistence root. Creates `<path>/nodalmerge.db` + `<path>/blobs/`. See [deployment.md](./deployment.md). |
| `--metrics-addr <ip:port>` | *(off)* | Admin HTTP listener for Prometheus scrape. Bind to loopback or a private VPC subnet — never the public WS port. |
| `--idle-timeout <secs>` | `300` | Evict rooms with zero connected peers after N seconds. `0` disables. Gated on durable persistence; warns + skips without `--store`. |
| `--broadcast-capacity <N>` | `512` | Per-room `broadcast::channel` ring size (G1). Larger = more slack for brief client stalls; smaller = faster divergence detection. `0` rejected. |
| `--peer-rate-nodes <N>` | `200` | Per-peer ceiling in nodes per second (G3). `0` disables. |
| `--peer-rate-bytes <MiB>` | `4` | Per-peer ceiling in decoded-pack bytes per second (G3). `0` disables. |
| `--blob-gc-interval <secs>` | `0` | Periodic blob GC sweep interval (G4). `0` disables. Durable-only. |
| `--blob-gc-grace <secs>` | `86400` | Tombstone grace before an orphan blob is deleted. |

**Environment.**

- `RUST_LOG` — tracing filter (for example
  `info,nodalmerge_server=info,nodalmerge_core=info`).
- Server keypair: auto-generated at `server.key` in the server's working directory on
  first run; reused across restarts.

---

## Metrics reference

Install with `--metrics-addr 127.0.0.1:9090`; scrape at
`http://127.0.0.1:9090/metrics`. Cardinality is bounded: `room` label
is used only where it's useful; `peer` is a 12-char pubkey prefix so
churn can't explode the series count.

| Metric | Kind | Labels | Alert shape |
|---|---|---|---|
| `nodalmerge_rooms_total` | gauge | — | capacity trend, not alerting |
| `nodalmerge_peers_total` | gauge | `room` | capacity; sudden drop = mass disconnect |
| `nodalmerge_nodes_accepted_total` | counter | `room` | rate trend; low-signal alone |
| `nodalmerge_merge_batch_seconds` | histogram | — | `p99 > 100 ms` for 5 min → verify CPU / ed25519 batch size |
| `nodalmerge_persistence_write_seconds` | histogram | `kind` (`node`/`nodes_batch`/`blob`) | `p99 > 50 ms` sustained → disk saturation, DB slowdown |
| `nodalmerge_eviction_total` | counter | — | trend; high rate in production = peer churn |
| `nodalmerge_broadcast_lagged_total` | counter | `room` | `rate > 0` = slow clients; chronic = bump `--broadcast-capacity` or investigate peer |
| `nodalmerge_ws_send_timeout_total` | counter | `room` | any non-zero = TCP or client stalled; investigate network |
| `nodalmerge_rate_limit_drops_total` | counter | `peer` | any non-zero = misbehaving (or misconfigured) client |
| `nodalmerge_blob_gc_deleted_total` | counter | — (global pool) | steady rate confirms GC is running |
| `nodalmerge_lamport_rejected_total` | counter | `reason` (`ceiling` / `wall_skew`) | any non-zero = client clock broken or malicious |
| `nodalmerge_token_expired_disconnects_total` | counter | `room` | trend; rate should correlate with token TTL |
| `nodalmerge_topology_promotion_total` | counter | `stage`, `outcome`, `reason` (rejects only) | rejection rate by `reason`; see [Topology promotion metrics](#topology-promotion-metrics-wave-3) |
| `nodalmerge_topology_promotion_seconds` | histogram | `stage`, `outcome` | handler latency; `p95` per stage when tuning promotion workflows |
| `nodalmerge_room_bytes_resident` | gauge | `room` | capacity; approximate — per-node estimate is flat 512 B, under-counts large transactions |

Histogram buckets are hand-tuned for the hot path:

- `nodalmerge_merge_batch_seconds`: 50µs … 2.5s (covers single-node
  packs through 10k-node catchup).
- `nodalmerge_persistence_write_seconds`: 100µs … 500ms (typical node
  INSERT is <1 ms; >100 ms is an alerting signal).

**Admission.** `metrics::init` installs a *process-global* recorder; a
second install fails. Install failure is logged and the server
continues without observability.

### Topology promotion metrics (Wave 3)

Emitted by `nodalmerge-server` on `topology.propose-promotion`,
`topology.validate-promotion`, and `topology.apply-promotion` (requires
`topology.admin`). These are **scrape-only** Prometheus series on
`/metrics` — there is no separate topology metrics HTTP API. Dashboard
panels and alert thresholds are **not** defined in-repo yet; use the
names and example queries below when you wire your own observability
stack.

| Series | Labels | Meaning |
|---|---|---|
| `nodalmerge_topology_promotion_total` | `stage` (`propose` \| `validate` \| `apply`), `outcome` (`ok` \| `rejected`) | One increment per handler completion. |
| `nodalmerge_topology_promotion_total` | `reason` (rejects only) | Stable wire value from `PromotionReasonClass`, e.g. `reject.promotion_stale_parent`, `reject.promotion_not_found`. |
| `nodalmerge_topology_promotion_seconds` | `stage`, `outcome` | Wall time inside the promotion handler (seconds). |

Example PromQL (adjust job/label selectors to your scrape config):

```promql
# Successful applies per minute
sum(rate(nodalmerge_topology_promotion_total{stage="apply",outcome="ok"}[5m])) * 60

# Rejection rate by reason (all stages)
sum by (reason) (rate(nodalmerge_topology_promotion_total{outcome="rejected"}[5m]))

# p95 apply latency (seconds)
histogram_quantile(0.95, sum by (le) (rate(nodalmerge_topology_promotion_seconds_bucket{stage="apply"}[5m])))
```

Evidence: `docs/acceptance/authority-topology-wave3-promotion-metrics-run01.json`.

---

## Archive alert policy wiring (Phase D)

This section wires the Phase D threshold policy into operational ownership
and dashboards. Threshold source-of-truth is
`docs-site/benchmarks/runtime-attribution.mdx`, with accepted baseline
evidence in `docs/acceptance/archive-phased-alert-thresholds-run01.json`
and `docs/acceptance/archive-phased-benchmark-baseline-run04.json`.

Published templates:

1. Dashboard annotation template: `docs/ALERT_DASHBOARD_ANNOTATION_TEMPLATE.md`
2. Incident ticket template: `docs/INCIDENT_TICKET_TEMPLATE_ARCHIVE_ALERT.md`

Published dry-run examples:

1. Dashboard annotation dry-run: `docs/acceptance/archive-alert-dashboard-annotation-dryrun-run01.txt`
2. Incident ticket dry-run: `docs/acceptance/archive-alert-incident-ticket-dryrun-run01.md`
3. Timing evidence bundle: `docs/acceptance/archive-phased-alert-dryrun-run01.json`
4. Dashboard annotation dry-run (critical route): `docs/acceptance/archive-alert-dashboard-annotation-dryrun-run02.txt`
5. Incident ticket dry-run (critical route): `docs/acceptance/archive-alert-incident-ticket-dryrun-run02.md`
6. Timing evidence bundle (critical route): `docs/acceptance/archive-phased-alert-dryrun-run02.json`

### Dashboard panels (required)

Create a dashboard folder named "Archive portability" with these panels:

1. Manifest cache miss ratio (15m):
  - Query inputs: `nodalmerge_archive_manifest_cache_lookup_total{outcome="hit"}` and `nodalmerge_archive_manifest_cache_lookup_total{outcome="miss"}`
  - Plot: `miss / (hit + miss)` with sample guard annotation (`hit + miss >= 200`)
  - Threshold overlays: warn 0.10, critical 0.25
2. Manifest cache miss ratio (5m fast-burn):
  - Same ratio with 5-minute window and sample guard (`hit + miss >= 100`)
  - Threshold overlay: critical 0.40
3. Object/file parity drift p95 (export/validate/import):
  - Plot absolute p95 delta between object lane and file lane
  - Threshold overlays: warn (1/1/2 ms by operation), critical 5 ms
4. Runtime p95 safety rail:
  - Plot p95 `archive.export`, `archive.validate`, and `archive.import`
  - Threshold overlays: warn 50 ms, critical 100 ms

### On-call ownership

Primary owner:

1. Runtime on-call (L1) monitors all four panels during business hours and pager windows.

Secondary owner:

1. Platform performance owner (L2) handles sustained threshold breaches and threshold tuning requests.

Escalation owner:

1. Runtime tech lead (L3) approves rollback, threshold override, or release hold decisions.

### Escalation flow

When a threshold triggers, use this flow:

1. L1 acknowledges within 10 minutes and captures panel screenshots + current room scope.
2. L1 validates sample guard before escalation:
  - miss ratio alerts require sample minimum in-window
  - parity alerts require both file/object lanes emitting
3. If warn persists for 15 minutes, page L2 and open incident ticket with runbook tag `archive-alert-policy`.
4. If any critical threshold fires or warn exceeds 30 minutes, page L3 and freeze archive-related rollout changes.
5. Recovery exit criteria:
  - all metrics return below warn thresholds for 30 consecutive minutes
  - incident ticket includes root-cause note and follow-up owner/date

### Immediate triage playbook

1. Cache miss ratio high:
  - verify manifest path churn and revision instability
  - inspect object root or file staging pipeline for frequent rewrites
2. Parity drift high:
  - compare object store latency/error rate with file lane
  - verify object manifest resolution path and storage gateway health
3. Absolute p95 high:
  - inspect CPU and disk saturation first, then signature/manifest path regressions

### Weekly review cadence

1. Review previous 7 days of threshold crossings in ops standup.
2. Recalibrate only if two consecutive benchmark runs show >20% stable p95 shift.
3. Record every threshold change in acceptance artifacts before deployment.

---

## Backup + restore

### `DirPersistence` (SQLite + files)

Layout (see [deployment.md](./deployment.md) and
[BLOB_STORAGE_LAYOUT.md](./BLOB_STORAGE_LAYOUT.md) for full detail):

```
<path>/
  nodalmerge.db              SQLite (legacy file name retained during migration)
  blobs/blake3/<hex>         global CAS pool, no room segment
  blobs/.tombstones/blake3/<hex>
```

- **Hot backup:** `sqlite3 nodalmerge.db ".backup '/dest/nodalmerge.db'"`
  + a recursive copy of `blobs/`. SQLite is WAL, so the `.backup`
  command is consistent.
- **Cold backup:** stop the server, copy `nodalmerge.db`,
  `nodalmerge.db-wal`, `nodalmerge.db-shm`, and `blobs/`.
- **Restore:** point a new `--store <path>` at the copy.
- **Consistency guarantee:** nodes and blobs are content-addressed;
  a partially restored `blobs/` only loses the blobs whose files are
  missing — the DAG re-references them safely and peers will
  re-upload on demand.
- **Upgrading a 0.1.x store:** the legacy per-room layout
  (`blobs/<sanitized_room>/<hex>`) auto-migrates on first open after
  upgrade; no operator action needed for file stores. Direct-S3 stores
  migrate manually — see BLOB_STORAGE_LAYOUT.md §6.

### `PostgresNodeStore`

- **Backup:** whatever your existing Postgres backup regime is (WAL
  archiving, logical dumps, managed provider snapshots). Tables to
  include: `nodalmerge_nodes` + the migration table sqlx generates.
- **Restore:** restore the DB; point
  `PostgresNodeStore::connect_and_migrate(cfg)` at it. Migrations
  are idempotent.
- **Consistency guarantee:** `UNIQUE(room_id, node_id)` with
  `INSERT ... ON CONFLICT ... DO NOTHING` means re-plays are safe.

### `MongoNodeStore`

- **Backup:** `mongodump` or your managed provider's snapshot.
  Include the `accepted_nodes` collection.
- **Restore:** `mongorestore`. The `(room_id, seq)` compound index
  and compound `_id` are recreated on first connect.
- **Consistency guarantee:** compound `_id = "<room>:<node_hex>"`
  makes re-inserts idempotent (duplicate-key on `E11000` is treated
  as success).

### `S3BlobStore`

- **Backup:** bucket-level versioning + cross-region replication in
  your S3 provider. Keys are `<path_prefix>blake3/<hex>` — the prefix is
  configurable per deployment; the `blake3/` segment is not (see
  BLOB_STORAGE_LAYOUT.md §5).
- **Restore:** no sync-server action needed. `S3BlobStore` looks up
  blobs on demand via `resolve_get_url`; missing blobs re-upload
  naturally on next `setBlob`.
- **Consistency guarantee:** blobs are content-addressed Blake3. A
  bucket with a subset of blobs is always safe; missing objects
  simply produce fresh uploads.

---

## Capacity planning

Rules of thumb, derived from the bench suite (`cargo bench -p
nodalmerge-core`, reference machine = Ryzen 9 5900X, 12 cores):

- **Merge throughput.** ~31 ms for a 10k-node batched pack
  (`merge_10k_batch`). Bottleneck is ed25519 `verify_batch`; ~95% of
  wall time.
- **Node bytes.** `nodalmerge_room_bytes_resident` assumes 512 bytes
  per node; real numbers on the wire are 100-300 B for small ops,
  much more for large `SetBlob` payloads (payload ≠ blob bytes).
- **Blob bytes.** No theoretical limit per room; budget per product.
  G4's GC keeps orphan blob accumulation bounded.
- **Peers per replica.** One `tokio::task` per peer WS; one broadcast
  receiver per peer per room. CPU bound by merge path; network bound
  by broadcast fan-out. No hard cap — size based on `p99` of
  `nodalmerge_merge_batch_seconds`.
- **Handshake size.** IBF hello is 2.9 KB regardless of graph size;
  MST handshake ≤3 round trips for a 1000-node diff in a 2000-node
  graph.

**When to shard rooms across replicas.** Once `nodalmerge_peers_total`
for a single room crosses 100 or `p99` of
`nodalmerge_merge_batch_seconds` exceeds 100 ms. Shard by room id
with consistent hashing at the load balancer; rooms are independent.

---

## Upgrading sync-server

NodalMerge's `A7` capability negotiation handles the mixed-version
window automatically. A rolling restart is:

1. Pick a rollout cohort (one replica at a time, or a quartile).
2. Drain: stop accepting new room-assignment traffic at the load
   balancer. Existing sessions close with `1012 service restart`
   (tokio runtime drop handles this).
3. Restart with the new binary.
4. SDK clients reconnect via their existing exp-backoff loop; the
   new server's handshake negotiates capabilities against whatever
   SDK version the client has.
5. Repeat for the remaining cohorts.

**Room-to-server affinity.** If you run >1 replica with the same
persistence backend, use consistent hashing on `room_id` at the load
balancer so a given room consistently lands on the same replica.
Without affinity, two replicas concurrently writing to the same room
is safe (idempotent inserts) but wastes the in-memory cache on every
bounce.

**Cross-region replication** is the database's problem. Neither
`MongoNodeStore` nor `PostgresNodeStore` nor `S3BlobStore`
coordinates sync-server-to-sync-server; rely on the DB's own
replica-set / logical-replication / cross-region-replication.

### Compatibility-window rollout policy (FSE-06 baseline)

Follow `docs/MIGRATION_COOKBOOK.md` for forward-only, dual-read, and rollback-safe patterns.

Operational policy:

1. do not remove old reader/writer paths until convergence evidence is attached
2. treat unsupported-window rejects (for example `reject.query_unsupported_version`) as rollout blockers, not transient retries
3. if unsupported-window rejects spike:
   - pause cutover
   - restore dual-read/dual-write posture
   - resume only after reject rate returns to baseline

---

## When things look wrong

| Symptom | First check |
|---|---|
| Mass disconnects with `4001 resync required` | `nodalmerge_broadcast_lagged_total` per room; investigate one slow peer, or bump `--broadcast-capacity`. |
| Mass disconnects with `4008 rate limit exceeded` | Per-peer `nodalmerge_rate_limit_drops_total`; investigate the offending pubkey. |
| Disconnects with `4002 token expired` | Normal if token TTL is short; confirm SDK is refetching tokens on reconnect. |
| `nodalmerge_lamport_rejected_total` climbing | Client clock skew or malicious peer. Cross-check with server logs (peer pubkey prefix is logged). |
| `nodalmerge_merge_batch_seconds` p99 rising | CPU saturation; scale horizontally (shard by room) or verify ed25519 simd backend is active. |
| `nodalmerge_persistence_write_seconds{kind="blob"}` p99 rising | Disk I/O or S3 latency; check the blob backend. |
| Disk fills | `nodalmerge_blob_gc_deleted_total` not ticking; verify `--blob-gc-interval` is set. |

### Presence continuity troubleshooting (FSE-05 phase A)

Expected semantics:

1. `presence` messages are ephemeral and session-scoped.
2. Presence leave causes are stable:
   - `leave`: websocket/session closed.
   - `stale`: TTL lease expired and sweep removed the entry.
3. Peer lifecycle broadcasts are pubkey-scoped (not socket-scoped):
   - one `peer-joined` on first active session for a pubkey,
   - one `peer-left` on last active session for that pubkey.

Triage checks:

1. Duplicate join/leave flicker for one user:
   - verify client is not rotating pubkeys across reconnect attempts,
   - verify reconnect overlap does not exceed expected brief window.
2. Presence entries never aging out:
   - verify sender includes TTL/clock fields where lease semantics are expected,
   - verify sweep cadence (runtime path that triggers stale cleanup) is active.
3. Unexpected stale removals:
   - check client clock drift vs server time and configured TTL budget,
   - confirm heartbeat/update interval is below TTL with margin.

Malformed lease diagnostics (stable rejects):

1. `reject.presence_lease_invalid:ttl_ms_requires_now_unix_ms`
2. `reject.presence_lease_invalid:now_unix_ms_requires_ttl_ms`
3. `reject.presence_lease_invalid:ttl_ms_must_be_positive_u64`
4. `reject.presence_lease_invalid:now_unix_ms_must_be_u64`

---

## Admin operation reference

This section summarizes room/admin protocol operations used in production
operations and debugging workflows.

### `server-info`

Purpose:

1. Confirm server capabilities/version shape during incident triage.

Request:

```json
{ "type": "server-info" }
```

Expected response:

```json
{
  "type": "server-info",
  "version": "<build>",
  "caps": { "supports_ibf": true }
}
```

### `set-room-key`

Purpose:

1. Enable room token enforcement by setting room verification key.

Request:

```json
{ "type": "set-room-key", "pubkey": "<ed25519_hex>" }
```

Expected response:

```json
{ "type": "room-locked" }
```

Operational notes:

1. Invalid key encoding returns `error`.
2. After lock, peers without valid room tokens are rejected.

### `set-policy`

Purpose:

1. Update room policy defaults and path rules.

Request:

```json
{
  "type": "set-policy",
  "default": "deny",
  "rules": [
    { "path_glob": "world/**", "allow": ["<peer_pubkey_hex>"] }
  ]
}
```

Expected response:

```json
{ "type": "policy-set" }
```

Operational notes:

1. Unknown defaults or malformed rules return `error`.
2. Treat policy updates as configuration events; log request/response with operator identity.

### `start-tick` and `stop-tick`

Purpose:

1. Control authoritative server-side tick loop for intent-to-world materialization.

Start request:

```json
{ "type": "start-tick", "interval_ms": 100, "intent_prefix": "intent/" }
```

Start response:

```json
{ "type": "tick-started", "interval_ms": 100, "intent_prefix": "intent/" }
```

If already running:

```json
{ "type": "tick-already-running", "interval_ms": 100, "intent_prefix": "intent/" }
```

Stop request/response:

```json
{ "type": "stop-tick" }
```

```json
{ "type": "tick-stopped" }
```

Operational notes:

1. Keep interval conservative in multi-tenant deployments to avoid bursty write amplification.
2. Prefer explicit change control for interval/prefix changes.

### `compact-room`

Purpose:

1. Trigger room compaction and snapshot pack emission.

Request:

```json
{ "type": "compact-room" }
```

Expected responses:

```json
{
  "type": "snapshot-pack",
  "snapshot": "<base64_snapshot>",
  "frontier": ["<node_id_hex>"]
}
```

then

```json
{ "type": "compact-ack" }
```

Operational notes:

1. Run during lower write pressure windows for large rooms.
2. Capture snapshot metadata in incident logs if compaction fails downstream.

### `error` handling guidance

Server error envelope:

```json
{ "type": "error", "msg": "reason" }
```

Operational handling:

1. Auth/policy format failures: fix request and retry.
2. Transient storage/runtime failures: retry with backoff and monitor error counters.
3. Repeated protocol errors from one peer: isolate client and inspect wire payloads.

---

## Query and projection operations

This section documents the canonical query/materialization operator flow for runtime websocket control-plane usage.

### `query.register`

Purpose:

1. Register a query specification version for later projection builds.

Request:

```json
{
  "type": "query.register",
  "query_spec_id": "q.rooms",
  "version": "v1",
  "descriptor": { "source": "rooms" }
}
```

Expected success response:

```json
{
  "type": "query.registered",
  "query_spec_id": "q.rooms",
  "version": "v1",
  "accepted": true
}
```

Expected rejection response:

```json
{
  "type": "query.register.rejected",
  "query_spec_id": "q.rooms",
  "version": "v2",
  "reason_class": "reject.query_unsupported_version",
  "reason_message": "unsupported"
}
```

Operational notes:

1. Treat `reason_class` as stable automation key; keep alert routing on class, not message text.
2. Version rejection means compatibility-window mismatch; retry with an allowed version instead of blind retries.

### `projection.build`

Purpose:

1. Materialize projection state for a query spec at a canonical checkpoint cut.

Request:

```json
{
  "type": "projection.build",
  "projection_id": "p.rooms",
  "query_spec_id": "q.rooms",
  "target_checkpoint": { "selector": "latest" }
}
```

Expected success response:

```json
{
  "type": "projection.build.completed",
  "projection_id": "p.rooms",
  "checkpoint": { "selector": "seq", "canonical_seq": 1, "canonical_hash": "<hex>" },
  "digest": "<digest>"
}
```

Expected rejection response:

```json
{
  "type": "projection.build.rejected",
  "projection_id": "p.rooms",
  "reason_class": "reject.checkpoint_selector_invalid",
  "reason_message": "selector hash requires canonical_hash in 64-char hex format"
}
```

Operational notes:

1. Distinguish `reject.checkpoint_selector_invalid` from `reject.checkpoint_not_found` during triage.
2. For replay mismatch incidents, capture `checkpoint` and `digest` from build/read responses in incident notes.
3. `reject.query_backpressure` indicates runtime guardrails rejected the build (row limit, saturated inflight with `NODALMERGE_QUERY_BUILD_MAX_QUEUE=0`, or full wait queue); tune `NODALMERGE_QUERY_BUILD_MAX_ROWS`, `NODALMERGE_QUERY_BUILD_MAX_INFLIGHT`, and `NODALMERGE_QUERY_BUILD_MAX_QUEUE` for the host if sustained.
4. Observe pressure and degradation through `nodalmerge_query_build_total{outcome,reason}`, `nodalmerge_query_build_seconds{outcome}`, `nodalmerge_query_build_inflight{room}`, `nodalmerge_query_build_queue_depth{room}`, `nodalmerge_query_build_queued_total`, and `nodalmerge_query_build_queue_wait_seconds`.

### `projection.read`

Purpose:

1. Read deterministic rows and digest for a built projection.

Request:

```json
{ "type": "projection.read", "projection_id": "p.rooms", "limit": 50, "page_token": "offset:0" }
```

Expected response:

```json
{
  "type": "projection.read.result",
  "projection_id": "p.rooms",
  "checkpoint": { "selector": "seq", "canonical_seq": 1, "canonical_hash": "<hex>" },
  "rows": [],
  "digest": "<digest>",
  "next_page_token": "offset:50"
}
```

Expected rejection response:

```json
{
  "type": "projection.read.rejected",
  "projection_id": "p.rooms",
  "reason_class": "reject.projection_not_found",
  "reason_message": "projection is not registered"
}
```

Operational notes:

1. Keep paging requests on returned `next_page_token` only; do not synthesize tokens externally.
2. Digest drift at the same checkpoint is a deterministic parity bug and should trigger escalation.
3. Treat `projection.read.rejected` as non-retriable until projection registration/build state is corrected.

### `projection.invalidate`

Purpose:

1. Explicitly mark a projection as invalidated with operator reason metadata.

Request:

```json
{ "type": "projection.invalidate", "projection_id": "p.rooms", "reason": "schema-change" }
```

Expected response:

```json
{
  "type": "projection.invalidated",
  "projection_id": "p.rooms",
  "reason": "schema-change",
  "invalidated_at_hlc": "<hlc>"
}
```

Expected rejection response:

```json
{
  "type": "projection.invalidate.rejected",
  "projection_id": "p.rooms",
  "reason_class": "reject.projection_immutable",
  "reason_message": "projection cannot be invalidated in current state"
}
```

Operational notes:

1. Use structured reason strings (`schema-change`, `policy-change`, `manual`) for low-cardinality telemetry.
2. Follow invalidation with rebuild+read and store new checkpoint/digest pair for audit traceability.
3. For `projection.invalidate.rejected`, stop retry loops and escalate to projection lifecycle state review.

### `projection.list`

Purpose:

1. Enumerate projections deterministically for a query spec and optional state filter.

Request:

```json
{ "type": "projection.list", "query_spec_id": "q.rooms", "state_filter": "active", "cursor": "offset:0" }
```

Expected response:

```json
{
  "type": "projection.list.result",
  "query_spec_id": "q.rooms",
  "items": [{ "projection_id": "p.rooms", "state": "active" }],
  "cursor": "offset:50"
}
```

Expected rejection response:

```json
{
  "type": "projection.list.rejected",
  "query_spec_id": "q.rooms",
  "reason_class": "reject.query_spec_not_found",
  "reason_message": "query spec is not registered"
}
```

Operational notes:

1. Reuse only server-returned `cursor` values; do not fabricate cursors.
2. Handle `projection.list.rejected` as deterministic input/state failure, not a transient transport failure.

### `replay.read-range`

Purpose:

1. Read deterministic replay event windows for a key prefix using lamport floor + cursor paging.

Request:

```json
{ "type": "replay.read-range", "key_prefix": "world/", "from_lamport": 0, "limit": 100, "cursor": "offset:0" }
```

Expected response:

```json
{
  "type": "replay.read-range.result",
  "key_prefix": "world/",
  "from_lamport": 0,
  "items": [{ "lamport": 1, "node_id": "<hex>", "touched_keys": ["world/a"] }],
  "next_cursor": "offset:100"
}
```

Expected rejection response:

```json
{
  "type": "replay.read-range.rejected",
  "reason_class": "reject.invalid_payload",
  "reason_message": "replay.read-range requires non-empty key_prefix"
}
```

Operational notes:

1. Treat `next_cursor` as opaque and reuse only server-issued cursor values.
2. Keep `limit` bounded for incident tooling to avoid oversized replay pages.
3. Route rejects by `reason_class`; fix payload/state first, then retry.

### Query/Projection failure triage checklist

1. Capture request envelope, response type, `reason_class`, and `reason_message`.
2. If available, capture `projection_id`, `query_spec_id`, checkpoint selector, and digest fields.
3. Classify failure bucket:
4. selector/compatibility class: fix payload and retry once.
5. not-found/lifecycle class: repair query spec or projection state before retry.
6. replay/digest mismatch class: escalate as deterministic parity incident.
7. For repeated rejects with identical payload, stop retries and open an incident with captured envelopes.

---

## Archive operations

This section documents archive control-plane requests used by operators during portability, audit, and restore workflows.

### `archive.describe`

Purpose:

1. Resolve archive metadata for `room://` or `file://` references before validate/import/export actions.

Request:

```json
{ "type": "archive.describe", "archive_ref": "room://parent-room" }
```

Expected response:

```json
{
  "type": "archive.describe.result",
  "archive_ref": "room://parent-room",
  "manifest_id": "m.<id>",
  "payload_digest": "sha256:<hex>"
}
```

### `archive.validate`

Purpose:

1. Run deterministic integrity/compatibility validation prior to import.

Request:

```json
{ "type": "archive.validate", "archive_ref": "room://parent-room", "mode": "full_integrity" }
```

Expected response:

```json
{
  "type": "archive.validate.result",
  "archive_ref": "room://parent-room",
  "accepted": true
}
```

Expected rejection:

```json
{
  "type": "archive.validate.rejected",
  "reason_class": "reject.archive_manifest_invalid",
  "reason_message": "..."
}
```

### `archive.export`

Purpose:

1. Export source room state to archive target.

Request:

```json
{
  "type": "archive.export",
  "source_room_id": "parent-room",
  "archive_ref": "file://C:/tmp/parent-room-001.nmarchive"
}
```

Expected response:

```json
{
  "type": "archive.export.completed",
  "archive_ref": "file://C:/tmp/parent-room-001.nmarchive",
  "checkpoint_hash": "<hex>"
}
```

### `archive.import`

Purpose:

1. Import archive content into the session room using deterministic checkpoint verification.

Request:

```json
{
  "type": "archive.import",
  "archive_ref": "file://C:/tmp/parent-room-001.nmarchive",
  "import_mode": "full_apply"
}
```

Expected response:

```json
{
  "type": "archive.import.completed",
  "archive_ref": "file://C:/tmp/parent-room-001.nmarchive",
  "checkpoint_hash": "<hex>",
  "imported_nodes": 42
}
```

Expected rejection:

```json
{
  "type": "archive.import.rejected",
  "reason_class": "reject.archive_import_digest_mismatch",
  "reason_message": "..."
}
```

Operational notes:

1. Use `reason_class` for alerts and runbook routing, not free-text messages.
2. Capture archive ref + checkpoint hash in incident notes for every failed validate/import.
3. Lock import/export actions behind change-control in production rooms.

---

## Topology operations

This section documents parent/child room and promotion flows for manager/worker topologies.

### `topology.create-child`

Purpose:

1. Create child room lineage record bound to parent checkpoint metadata.

Request:

```json
{
  "type": "topology.create-child",
  "parent_room_id": "parent-room",
  "child_room_id": "child-room-a",
  "purpose": "worker-task",
  "policy": "promotion-based",
  "parent_checkpoint": { "hash": "<hex>", "seq": 12 }
}
```

Expected response:

```json
{
  "type": "topology.create-child.completed",
  "parent_room_id": "parent-room",
  "child_room_id": "child-room-a"
}
```

### `topology.describe-lineage` and `topology.list-children`

Purpose:

1. Inspect lineage metadata for one room or list all children under a parent.

Requests:

```json
{ "type": "topology.describe-lineage", "room_id": "child-room-a" }
```

```json
{ "type": "topology.list-children", "parent_room_id": "parent-room" }
```

### Promotion flow

Purpose:

1. Deterministically promote a child checkpoint into parent canonical lane with explicit proposal/validation/apply steps.

Requests:

```json
{
  "type": "topology.propose-promotion",
  "parent_room_id": "parent-room",
  "child_room_id": "child-room-a",
  "child_checkpoint_hash": "<hex>",
  "payload_ref": "room://child-room-a"
}
```

```json
{ "type": "topology.validate-promotion", "proposal_id": "p.123" }
```

```json
{ "type": "topology.apply-promotion", "proposal_id": "p.123" }
```

Operational notes:

1. Promotion stage/outcome metrics are emitted as `nodalmerge_topology_promotion_total` and `nodalmerge_topology_promotion_seconds`.
2. For rejects, route by `reason` label and include proposal id in incident logs.
3. Large-family policy tuning uses lineage retention and queue controls (see topology execution plan and acceptance artifacts).
4. Under concurrent apply contention against the same parent checkpoint, expect a single winner and `reject.promotion_stale_parent` for losing applies.

---

## Headless peer worker (`nodalmerge-headless`)

Pod/workstation peer that syncs to a reflector over WebSocket and persists a **peer-local** log (separate from server `--store`). See `headless/README.md` and `docs/HEADLESS_RUNTIME_PERSISTENCE_EXECUTION_PLAN.md`.

### When to use

| Deployment | Peer-local backend | Server store |
|---|---|---|
| Kubernetes worker pod | `file` + mounted volume at `/data` | Reflector `--store` (room authority) |
| CI / dev smoke | `memory` | In-memory or `--store` reflector |
| Workstation tool | `file` under operator home | Remote reflector URL |

Never point peer-local `NODALMERGE_HEADLESS_DATA_DIR` at the server persistence root — layouts differ by design.

### Environment (required)

| Variable | Required | Meaning |
|---|---|---|
| `NODALMERGE_HEADLESS_SERVER_URL` | yes | `ws://host:port/ws/<room-id>` |
| `NODALMERGE_HEADLESS_ROOM` | yes | Room id (must match URL path) |
| `NODALMERGE_HEADLESS_BACKEND` | no | `memory` (default), `file`, or `composite` (cache + durable file); `registered:<name>` for registry pilots |
| `NODALMERGE_HEADLESS_DATA_DIR` | if `file` | Peer-local SQLite + blobs directory |
| `NODALMERGE_HEADLESS_RUN_SECS` | no | Catch-up window after hello (default `10`) |
| `NODALMERGE_HEADLESS_NEGOTIATE_IBF` | no | `0`/`false` disables IBF in hello |
| `NODALMERGE_HEADLESS_NEGOTIATE_MST` | no | `0`/`false` disables MST descent |
| `NODALMERGE_HEADLESS_REPORT_JSON` | no | Path for session report JSON (`-` = stdout) |
| `NODALMERGE_HEADLESS_METRICS_ADDR` | no | Optional Prometheus listener (e.g. `127.0.0.1:9191`) |

Container image: `docker build -f headless/Dockerfile -t nodalmerge-headless .`

### Session report JSON

Use `--report-json /path/report.json` (or env above) after each run for dashboards and acceptance baselines. Fields include `backend`, `durable`, sync counters (`packs_applied`, `mst_requests`), `canonical_hash_hex`, and `timings_ms` (`hydrate_ms`, `websocket_sync_ms`, `flush_ms`, `checkpoint_ms`, `total_ms`).

### Headless metrics endpoint

`nodalmerge-headless` now supports a dedicated Prometheus listener via `--metrics-addr <ip:port>` or `NODALMERGE_HEADLESS_METRICS_ADDR`.

Example:

```bash
NODALMERGE_HEADLESS_METRICS_ADDR=127.0.0.1:9191 nodalmerge-headless
curl http://127.0.0.1:9191/metrics
```

Session-level metrics include:

1. `nodalmerge_headless_sessions_total` (labels: `backend`, `durable`, `outcome`)
2. `nodalmerge_headless_packs_applied_total`
3. `nodalmerge_headless_mst_requests_total`
4. `nodalmerge_headless_mst_nodes_fetched_total`
5. `nodalmerge_headless_websocket_sync_seconds`
6. `nodalmerge_headless_session_total_seconds`

### Failure triage

| Symptom | Likely cause | Action |
|---|---|---|
| `handshake did not receive welcome` | Wrong URL/room, reflector down, token required on locked room | Verify WS path; issue `RoomToken` with sync caps (headless open rooms need no token) |
| `reject.` in stderr | Policy/capability rejection | Capture full WS line; compare with server logs |
| `websocket connect failed` | Network / TLS mismatch | Use `ws://` for dev; terminate TLS at ingress for prod |
| Restart with same `data_dir` but stale hash | Server gained new ops while worker was down | Re-run worker (HEADLESS-RUN-003 pattern); increase `RUN_SECS` if catch-up window too short |
| `timed out waiting for protocol message` | MST descent or slow catch-up | Increase `NODALMERGE_HEADLESS_RUN_SECS`; check server load |
| High `flush_ms` on file backend | Disk pressure | Move volume to faster storage; ensure exclusive mount |

### Peer-local durability reject classes (FSE-09.A hardening)

Runtime-local adapters expose stable classes for operator routing:

1. `reject.local_persist_unavailable` — backend unavailable/misconfigured (retry or fix mount/path/permissions)
2. `reject.local_persist_version_skew` — persisted schema newer/older than runtime supports (operator migration action)
3. `reject.local_persist_corruption` — persisted bytes/structure invalid (data-loss-risk escalation path)
4. `reject.local_persist_tail_conflict` — stale/concurrent writer tail expectation (retry with fresh tail)
5. `reject.local_persist_quota` — backend quota exceeded (capacity action)
6. `reject.local_persist_readonly` — write attempted on read-only backend/mount

Hardening vectors:

1. `LOCAL-PERSIST-005`: filesystem read-only write rejection class
2. `LOCAL-PERSIST-006`: schema version skew rejection class
3. `LOCAL-PERSIST-007`: malformed persisted row classified as corruption
4. `LOCAL-PERSIST-008`: composite stale-tail conflict against external durable append
5. `LOCAL-PERSIST-009`: composite blob fallback after restart (cold cache -> durable read)

### Backend selection

1. **memory** — ephemeral; process exit loses peer-local state unless server still holds authority.
2. **file** — durable across pod restarts; mount a PVC at `NODALMERGE_HEADLESS_DATA_DIR` (image default `/data`).
3. **composite** — write-through memory cache + file durability (§4b pilot); same `data_dir` as `file`.
4. **registered:\<name\>** — opens a backend from `nodalmerge_runtime_local` registry (built-in: `registered:composite`).
5. Custom backends — `register_backend` + `PersistenceHandle::from_arc`; must pass `LOCAL-PERSIST-*` vectors.
6. Production hardening gate: `docs/PEER_LOCAL_PRODUCTION_HARDENING_CHECKLIST.md`.

### In-process peer-local on .NET host (`NodalMerge.DotNetHost`)

Optional mirror of inbound WS `pack` traffic into `nodalmerge-runtime-local-ffi` (same peer-local semantics as headless, without a sidecar).

| Setting | Meaning |
|---|---|
| `NodalMerge:Runtime:PeerLocal:Enabled` | `true` to open `LocalPersistFfiClient` at host startup |
| `NodalMerge:Runtime:PeerLocal:Backend` | `memory`, `embedded`/`file`, `composite` |
| `NodalMerge:Runtime:PeerLocal:DataDir` | Required for durable backends |

Build native library before pack/run:

```bash
cargo build -p nodalmerge-runtime-local-ffi --release
```

Set `NODALMERGE_LOCAL_FFI_DLL` when the DLL is not beside the host binary. NuGet native packages (`NodalMerge.DotNetHost.Native.*`) include both `nodalmerge_host_ffi` and `nodalmerge_runtime_local_ffi` after `nodalmerge-host/pack-local-nuget.ps1`.

Integration smoke: `scripts/integration-smoke.ps1` (vectors + live `nodalmerge run`). Pass `-TokenJsonPath` for locked-room `archive` / `query` CLI steps.

### Room-family topology (manager/worker)

Use `nodalmerge topology` CLI (`nodalmerge-cli` crate) against the same reflector for child rooms and promotion. Headless workers hold child-room peer-local state; topology commands require `topology.admin` on locked rooms. See `docs/MANAGER_WORKER_TOPOLOGY_PLAYBOOK.md`. Promotion counters and histograms are listed under [Topology promotion metrics](#topology-promotion-metrics-wave-3) (metrics scrape only; no in-repo dashboards yet).

Governance and drills:

1. policy templates: `docs/TOPOLOGY_GOVERNANCE_POLICY_TEMPLATES.md`
2. operator rehearsal: `docs/TOPOLOGY_OPERATOR_DRILL_RUNBOOK.md`
