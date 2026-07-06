# nodalmerge-bridge

Rust/WASM bridge exposing `nodalmerge-core` to JavaScript runtimes.

## Scope

- `SyncStore` runtime handle for local state graph operations
- pack/import helpers for sync exchange
- replay and canonical hash verification helpers
- blob CAS helpers and topology/introspection helpers

The generated npm package is under `bridge/pkg`.
