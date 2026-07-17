# Changelog

All notable changes to the NodalMerge .NET host packages (`NodalMerge.Host.Abstractions`,
`NodalMerge.Host.Composition`, `NodalMerge.DotNetHost`, `NodalMerge.DotNetHost.Native.win-x64`,
`NodalMerge.DotNetHost.Native.linux-x64`) are documented here.

## 0.2.3 — 2026-07-16

- **Fixed: legacy `/sync/blob-url` (+ `/api/sync/blob-url`) compat regression, introduced
  during the 0.2.0 blob-layout-convergence work and never disclosed here.** Between the
  0.2.0 entry below and this release, an internal refactor (slice S4.1, adding the new
  frozen `GET /blobs/{hash}/url` contract) silently turned this pre-existing route — which
  shipped on `main` and predates 0.2.0 — into a thin alias of the new route, changing its
  response shape and status codes without a corresponding disclosure here. Concretely, the
  legacy route had started: emitting `expiresAtUtc` (ISO-8601) instead of its original
  `expiresAt` (unix seconds); answering `501` instead of `404` when no presign-capable
  backend is configured; and rejecting non-canonical/malformed hashes with `400`, which the
  original route never did (it accepts any non-empty hash, anonymously, and always did).
  This release restores the route's original, pre-0.2.0 behavior exactly (see
  `docs/BLOB_HTTP_SURFACE.md`'s "Status and what's deferred" section for the full
  before/after) — no config or client change required for existing pre-S4.1 callers.
  The new `GET /blobs/{hash}/url` contract (introduced by S4.1, unaffected by this fix)
  keeps its frozen shape.
- **Additive: `BlobHttpOptions.Validate()`.** A non-positive `MaxBlobBytes` now fails fast
  at startup with a clear `InvalidOperationException` instead of throwing an unhandled
  `ArgumentOutOfRangeException` on the first chunked PUT (negative) or silently rejecting
  every PUT with 413 (zero).
- **Fixed: a reverse-proxy path prefix in `RemoteBlobOriginOptions:BaseUrl` (e.g.
  `https://host/nodalmerge`) was silently dropped from every request
  `HttpRemoteBlobStoreProvider`/`S3DirectBlobStoreProvider` sent** (they always hit
  `/blobs/{hash}` at the bare host instead of `/nodalmerge/blobs/{hash}`). Deployments
  behind a path-prefixed reverse proxy for the `ChainedRemote` blob provider were affected;
  an unprefixed `BaseUrl` is unaffected.
- **Additive: a startup warning (not a hard failure) when `S3DelegatedBlobOptions` config
  still sets the removed `PutPath`/`GetPath` keys** from the pre-0.2.0 two-path delegate
  presign protocol. These keys have been silently ignored by the options binder since the
  0.2.0 presign-protocol-v1 migration below; a deployment that never migrated its config
  degraded to the WS blob fallback with only a generic warning. Now names the exact stale
  keys and the migration to make.

## 0.2.2 — 2026-07-16

- **Additive: `IInboundPackObserver` hook** (`NodalMerge.Host.Abstractions.Providers`). Invoked
  after engine import + persistence succeed for a genuinely peer-authored inbound `"pack"`
  message on the server-side WebSocket path (`RuntimeWebSocketLoopRunner`, a peer connected to
  this host's `/ws/{room}` endpoint) — never for this host's own outbound/rebroadcast pack
  traffic. Resolved via DI as `IEnumerable<IInboundPackObserver>` through a new
  `RuntimeWebSocketLoopRunner` constructor overload; zero registered observers (today's default)
  reproduces prior behavior exactly. A throwing observer is caught and logged per-observer and
  never breaks the WS loop or the pack's already-completed persistence. No breaking changes: all
  existing public constructors and `RunAsync` overload signatures are unchanged.

## 0.2.1

- Local package bump carried the new host surface plus real win-x64/linux-x64 native runtimes
  (see 0.2.0 below for the blob-storage-layout convergence it built on). No `NodalMerge.DotNetHost`
  public API changes recorded against 0.2.0 at this revision.

## 0.2.0 — 2026-07-06

- **Breaking: blob storage layout convergence.** File and S3 blob stores on
  both the Rust and .NET runtimes now share one canonical layout: a flat,
  global content-addressed pool at `<root>/blake3/<hex>` (no room scoping,
  no sharding, no file extension). See
  [docs/BLOB_STORAGE_LAYOUT.md](../../docs/BLOB_STORAGE_LAYOUT.md).
- **0.1.x blob stores are not readable by 0.2.x and vice versa.** File
  stores migrate themselves automatically and once, on first open after
  upgrading (marker-gated, crash-safe, non-conforming files quarantined
  rather than deleted). Direct-S3 operators must run the documented manual
  migration. Delegated-S3 data was already canonical and needs no
  migration.
- **Delegate presign protocol unified to v1**: one endpoint, `op` in the
  request body (`op/room/hash/algorithm/size/ttl_seconds/content_type/
  namespace`), response is `{"url": ...}` only — the caller computes
  expiry locally from the TTL it sent.
- Blob GC now sweeps the true global live set (all rooms, not just
  resident ones), fixing a correctness gap where blobs belonging to idle
  rooms could be wrongly collected.

## 0.1.2 — 2026-07-02

Core engine improvements carried in from the underlying `nodalmerge-core` crate:

- **Text projection rewrite**: replaced the per-character parallel structures (hashmap
  entries/children/positions, O(n) shift loops, full-forest DFS integration) with a chunked
  order-statistic RGA (128-char chunks indexed by Fenwick trees over visible chars/bytes).
  50k-op replay throughput: 5.5k -> 214k ops/sec.
- **Incremental map/list/blob/conflict caches**: `StateGraph` now updates LWW map winners,
  list projections, and referenced-blob-hash sets incrementally at apply time instead of
  replaying full history on every read. Conflict detection now streams winner/loser pairs
  as they happen instead of rescanning history. No public API, FFI, or wire changes.
- Added a crdt-benchmarks B4 parity harness (automerge-perf editing trace replay) to track
  these gains going forward: 882ms / ~294,500 ops/sec apply, 947ms cold-start.

## 0.1.1

- Initial published baseline.
