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
| `activesync-host-core` | `nodalmerge-host-core` | `nodalmerge-host-core` re-exports `activesync_host_core::*` |
| `activesync-host-ffi` | `nodalmerge-host-ffi` | `nodalmerge-host-ffi` re-exports `activesync_host_ffi::*` |

Recommended dependency shape for new integrations:

```toml
[dependencies]
nodalmerge-core = "0.1.0"
nodalmerge-host-core = "0.1.0"
nodalmerge-host-ffi = "0.1.0"
```

Compatibility fallback shape:

```toml
[dependencies]
activesync-core = "0.1.0"
activesync-host-core = "0.1.0"
activesync-host-ffi = "0.1.0"
```

## Rust Binaries

| Legacy command | NodalMerge-first command | Current compatibility status |
|---|---|---|
| `cargo run -p activesync-server --bin activesync-server` | `cargo run -p activesync-server --bin nodalmerge-server` | Both bins are available |
| `cargo run -p activesync-dev-server --bin activesync-dev-server` | `cargo run -p activesync-dev-server --bin nodalmerge-dev-server` | Both bins are available |

## CI/Release Workflow Coverage

The pipeline in `.github/workflows/nuget-build-push.yml` now includes:

1. npm wrapper dry-run packaging (`nodalmerge-bridge`, `nodalmerge-sdk-js`)
2. crate wrapper dry-run packaging (`nodalmerge-core`, `nodalmerge-host-core`, `nodalmerge-host-ffi`)
3. optional wrapper publish jobs on workflow dispatch (gated by credentials)

Required secrets for optional publish jobs:

1. `NPM_TOKEN` for npm wrapper publish
2. `CARGO_REGISTRY_TOKEN` for crate wrapper publish
