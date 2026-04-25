# Migration guide

Phase-boundary deltas for consumers upgrading an existing ActiveSync
deployment. Each section is scoped narrowly: schema changes, new config
knobs, wire-level back-compat, and whether any code changes are
required. Skip sections you've already crossed.

> This doc is for *upgrading*. If you're integrating for the first time,
> start at [quickstart.md](./quickstart.md) and
> [integration.md](./integration.md).

---

## Before F4 → after F4: persistent store

**Change.** Server gained `--store <path>` for on-disk durability
(SQLite for nodes, file-per-blob for bytes). In-memory remains the
default.

- **Schema.** First start with `--store` creates
  `<path>/activesync.db` and `<path>/blobs/`. See the layout in
  [deployment.md](./deployment.md).
- **Config.** `--store <path>`, `--idle-timeout <secs>` (default 300;
  eviction is gated on durable persistence).
- **Wire.** No change.
- **Code.** None — the default is still in-memory.
- **Rollback.** Stop with `--store`, restart without. On-disk state is
  preserved; it's simply not hydrated.

---

## Before F5 → after F5: JWT → RoomToken bridge

**Change.** `activesync-jwt-bridge` crate added. Your existing JWT
issuer (Clerk / Supabase / Auth0 / homegrown) becomes the identity
layer; a tiny Rust service mints `RoomToken`s from verified JWTs.

- **Schema.** None.
- **Config.** Bridge owns the room key(s). Server is unchanged.
- **Wire.** None — the token format is identical to a self-minted one.
- **Code.** Wrap `mint_room_token` in ~30 lines of Axum (see
  [self-host.md](./self-host.md) for the full example).
- **Rollback.** Clients self-mint tokens; retire the bridge.

---

## Before F6 → after F6: pluggable blob store + capability flag

**Change.** `ServerPersistence` split into `NodePersistence` +
`BlobPersistence`. S3-compatible blob store shipped
(`activesync-s3-blobs`). New capability flag
`supports_direct_blob_io` in `A7` handshake.

- **Schema.** None for `DirPersistence` (still ships unchanged).
- **Config.** S3 backends take `S3BlobStoreConfig` (endpoint, bucket,
  region, path prefix, auth mode, presign TTLs, direct-upload
  threshold). `Composite::new(nodes, blobs)` wires two halves.
- **Wire (additive, capability-gated).** Three new messages:
  `blob-redirect`, `request-upload` / `upload-granted` /
  `upload-denied`, `blob-uploaded`. All gated on
  `supports_direct_blob_io`; old SDKs never see them.
- **Code.**
  - **Self-host using `DirPersistence`:** no changes.
  - **SaaS switching to S3:** change one line from
    `Arc::new(DirPersistence::open(path)?)` to
    `Arc::new(Composite::new(nodes, S3BlobStore::new(cfg)?))`. See
    [integration.md](./integration.md).
  - **Frontend:** no API change. `setBlob` / `getBlob` signatures
    are identical; size-based bifurcation is internal.
- **Rollback.** Compose `DirPersistence` (or any other
  `BlobPersistence` impl) in place of `S3BlobStore`. Existing blobs
  do *not* automatically migrate — move them out-of-band if needed.
- **Gotcha.** The SDK caches presigned GET URLs until 60 s before
  expiry. Aggressive key rotation at the S3 layer can outrun the
  cache; keep `presign_get_ttl` ≥ your rotation window or expect
  403 retries (which the SDK handles gracefully with fresh redirects).

---

## Before F7 → after F7: database node-store adapters

**Change.** `activesync-mongo-store` and `activesync-postgres-store`
crates shipped. `DirPersistence` (SQLite) still ships.

- **Schema.** Adapter-specific, created on first run.
  - **Mongo:** `activesync_nodes` + `activesync_seq` collections;
    compound index on `(room_id, seq)`; compound `_id` for idempotent
    inserts.
  - **Postgres:** `activesync_nodes` table; `sqlx::migrate!()` runs
    on startup. `UNIQUE(room_id, node_id)` + index on
    `(room_id, seq)`.
- **Config.** `MongoNodeStoreConfig` / `PostgresNodeStoreConfig`.
  Both use a connection URI and per-DB defaults.
- **Wire.** No change.
- **Code.** Swap `DirPersistence` for
  `Composite::new(MongoNodeStore::connect(cfg).await?, blobs)` (or
  the Postgres equivalent). The `Composite` blanket impl means no
  other code changes.
- **Rollback.** Your DB is the source of truth; rolling back the
  sync-server binary does not drop data. Swap the store back and the
  DAG rehydrates on next room open.
- **Gotcha.** `DirPersistence` and a DB-backed store hold the *same*
  shape of data (opaque postcard node bytes keyed by id) but there is
  no built-in migrator. If you need to move from SQLite to Postgres,
  dump nodes via `load_room_nodes` and re-`persist_nodes` into the new
  backend room by room.

---

## Before F8 → after F8: List CRDT

**Change.** `doc.list<T>(key)` now returns a `ListHandle` instead of
throwing. New capability flag `supports_list_crdt` in `A7`.

- **Schema.** None. `Op::List { Insert, Move, Delete }` is a new
  variant; old peers decode it as unknown and drop — lists simply
  don't appear for them (additive).
- **Config.** None.
- **Wire.** Additive. Capability-gated; mixed-version rooms are safe.
- **Code.** Opt-in on the frontend: pick up `ListHandle` wherever you
  used to model ordered collections as Map-with-sortKey. See
  [sdk.md](./sdk.md) for the API.
- **Gotcha.** Apps that previously modeled lists as a map with an
  app-level `sortKey` field will **not** auto-migrate. Convert the
  data on read the first time (or one-time on server, via a
  migration script using the low-level bridge).

---

## Before Phase G → after Phase G: operational safety

All of Phase G is additive and defaults to safe behavior. No schema
or wire changes.

- **G1 (backpressure).** `--broadcast-capacity <N>` (default 512).
  Slow clients close with `4001 resync required`; SDK reconnects and
  recatches up. No frontend code change required.
- **G3 (rate limit).** `--peer-rate-nodes <N>` (default 200, `0`
  disables), `--peer-rate-bytes <MiB>` (default 4, `0` disables).
  Floods close with `4008 rate limit exceeded`.
- **G4 (blob GC).** `--blob-gc-interval <secs>` (default `0`
  disabled), `--blob-gc-grace <secs>` (default 86400). Two-phase
  tombstone — a blob is only deleted on the second sweep past the
  grace window. Durable stores only.
- **G5 (Lamport ceiling).** `LAMPORT_SLACK = 1<<20`,
  `WALL_SKEW_MAX_MS = 86_400_000`. No config. Nodes past the ceiling
  are rejected before signature verify.
- **G6 (token expiry).** Mid-session enforcement. Locked rooms close
  with `4002 token expired` when the token's `expiry_secs` lapses.
  SDK refetches token + reconnects.
- **G7 (metrics).** `--metrics-addr <ip:port>` (default off). Install
  is best-effort; failure is logged and the server continues.
- **G8 (SDK metrics).** `createDoc({ onMetric })` is opt-in and
  zero-cost when unset. See [sdk.md](./sdk.md).
- **G9 (conflicts).** `doc.onConflict(cb)` +
  `doc.recentConflicts(sinceMs)`. Observability, not policy — merge
  behavior is unchanged.
- **G11 (hot-room bound).** Observability slice only: new gauge
  `activesync_room_bytes_resident{room}` (see
  [operator.md](./operator.md)). No enforcement yet.

---

## Notes for mixed-version fleets

ActiveSync's `A7` capability negotiation covers every wire addition
since F6. A peer that doesn't speak a capability simply doesn't see
its wire messages; the server falls back to the legacy path.

**Implication:** rolling restarts don't need a wire cut-over. Upgrade
servers first, SDKs can follow at any pace. See
[operator.md](./operator.md#upgrading-sync-server) for the procedure.
