# NodalMerge Crate Identity Transition Plan

Owner: Rust Runtime + SDK Streams  
Status: InProgress  
Updated: 2026-05-25

## Objective

Move Rust crate consumption and publishing from legacy `activesync-*`
identities to `nodalmerge-*` identities without breaking existing downstreams.

## Strategy

1. Keep all existing `activesync-*` crates publishable and supported.
2. Add additive `nodalmerge-*` wrapper crates that re-export legacy crates.
3. Update docs/CI/artifact tooling to pack/publish both identities.
4. Migrate first-party examples/docs to nodalmerge-first imports.
5. Defer hard renaming/removal of legacy crate IDs until post-window cutover.

## Compatibility Window Rules

1. `activesync-*` remains source-of-truth implementation crates.
2. `nodalmerge-*` wrappers expose the same API surface via re-export.
3. CI must package and publish wrappers in the same pipeline as legacy crates.
4. Tooling must explicitly note legacy fallback behavior.

## First Implementation Pass (Completed in this wave)

Existing wrappers retained:

1. `nodalmerge-core` -> `activesync-core`
2. `nodalmerge-host-core` -> `activesync-host-core`
3. `nodalmerge-host-ffi` -> `activesync-host-ffi`

New wrappers added:

1. `nodalmerge-server` -> `activesync-server`
2. `nodalmerge-gc` -> `activesync-gc`
3. `nodalmerge-jwt-bridge` -> `activesync-jwt-bridge`

Second-pass wrappers added:

1. `nodalmerge-host-axum` -> `activesync-host-axum`
2. `nodalmerge-s3-blobs` -> `activesync-s3-blobs`
3. `nodalmerge-mongo-store` -> `activesync-mongo-store` (internal, publish=false)
4. `nodalmerge-postgres-store` -> `activesync-postgres-store` (internal, publish=false)
5. `nodalmerge-nodestore-conformance` -> `activesync-nodestore-conformance` (internal, publish=false)

Note: Rust crate `activesync-bridge` is `cdylib`-only and not re-exportable as
a normal Rust wrapper crate. Bridge identity migration is covered via npm
wrapper packages (`nodalmerge-bridge`).

Operational wiring added:

1. Workspace membership in root `Cargo.toml`.
2. Local artifact pack support in `pack-local-artifacts.ps1`.
3. CI crate wrapper pack/publish support in `.github/workflows/nuget-build-push.yml`.
4. CI wrapper smoke validation via `cargo check -p nodalmerge-*` coverage in `.github/workflows/nuget-build-push.yml`.

## Next Pass Candidates

1. Add wrappers for remaining high-use crates (`gc`, `host-axum`, `s3-blobs`, `node-stores/*`).
2. Shift first-party integration docs to nodalmerge crate imports with explicit legacy snippets.
3. Add a focused wrapper smoke-test job (`cargo check -p nodalmerge-*`) for all wrappers.
