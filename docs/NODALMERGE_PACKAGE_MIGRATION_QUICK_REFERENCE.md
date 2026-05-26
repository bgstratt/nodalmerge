# NodalMerge Package Migration Quick Reference

Status: Active migration guide
Updated: 2026-05-25

Use this map when moving from ActiveSync package/crate names to NodalMerge-first names.

## npm Packages

| Legacy name | NodalMerge-first name | Current compatibility status |
|---|---|---|
| `activesync-bridge` | `nodalmerge-bridge` | `nodalmerge-bridge` re-exports `activesync-bridge` during migration window |
| `activesync-sdk-js` | `nodalmerge-sdk-js` | `nodalmerge-sdk-js` re-exports `activesync-sdk-js` during migration window |

Install (preferred):

```bash
npm install nodalmerge-bridge nodalmerge-sdk-js
```

Compatibility fallback (still supported during migration window):

```bash
npm install activesync-bridge activesync-sdk-js
```

## Rust Crates

| Legacy crate | NodalMerge-first wrapper crate | Current compatibility status |
|---|---|---|
| `activesync-core` | `nodalmerge-core` | `nodalmerge-core` re-exports `activesync_core::*` |
| `activesync-gc` | `nodalmerge-gc` | `nodalmerge-gc` re-exports `activesync_gc::*` |
| `activesync-host-core` | `nodalmerge-host-core` | `nodalmerge-host-core` re-exports `activesync_host_core::*` |
| `activesync-host-axum` | `nodalmerge-host-axum` | `nodalmerge-host-axum` re-exports `activesync_host_axum::*` |
| `activesync-host-ffi` | `nodalmerge-host-ffi` | `nodalmerge-host-ffi` re-exports `activesync_host_ffi::*` |
| `activesync-server` | `nodalmerge-server` | `nodalmerge-server` re-exports `activesync_server::*` |
| `activesync-jwt-bridge` | `nodalmerge-jwt-bridge` | `nodalmerge-jwt-bridge` re-exports `activesync_jwt_bridge::*` |
| `activesync-s3-blobs` | `nodalmerge-s3-blobs` | `nodalmerge-s3-blobs` re-exports `activesync_s3_blobs::*` |
| `activesync-mongo-store` | `nodalmerge-mongo-store` | internal wrapper (publish=false) for migration/test usage |
| `activesync-postgres-store` | `nodalmerge-postgres-store` | internal wrapper (publish=false) for migration/test usage |
| `activesync-nodestore-conformance` | `nodalmerge-nodestore-conformance` | internal wrapper (publish=false) for migration/test usage |

Recommended dependency shape for new integrations:

```toml
[dependencies]
nodalmerge-core = "0.1.0"
nodalmerge-gc = "0.1.0"
nodalmerge-host-core = "0.1.0"
nodalmerge-host-axum = "0.1.0"
nodalmerge-host-ffi = "0.1.0"
nodalmerge-server = "0.1.0"
nodalmerge-jwt-bridge = "0.1.0"
nodalmerge-s3-blobs = "0.1.0"
```

Compatibility fallback shape:

```toml
[dependencies]
activesync-core = "0.1.0"
activesync-gc = "0.1.0"
activesync-host-core = "0.1.0"
activesync-host-axum = "0.1.0"
activesync-host-ffi = "0.1.0"
activesync-server = "0.1.0"
activesync-jwt-bridge = "0.1.0"
activesync-s3-blobs = "0.1.0"
```

## NuGet Packages

| Legacy package | NodalMerge-first wrapper package | Current compatibility status |
|---|---|---|
| `ActiveSync.Host.Abstractions` | `NodalMerge.Host.Abstractions` | Wrapper package depends on legacy package during migration window |
| `ActiveSync.Host.Composition` | `NodalMerge.Host.Composition` | Wrapper package depends on legacy package during migration window |
| `ActiveSync.DotNetHost.Native.win-x64` | `NodalMerge.DotNetHost.Native.win-x64` | Wrapper package depends on legacy package during migration window |
| `ActiveSync.DotNetHost.Native.linux-x64` | `NodalMerge.DotNetHost.Native.linux-x64` | Wrapper package depends on legacy package during migration window |

## Rust Binaries

| Legacy command | NodalMerge-first command | Current compatibility status |
|---|---|---|
| `cargo run -p activesync-server --bin activesync-server` | `cargo run -p activesync-server --bin nodalmerge-server` | Both bins are available |
| `cargo run -p activesync-dev-server --bin activesync-dev-server` | `cargo run -p activesync-dev-server --bin nodalmerge-dev-server` | Both bins are available |

## CI/Release Workflow Coverage

The pipeline in `.github/workflows/nuget-build-push.yml` now includes:

1. npm wrapper dry-run packaging (`nodalmerge-bridge`, `nodalmerge-sdk-js`)
2. crate wrapper dry-run packaging (`nodalmerge-core`, `nodalmerge-gc`, `nodalmerge-host-core`, `nodalmerge-host-axum`, `nodalmerge-host-ffi`, `nodalmerge-server`, `nodalmerge-jwt-bridge`, `nodalmerge-s3-blobs`)
3. wrapper smoke validation (`cargo check`) across nodalmerge wrappers in CI
4. NuGet wrapper packaging (`NodalMerge.Host.*` and `NodalMerge.DotNetHost.Native.*`) in the managed/native pack jobs
5. optional npm/crate wrapper publish jobs on workflow dispatch (gated by credentials)

Required workflow_dispatch inputs for optional publish jobs:

1. `npmPublishToken` for npm wrapper publish
2. `cargoRegistryToken` for crate wrapper publish
