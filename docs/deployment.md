# ActiveSync Server — Deployment

The `activesync-server` binary is a websocket reflector: it multiplexes peers
by room, merges and relays packs, and (optionally) persists room state to
disk. This document covers the operational basics.

> For a 5-minute Docker + JWT walkthrough, see [self-host.md](./self-host.md).

## Quick start

```powershell
# In-memory only (default) — room state vanishes when the process stops.
cargo run -p activesync-server

# With on-disk persistence rooted at ./data
cargo run -p activesync-server -- --store ./data
```

Default listen address: `ws://127.0.0.1:7878/ws/<room_id>`.

## Storage (`--store <path>`)

When `--store` is passed, the server writes every accepted node to a SQLite
database and every accepted blob to a content-addressed file. On startup each
room is hydrated from disk *before* its first client connects.

```
<path>/
  activesync.db              SQLite — one row per (room, node)
  blobs/
    <sanitized_room_id>/
      <blake3_hex>            one file per blob
```

- **Engine:** bundled SQLite via `rusqlite`. No external SQLite install needed.
- **Schema:** `nodes(room_id TEXT, node_id BLOB, bytes BLOB, seq INTEGER PK AUTOINCREMENT, UNIQUE(room_id, node_id))`.
  Rows are written in acceptance order; hydrate replays in `seq` order so the
  DAG re-builds deterministically.
- **Blobs:** one file per blob. The filename is the hex Blake3 hash; the file
  contents are the raw bytes. Integrity is re-checked on hydrate — tampered
  files are logged and dropped.
- **Idempotency:** re-persisting the same node or blob is a no-op (`INSERT OR
  IGNORE` for nodes; file existence check for blobs).
- **Durability:** the DB is opened in WAL + `synchronous=NORMAL`. Blob writes
  use write-tmp-then-rename.

### Sanitization

Room ids become directory names for blobs. Every byte outside `[A-Za-z0-9_-]`
is escaped as `_HH` (two upper-hex digits). `my/room!` becomes `my_2Froom_21`.

### Backups

- **Nodes:** `.backup` the SQLite DB, or copy the file (`activesync.db`,
  `activesync.db-wal`, `activesync.db-shm`) while the server is stopped.
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

- `RUST_LOG` — overrides the default filter
  (`info,activesync_server=info,activesync_core=info`).
- Server keypair: auto-generated at `~/.activesync/server.key` on first run;
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
