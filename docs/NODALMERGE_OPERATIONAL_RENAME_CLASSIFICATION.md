# NodalMerge Operational Rename Classification

Updated: 2026-05-25

Scope: remaining `activesync-` hits in non-doc, non-wrapper operational files.

## Compatibility Retirement Status

Wave A (completed 2026-05-26):

1. Removed Docker runtime symlink alias (`activesync-server`).
2. Removed legacy Mongo container fallback alias (`activesync-mongo`) from root run scripts.
3. Removed legacy Rust bin aliases (`activesync-server`, `activesync-dev-server`) from crate manifests.
4. Removed legacy web storage-key fallback reads and switched browser demo IndexedDB default to `nodalmerge-v7`.

Post-Wave-A residual source-level operational hits (excluding docs/wrappers/target artifacts): `22`.

Wave B (completed 2026-05-26):

1. Renamed bridge crate/package identity to `nodalmerge-bridge` (`bridge/Cargo.toml`).
2. Renamed SDK npm identity/dependency/imports to `nodalmerge-*` (`sdk-js/package.json`, `sdk-js/index.js`, `sdk-js/index.test.js`).
3. Renamed protocol/domain constants from `activesync-*` to `nodalmerge-*` in core crypto/IBF/MST/token modules.
4. Renamed generated wasm package metadata to `nodalmerge-bridge` (`bridge/pkg/package.json`, `web/pkg/package.json`).
5. Removed remaining legacy package IDs in artifact packaging script (`pack-local-artifacts.ps1`).
6. Refreshed benchmark artifact crate-name reference to `nodalmerge-core`.

Post-Wave-B residual source-level operational hits (excluding docs/wrappers/target artifacts): `0`.

## Classification Legend

- `intentional-compatibility`: keep for compatibility, protocol stability, package identity bridge, or legacy alias support.
- `hard-cutover-candidate`: not required for compatibility; can be renamed to `nodalmerge-*`.

## Inventory

| File | Hits | Classification | Notes |
| --- | ---: | --- | --- |
| `Cargo.lock` | 12 | intentional-compatibility | Reflects legacy compatibility wrapper crate package names (`activesync-*`). |
| `bridge/Cargo.toml` | 1 | intentional-compatibility | Bridge crate remains legacy package identity by design during migration window. |
| `dev-server/Cargo.toml` | 1 | intentional-compatibility | Legacy package identity retained for compatibility. |
| `server/Cargo.toml` | 1 | intentional-compatibility | Legacy package identity retained for compatibility. |
| `Dockerfile` | 1 | intentional-compatibility | Legacy runtime alias symlink (`activesync-server`) kept intentionally. |
| `pack-local-artifacts.ps1` | 4 | intentional-compatibility | Explicitly packs legacy npm crate IDs and references legacy bridge package name. |
| `run_task.ps1` | 3 | intentional-compatibility | Mongo container legacy alias fallback (`activesync-mongo`) is intentional. |
| `run_bench.ps1` | 3 | intentional-compatibility | Mongo container legacy alias fallback (`activesync-mongo`) is intentional. |
| `sdk-js/package.json` | 2 | intentional-compatibility | Legacy npm package identity and dependency maintained for compatibility release line. |
| `sdk-js/index.js` | 1 | intentional-compatibility | Imports legacy bridge package name intentionally. |
| `sdk-js/index.test.js` | 1 | intentional-compatibility | Test import mirrors intentional legacy package identity. |
| `web/demo.js` | 5 | intentional-compatibility | Legacy storage keys retained as fallback; one active DB name remains candidate (see below). |
| `web/smoke/demo-smoke.mjs` | 1 | hard-cutover-candidate | Smoke launcher can use `nodalmerge-server` binary directly. |
| `web/smoke/package.json` | 1 | hard-cutover-candidate | Package name can be renamed to `nodalmerge-demo-smoke`. |
| `web/smoke/package-lock.json` | 2 | hard-cutover-candidate | Lockfile package name mirrors `web/smoke/package.json`. |
| `nodalmerge-host/src/ActiveSync.Host.Composition/SqliteNodeStorageOptions.cs` | 1 | hard-cutover-candidate | Default sqlite DB path can be renamed to `data/nodalmerge-nodes.db`. |
| `server/src/store.rs` | 1 | hard-cutover-candidate | Temp test path prefix can be renamed safely. |
| `server/tests/blob_gc.rs` | 1 | hard-cutover-candidate | Temp test path prefix can be renamed safely. |
| `server/tests/idle_eviction.rs` | 1 | hard-cutover-candidate | Temp test path prefix can be renamed safely. |
| `server/tests/persistence.rs` | 1 | hard-cutover-candidate | Temp test path prefix can be renamed safely. |
| `s3-blobs/tests/minio_round_trip.rs` | 1 | hard-cutover-candidate | Test bucket fixture name can be renamed safely. |
| `jwt-bridge/src/lib.rs` | 1 | hard-cutover-candidate | Comment text can be updated to NodalMerge wording. |
| `benchmarks/results/text_trace_rustcode.bench.txt` | 1 | hard-cutover-candidate | Generated artifact text can be refreshed or left stale; safe to update when artifacts are regenerated. |
| `core/src/crypto.rs` | 2 | intentional-compatibility | Crypto KDF/info domain constants are wire-compatibility sensitive; changing would alter derivation outputs. |
| `core/src/ibf.rs` | 3 | intentional-compatibility | IBF hash key seeds are protocol compatibility constants. |
| `core/src/mst.rs` | 2 | intentional-compatibility | MST hash domain constants are protocol compatibility constants. |
| `core/src/token.rs` | 1 | intentional-compatibility | Token signing domain separator impacts token wire compatibility. |

## Remaining Post-Wave-B Inventory (Source-Level)

No remaining `activesync-` hits in source-level operational scope after exclusions.
