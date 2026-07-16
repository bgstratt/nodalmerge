# Changelog

All notable changes to the NodalMerge .NET host packages (`NodalMerge.Host.Abstractions`,
`NodalMerge.Host.Composition`, `NodalMerge.DotNetHost`, `NodalMerge.DotNetHost.Native.win-x64`,
`NodalMerge.DotNetHost.Native.linux-x64`) are documented here.

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
