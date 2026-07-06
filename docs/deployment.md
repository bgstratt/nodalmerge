# NodalMerge Server — Deployment

Authorization execution references:
- [AUTHORIZATION_CORE_HOST_SEPARATION_PLAN.md](AUTHORIZATION_CORE_HOST_SEPARATION_PLAN.md)
- [AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md](AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md)

The `nodalmerge-server` binary is a websocket reflector: it multiplexes peers
by room, merges and relays packs, and (optionally) persists room state to
disk. This document covers the operational basics.

> For a 5-minute Docker + JWT walkthrough, see [self-host.md](./self-host.md).

## Quick start

```powershell
# In-memory only (default) — room state vanishes when the process stops.
cargo run -p nodalmerge-server

# With on-disk persistence rooted at ./data
cargo run -p nodalmerge-server -- --store ./data
```

Default listen address: `ws://127.0.0.1:7878/ws/<room_id>`.

## Storage (`--store <path>`)

When `--store` is passed, the server writes every accepted node to a SQLite
database and every accepted blob to a content-addressed file. On startup each
room is hydrated from disk *before* its first client connects.

```
<path>/
  nodalmerge.db              SQLite — one row per (room, node)
  blobs/
    blake3/
      <hex>                  one file per blob — global CAS, no room segment
    .tombstones/
      blake3/
        <hex>                empty marker; blob GC (see below)
    .layout-v2               empty marker; presence = already-canonical layout
```

- **Engine:** bundled SQLite via `rusqlite`. No external SQLite install needed.
- **Schema:** `nodes(room_id TEXT, node_id BLOB, bytes BLOB, seq INTEGER PK AUTOINCREMENT, UNIQUE(room_id, node_id))`.
  Rows are written in acceptance order; hydrate replays in `seq` order so the
  DAG re-builds deterministically.
- **Blobs:** a single content-addressed pool shared across every room — see
  [docs/BLOB_STORAGE_LAYOUT.md](BLOB_STORAGE_LAYOUT.md) for the full contract.
  The filename is the hex Blake3 hash; the file contents are the raw bytes.
  Integrity is re-checked on hydrate — tampered files are logged and dropped.
- **Idempotency:** re-persisting the same node or blob is a no-op (`INSERT OR
  IGNORE` for nodes; file existence check for blobs — a blob shared by two
  rooms is stored once).
- **Durability:** the DB is opened in WAL + `synchronous=NORMAL`. Blob writes
  use write-tmp-then-rename.
- **Upgrading from 0.1.x:** on first open, a legacy per-room blob layout
  (`blobs/<sanitized_room_id>/<hex>`) is migrated automatically into the
  layout above — see docs/BLOB_STORAGE_LAYOUT.md §6. No manual step for file
  stores; direct-S3 deployments migrate manually (below).

### Backups

- **Nodes:** `.backup` the SQLite DB, or copy the file (`nodalmerge.db`,
  `nodalmerge.db-wal`, `nodalmerge.db-shm`) while the server is stopped.
- **Blobs:** a plain recursive copy of `blobs/` works because each file is
  content-addressed and self-verifying.
- **Restore:** point a new `--store` at the copy.

### Migration between backends

- **Mem → disk:** stop the server with `--store` unset, restart with
  `--store <path>`. Rooms start fresh; peers will push their local state on
  reconnect and the server will persist it from that point on.
- **Disk → mem:** stop with `--store`, restart without. State stays on disk
  but is not loaded.

## Environment

- `RUST_LOG` — overrides the default filter.
  Example: `info,nodalmerge_server=info,nodalmerge_core=info`.
- Server keypair: auto-generated at `server.key` in the server's working directory on first run;
  reused on subsequent starts (E1).

## Operational notes

- **Room eviction:** rooms with zero connected peers are evicted after
  `--idle-timeout <seconds>` (default 300, i.e. 5 min; `0` disables).
  A 60 s background sweeper does the work. Eviction is gated on durable
  persistence — passing `--idle-timeout` without `--store` logs a warning
  at startup and the sweeper never runs (dropping an in-memory room is
  data loss). On rejoin, an evicted room is rebuilt from the SQLite DB
  and blobs directory via the same path as a cold restart.
- **Write amplification:** every accepted node triggers one SQLite INSERT;
  every accepted blob one `fsync`-backed file create. For high-churn rooms on
  spinning disks this is the primary bottleneck — co-locate the `--store`
  path on an SSD.
- **Replay performance:** the integration test `large_room_hydrates_quickly`
  spins up a 10,000-node room and hydrates it in under 5s on an AMD Ryzen 9
  5900X. Budget accordingly for larger rooms.
- **Sentinel keys:** the subscription filter always passes through keys that
  begin with `\x00` (E2EE envelopes, snapshot meta). Don't try to scope them
  via patterns — they must reach every peer verbatim.
- **Backpressure:** the per-room broadcast ring buffer holds
  `--broadcast-capacity <N>` messages (default `512`; `0` is rejected). A
  consumer that falls behind is closed with WS code `4001 resync required`
  and bumps `nodalmerge_broadcast_lagged_total{room}`; the SDK's
  exp-backoff reconnect runs the normal recovery (hello → IBF → catch-up).
  Every application send is wrapped in a 5 s timeout — on timeout the
  peer is closed with `1011 server overload` and
  `nodalmerge_ws_send_timeout_total{room}` is incremented. Tradeoff:
  larger capacity = more slack for brief stalls; smaller = faster
  divergence detection.
- **Rate limiting:** every peer gets two independent token buckets,
  checked *before* Ed25519 verification so floods can't burn CPU:
  - `--peer-rate-nodes <N>` (default `200`; `0` disables): inbound
    nodes per second.
  - `--peer-rate-bytes <MiB>` (default `4`; `0` disables): inbound
    decoded-pack bytes per second.
  On violation (quota exceeded *or* a single pack larger than the 1-second
  burst) the peer is closed with WS code `4008 rate limit exceeded` and
  `nodalmerge_rate_limit_drops_total{peer}` is incremented (`peer` = first
  12 hex chars of the peer pubkey). The server's own signing key is
  exempt so the authoritative tick loop is never throttled. Well-behaved
  clients should split large packs; unsplit packs larger than the burst
  are treated as malicious/mis-configured.
- **Blob garbage collection (G4):** `DirPersistence` never reclaimed old
  blob files on its own. Opt into a periodic sweep with:
  - `--blob-gc-interval <secs>` (default `0` = disabled): how often to
    sweep the store.
  - `--blob-gc-grace <secs>` (default `86400` = 24 h): minimum time a
    blob must be orphaned-on-disk before deletion.
  Blobs are a single global content-addressed pool (see
  [docs/BLOB_STORAGE_LAYOUT.md](BLOB_STORAGE_LAYOUT.md)), so each sweep
  covers *every* room the store knows about — currently-loaded rooms via
  their in-memory graph, plus every other room with persisted nodes via a
  fresh scan — not just resident ones. The sweeper runs a **two-phase
  protocol**. First visit: any blob not referenced by *any* `SetBlob` op
  in any room's DAG gets an empty tombstone file written at
  `<store>/blobs/.tombstones/blake3/<hash>`. Subsequent visit: if the
  tombstone is older than the grace window **and** the blob is still
  orphaned, blob + tombstone are deleted together and
  `nodalmerge_blob_gc_deleted_total` is incremented. If a blob becomes
  live again (a peer re-publishes a `SetBlob` referencing it) the
  sweeper clears its tombstone instead. Set `--blob-gc-grace 0` to
  collapse the two phases into a single aggressive pass. Requires a
  durable store — the flag is a warning-and-skip no-op with the default
  in-memory persistence.
- **Node sanity checks (G5):** every inbound node is compared against
  two cheap ceilings *before* Ed25519 verification, so malformed
  floods never burn crypto CPU:
  - **Lamport ceiling.** Nodes whose `transaction.lamport` exceeds the
    local `graph.lamport() + 1 048 576` (`LAMPORT_SLACK = 1<<20`) are
    rejected. Legitimate concurrent-writer fan-out stays well under
    this window; crossing it indicates a tampered or malicious node.
  - **Wall-clock skew.** Nodes whose `transaction.wall_ms` is more
    than 24 h past the server's current time are rejected. `wall_ms`
    is informational (never used for merge ordering), so the check
    only closes the "sort-me-to-the-top-of-the-timeline" class of
    abuse for UIs that render by wall clock. `wall_ms == 0`
    (compaction snapshots and legacy unsigned clients) is always
    accepted. No tuning needed — the 24 h window absorbs client
    drift and NTP stumbles.
  Rejects surface in `import_nodes` logs and bump
  `nodalmerge_lamport_rejected_total{reason}` where `reason` is
  `ceiling` or `wall_skew`. A sustained non-zero rate points at a
  misbehaving client or badly-synced clocks (often the server's own
  clock drift).
- **Token expiry enforced mid-session (G6):** in locked rooms
  (`room.auth_key` set), the server captures the `expiry_secs` field
  of each accepted `RoomToken` once at authentication time and holds
  it as a per-session deadline. When the deadline lapses the WS is
  closed with code `4002 token expired` and
  `nodalmerge_token_expired_disconnects_total{room}` is incremented.
  Admission-time expiry (`now >= expiry_secs` at `hello`) still
  rejects with `4001 unauthorized` — `4002` is exclusively the
  mid-session signal. The SDK's `getToken` hook should refresh
  opportunistically before `expiry_secs` and reconnect on `4002`:
  rotate tokens on the short side (minutes, not days) so a leaked
  token's blast radius is bounded by the TTL. Unlocked rooms (no
  `auth_key`) have no session deadline. No server flags, no wire
  change, no new state.

## Compaction Cadence And Replay Truncation (Phase B)

Use these defaults unless workload data suggests otherwise:

- Scheduled compaction: set `--snapshot-interval 2000` for moderate/high churn rooms.
- Incremental chain bound: keep `--snapshot-max-chain 10` so restore cost stays bounded.
- Manual emergency compaction: trigger runtime `{ "type": "compact-room" }` from an admin-authorized peer.

Replay truncation watermark semantics:

- Effective replay watermark is the latest accepted snapshot node for a room.
- Nodes strictly before the watermark are considered truncated history for restore purposes.
- Restore path must rebuild from `snapshot + post-snapshot delta`; pre-watermark history is not required for correctness.

Rollback path (operator):

1. Stop the server for the target deployment.
2. Restore `nodalmerge.db` (+ `-wal`/`-shm` when present) and `blobs/` from backup.
3. Start server with the restored `--store` path and same room auth policy configuration.
4. Run the snapshot restore drill to verify deterministic restore/hash semantics:
   `pwsh -File .\docs\acceptance\Run-SnapshotRestoreDrill.ps1`
5. If drill fails, keep the rollback deployment isolated and investigate snapshot metadata/hash mismatch before reopening traffic.

CI drill contract:

- Canonical nightly runs `docs/acceptance/Run-SnapshotRestoreDrill.ps1`.
- Artifact `docs/acceptance/snapshot-restore-drill.json` must report:
  - `status = pass`
  - `assertions.snapshot_restore_forward_replay = true`
  - `assertions.snapshot_hash_equality = true`

## Metrics (`--metrics-addr <ip:port>`)

When set, the server installs a Prometheus exporter on a **separate admin
port** and serves `/metrics` there. The public WS port is unchanged. Default
off — absent the flag, no recorder is installed.

```
nodalmerge-server --store ./data --metrics-addr 127.0.0.1:9090
curl http://127.0.0.1:9090/metrics
```

Bind to loopback or a private subnet; the endpoint has no auth. If the
install fails (port taken, already installed in-process) the server logs a
warning and continues without observability.

Baseline series (primary):

| Metric | Kind | Labels |
|---|---|---|
| `nodalmerge_rooms_total` | gauge | — |
| `nodalmerge_peers_total` | gauge | `room` |
| `nodalmerge_nodes_accepted_total` | counter | `room` |
| `nodalmerge_merge_batch_seconds` | histogram | — |
| `nodalmerge_persistence_write_seconds` | histogram | `kind=node\|nodes_batch\|blob` |
| `nodalmerge_eviction_total` | counter | — |
| `nodalmerge_broadcast_lagged_total` | counter | `room` |
| `nodalmerge_ws_send_timeout_total` | counter | `room` |
| `nodalmerge_rate_limit_drops_total` | counter | `peer` |
| `nodalmerge_blob_gc_deleted_total` | counter | — (global pool, no room label) |
| `nodalmerge_lamport_rejected_total` | counter | `reason` |
| `nodalmerge_token_expired_disconnects_total` | counter | `room` |

Histograms ship with hand-tuned buckets (µs-scale for merges and persistence
writes) so Prometheus `histogram_quantile(0.99, …)` works without extra
config. All Phase G operational-safety counters are now registered.
