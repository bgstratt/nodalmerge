# NodalMerge Crate Identity Transition Plan

Owner: Rust Runtime + SDK Streams  
Status: InProgress  
Updated: 2026-05-25

## Objective

Move Rust crate consumption and publishing from legacy `activesync-*`
identities to `nodalmerge-*` identities without breaking existing downstreams.

## Strategy

1. Use `nodalmerge-*` as canonical implementation crate identities.
2. Keep `activesync-*` publishable via compatibility wrappers that re-export canonical crates.
3. Update docs/CI/artifact tooling to pack/publish both identities.
4. Migrate first-party examples/docs to nodalmerge-first imports.
5. Defer hard removal of legacy crate IDs until post-window cutover.

## Compatibility Window Rules

1. `nodalmerge-*` remains source-of-truth implementation crates.
2. `activesync-*` wrappers expose the same API surface via re-export.
3. CI must package and publish wrappers in the same pipeline as legacy crates.
4. Tooling must explicitly note legacy fallback behavior.

## Initial Wrapper Pass (Completed)

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

## Dedicated Crate-ID Migration Wave (Completed in this wave)

Implementation crates renamed to canonical `nodalmerge-*` package IDs:

1. `activesync-core` -> `nodalmerge-core`
2. `activesync-gc` -> `nodalmerge-gc`
3. `activesync-host-core` -> `nodalmerge-host-core`
4. `activesync-host-ffi` -> `nodalmerge-host-ffi`
5. `activesync-host-axum` -> `nodalmerge-host-axum`
6. `activesync-server` -> `nodalmerge-server`
7. `activesync-dev-server` -> `nodalmerge-dev-server`
8. `activesync-jwt-bridge` -> `nodalmerge-jwt-bridge`
9. `activesync-s3-blobs` -> `nodalmerge-s3-blobs`
10. `activesync-mongo-store` -> `nodalmerge-mongo-store`
11. `activesync-postgres-store` -> `nodalmerge-postgres-store`
12. `activesync-nodestore-conformance` -> `nodalmerge-nodestore-conformance`

Compatibility wrappers inverted to legacy `activesync-*` package IDs that re-export canonical crates:

1. `activesync-core` -> `nodalmerge-core`
2. `activesync-gc` -> `nodalmerge-gc`
3. `activesync-host-core` -> `nodalmerge-host-core`
4. `activesync-host-ffi` -> `nodalmerge-host-ffi`
5. `activesync-host-axum` -> `nodalmerge-host-axum`
6. `activesync-server` -> `nodalmerge-server`
7. `activesync-jwt-bridge` -> `nodalmerge-jwt-bridge`
8. `activesync-s3-blobs` -> `nodalmerge-s3-blobs`
9. `activesync-mongo-store` -> `nodalmerge-mongo-store`
10. `activesync-postgres-store` -> `nodalmerge-postgres-store`
11. `activesync-nodestore-conformance` -> `nodalmerge-nodestore-conformance`

Validation evidence:

1. `cargo check --workspace` succeeded.
2. `cargo test --workspace --no-run -j 1` succeeded.
3. `Cargo.lock` regenerated with canonical + compatibility package graph.

## Next Pass Candidates

1. Sweep docs/runbooks from `cargo -p activesync-*` to nodalmerge-first commands with explicit compatibility notes.
2. Decide bridge Rust crate strategy (`activesync-bridge` remains cdylib-only exception).
3. Add CI gates that fail new `cargo -p activesync-*` usage outside compatibility wrappers/docs.
