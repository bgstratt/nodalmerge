# Changelog

All notable changes to the NodalMerge .NET host packages (`NodalMerge.Host.Abstractions`,
`NodalMerge.Host.Composition`, `NodalMerge.DotNetHost`, `NodalMerge.DotNetHost.Native.win-x64`,
`NodalMerge.DotNetHost.Native.linux-x64`) are documented here.

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
