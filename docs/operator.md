# Operator runbook

Steady-state operation of a deployed `activesync-server`. Covers every
CLI flag, every metric, backup/restore for each supported persistence
backend, and the rolling-restart procedure.

Migration note: this runbook is nodalmerge-first. During the compatibility
window, `activesync-server` command forms and `activesync_*` identifiers remain
supported aliases.

> First-time readers: see [quickstart.md](./quickstart.md) and
> [self-host.md](./self-host.md). For integration shapes, see
> [integration.md](./integration.md). For version upgrades, see
> [migration.md](./migration.md). For cross-surface operation inventory and
> gap analysis, see [operations-inventory.md](./operations-inventory.md).

---

## Config reference

All configuration is via CLI flags on `nodalmerge-server`. There is no
config file. Flags accept both `--flag value` and `--flag=value` form.

| Flag | Default | Meaning |
|---|---|---|
| `--store <path>` | *(in-memory)* | Durable persistence root. Creates `<path>/activesync.db` + `<path>/blobs/`. See [deployment.md](./deployment.md). |
| `--metrics-addr <ip:port>` | *(off)* | Admin HTTP listener for Prometheus scrape. Bind to loopback or a private VPC subnet — never the public WS port. |
| `--idle-timeout <secs>` | `300` | Evict rooms with zero connected peers after N seconds. `0` disables. Gated on durable persistence; warns + skips without `--store`. |
| `--broadcast-capacity <N>` | `512` | Per-room `broadcast::channel` ring size (G1). Larger = more slack for brief client stalls; smaller = faster divergence detection. `0` rejected. |
| `--peer-rate-nodes <N>` | `200` | Per-peer ceiling in nodes per second (G3). `0` disables. |
| `--peer-rate-bytes <MiB>` | `4` | Per-peer ceiling in decoded-pack bytes per second (G3). `0` disables. |
| `--blob-gc-interval <secs>` | `0` | Periodic blob GC sweep interval (G4). `0` disables. Durable-only. |
| `--blob-gc-grace <secs>` | `86400` | Tombstone grace before an orphan blob is deleted. |

**Environment.**

- `RUST_LOG` — tracing filter. During migration, logger targets remain
  `activesync_*` (for example `info,activesync_server=info,activesync_core=info`).
- Server keypair: auto-generated at `~/.activesync/server.key` on
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
| `nodalmerge_blob_gc_deleted_total` | counter | `room` | steady rate confirms GC is running |
| `nodalmerge_lamport_rejected_total` | counter | `reason` (`ceiling` / `wall_skew`) | any non-zero = client clock broken or malicious |
| `nodalmerge_token_expired_disconnects_total` | counter | `room` | trend; rate should correlate with token TTL |
| `nodalmerge_room_bytes_resident` | gauge | `room` | capacity; approximate — per-node estimate is flat 512 B, under-counts large transactions |

Compatibility note: legacy `activesync_*` metric names are still emitted while
dashboards migrate.

Histogram buckets are hand-tuned for the hot path:

- `nodalmerge_merge_batch_seconds`: 50µs … 2.5s (covers single-node
  packs through 10k-node catchup).
- `nodalmerge_persistence_write_seconds`: 100µs … 500ms (typical node
  INSERT is <1 ms; >100 ms is an alerting signal).

**Admission.** `metrics::init` installs a *process-global* recorder; a
second install fails. Install failure is logged and the server
continues without observability.

---

## Backup + restore

### `DirPersistence` (SQLite + files)

Layout (see [deployment.md](./deployment.md) for full detail):

```
<path>/
  activesync.db              SQLite (legacy file name retained during migration)
  blobs/<sanitized_room>/<blake3_hex>
  blob-tombstones/<sanitized_room>/<blake3_hex>
```

- **Hot backup:** `sqlite3 activesync.db ".backup '/dest/activesync.db'"`
  + a recursive copy of `blobs/`. SQLite is WAL, so the `.backup`
  command is consistent.
- **Cold backup:** stop the server, copy `activesync.db`,
  `activesync.db-wal`, `activesync.db-shm`, and `blobs/`.
- **Restore:** point a new `--store <path>` at the copy.
- **Consistency guarantee:** nodes and blobs are content-addressed;
  a partially restored `blobs/` only loses the blobs whose files are
  missing — the DAG re-references them safely and peers will
  re-upload on demand.

### `PostgresNodeStore`

- **Backup:** whatever your existing Postgres backup regime is (WAL
  archiving, logical dumps, managed provider snapshots). Tables to
  include: `activesync_nodes` + the migration table sqlx generates.
- **Restore:** restore the DB; point
  `PostgresNodeStore::connect_and_migrate(cfg)` at it. Migrations
  are idempotent.
- **Consistency guarantee:** `UNIQUE(room_id, node_id)` with
  `INSERT ... ON CONFLICT ... DO NOTHING` means re-plays are safe.

### `MongoNodeStore`

- **Backup:** `mongodump` or your managed provider's snapshot.
  Include `activesync_nodes` + `activesync_seq`.
- **Restore:** `mongorestore`. The `(room_id, seq)` compound index
  and compound `_id` are recreated on first connect.
- **Consistency guarantee:** compound `_id = "<room>:<node_hex>"`
  makes re-inserts idempotent (duplicate-key on `E11000` is treated
  as success).

### `S3BlobStore`

- **Backup:** bucket-level versioning + cross-region replication in
  your S3 provider. Path prefix is configurable per deployment.
- **Restore:** no sync-server action needed. `S3BlobStore` looks up
  blobs on demand via `resolve_get_url`; missing blobs re-upload
  naturally on next `setBlob`.
- **Consistency guarantee:** blobs are content-addressed Blake3. A
  bucket with a subset of blobs is always safe; missing objects
  simply produce fresh uploads.

---

## Capacity planning

Rules of thumb, derived from the bench suite (`cargo bench -p
activesync-core`, reference machine = Ryzen 9 5900X, 12 cores):

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

ActiveSync's `A7` capability negotiation handles the mixed-version
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

### Query/Projection failure triage checklist

1. Capture request envelope, response type, `reason_class`, and `reason_message`.
2. If available, capture `projection_id`, `query_spec_id`, checkpoint selector, and digest fields.
3. Classify failure bucket:
4. selector/compatibility class: fix payload and retry once.
5. not-found/lifecycle class: repair query spec or projection state before retry.
6. replay/digest mismatch class: escalate as deterministic parity incident.
7. For repeated rejects with identical payload, stop retries and open an incident with captured envelopes.
