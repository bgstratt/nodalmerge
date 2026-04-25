# Operator runbook

Steady-state operation of a deployed `activesync-server`. Covers every
CLI flag, every metric, backup/restore for each supported persistence
backend, and the rolling-restart procedure.

> First-time readers: see [quickstart.md](./quickstart.md) and
> [self-host.md](./self-host.md). For integration shapes, see
> [integration.md](./integration.md). For version upgrades, see
> [migration.md](./migration.md).

---

## Config reference

All configuration is via CLI flags on `activesync-server`. There is no
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

- `RUST_LOG` — tracing filter. Default
  `info,activesync_server=info,activesync_core=info`.
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
| `activesync_rooms_total` | gauge | — | capacity trend, not alerting |
| `activesync_peers_total` | gauge | `room` | capacity; sudden drop = mass disconnect |
| `activesync_nodes_accepted_total` | counter | `room` | rate trend; low-signal alone |
| `activesync_merge_batch_seconds` | histogram | — | `p99 > 100 ms` for 5 min → verify CPU / ed25519 batch size |
| `activesync_persistence_write_seconds` | histogram | `kind` (`node`/`nodes_batch`/`blob`) | `p99 > 50 ms` sustained → disk saturation, DB slowdown |
| `activesync_eviction_total` | counter | — | trend; high rate in production = peer churn |
| `activesync_broadcast_lagged_total` | counter | `room` | `rate > 0` = slow clients; chronic = bump `--broadcast-capacity` or investigate peer |
| `activesync_ws_send_timeout_total` | counter | `room` | any non-zero = TCP or client stalled; investigate network |
| `activesync_rate_limit_drops_total` | counter | `peer` | any non-zero = misbehaving (or misconfigured) client |
| `activesync_blob_gc_deleted_total` | counter | `room` | steady rate confirms GC is running |
| `activesync_lamport_rejected_total` | counter | `reason` (`ceiling` / `wall_skew`) | any non-zero = client clock broken or malicious |
| `activesync_token_expired_disconnects_total` | counter | `room` | trend; rate should correlate with token TTL |
| `activesync_room_bytes_resident` | gauge | `room` | capacity; approximate — per-node estimate is flat 512 B, under-counts large transactions |

Histogram buckets are hand-tuned for the hot path:

- `activesync_merge_batch_seconds`: 50µs … 2.5s (covers single-node
  packs through 10k-node catchup).
- `activesync_persistence_write_seconds`: 100µs … 500ms (typical node
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
  activesync.db              SQLite
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
- **Node bytes.** `activesync_room_bytes_resident` assumes 512 bytes
  per node; real numbers on the wire are 100-300 B for small ops,
  much more for large `SetBlob` payloads (payload ≠ blob bytes).
- **Blob bytes.** No theoretical limit per room; budget per product.
  G4's GC keeps orphan blob accumulation bounded.
- **Peers per replica.** One `tokio::task` per peer WS; one broadcast
  receiver per peer per room. CPU bound by merge path; network bound
  by broadcast fan-out. No hard cap — size based on `p99` of
  `activesync_merge_batch_seconds`.
- **Handshake size.** IBF hello is 2.9 KB regardless of graph size;
  MST handshake ≤3 round trips for a 1000-node diff in a 2000-node
  graph.

**When to shard rooms across replicas.** Once `activesync_peers_total`
for a single room crosses 100 or `p99` of
`activesync_merge_batch_seconds` exceeds 100 ms. Shard by room id
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
| Mass disconnects with `4001 resync required` | `activesync_broadcast_lagged_total` per room; investigate one slow peer, or bump `--broadcast-capacity`. |
| Mass disconnects with `4008 rate limit exceeded` | Per-peer `activesync_rate_limit_drops_total`; investigate the offending pubkey. |
| Disconnects with `4002 token expired` | Normal if token TTL is short; confirm SDK is refetching tokens on reconnect. |
| `activesync_lamport_rejected_total` climbing | Client clock skew or malicious peer. Cross-check with server logs (peer pubkey prefix is logged). |
| `activesync_merge_batch_seconds` p99 rising | CPU saturation; scale horizontally (shard by room) or verify ed25519 simd backend is active. |
| `activesync_persistence_write_seconds{kind="blob"}` p99 rising | Disk I/O or S3 latency; check the blob backend. |
| Disk fills | `activesync_blob_gc_deleted_total` not ticking; verify `--blob-gc-interval` is set. |
