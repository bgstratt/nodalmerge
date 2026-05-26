# NodalMerge Rename Execution Plan

Owner: Platform + packaging + runtime
Status: Planned
Last updated: 2026-05-25

Phase A execution artifact:

1. `docs/NODALMERGE_RENAME_INVENTORY_CHECKLIST.md`

## 1. Objective

Rename product identity from ActiveSync to NodalMerge across the repository and published surfaces, with minimal consumer breakage.

Primary outcomes:

1. canonical product name becomes NodalMerge
2. primary CLI command becomes nodalmerge
3. all package and artifact names migrate on a controlled timeline
4. compatibility aliases preserve existing integrations during transition

## 2. Scope

In scope:

1. Rust crates, binary names, module references, and docs
2. .NET project names, namespaces, NuGet package ids, config keys
3. JavaScript package names, bridge import paths, docs
4. Docker image references, container users, entrypoints, runbooks
5. env vars, metrics prefixes, default data file names, key paths
6. scripts, CI pipelines, benchmark tooling, release automation

Out of scope:

1. wire protocol message type renaming unless required for branding only
2. behavior changes to replication, authority, or replay semantics

## 3. Rename policy

1. Runtime semantics must remain unchanged during rename.
2. Rename is additive then switching then cleanup, not a flag day.
3. Old names remain supported via alias windows for at least one full release cycle.
4. Every external surface gets an explicit migration path and rollback note.

## 4. Surface inventory and target mapping

## 4.1 Product and repo identity

1. Display name: ActiveSync -> NodalMerge
2. Repository folder and docs title references
3. README badges and examples

## 4.2 Rust workspace and binaries

Current patterns include:

1. activesync-core
2. activesync-server
3. activesync-host-core
4. activesync-host-ffi
5. activesync-bridge
6. activesync-gc
7. activesync-s3-blobs
8. activesync-mongo-store
9. activesync-postgres-store
10. activesync-dev-server

Targets:

1. nodalmerge-core
2. nodalmerge-server
3. nodalmerge-host-core
4. nodalmerge-host-ffi
5. nodalmerge-bridge
6. nodalmerge-gc
7. nodalmerge-s3-blobs
8. nodalmerge-mongo-store
9. nodalmerge-postgres-store
10. nodalmerge-dev-server

CLI target:

1. nodalmerge (primary)
2. activesync-server retained as compatibility alias during migration window

## 4.3 .NET and NuGet

Current patterns include ActiveSync.* package ids, csproj names, solution paths, and config keys such as ActiveSync:*.

Targets:

1. PackageId and assembly naming prefix NodalMerge.*
2. config prefix NodalMerge:*
3. env and launch profile docs updated to NODALMERGE and NodalMerge forms
4. old package ids and config keys maintained as compatibility aliases

## 4.4 JavaScript and WASM bridge

Current patterns include:

1. npm package activesync-sdk-js
2. bridge import ./pkg/activesync_bridge.js
3. docs and logger tags [activesync]

Targets:

1. npm package nodalmerge-sdk-js
2. bridge package/artifact nodalmerge-bridge
3. updated import path and logger branding
4. compatibility package and import shim window

## 4.5 Docker and container runtime

Current patterns include:

1. image tags activesync-server
2. binary path /usr/local/bin/activesync-server
3. container user activesync

Targets:

1. image tags nodalmerge
2. binary/entrypoint nodalmerge
3. container user nodalmerge
4. compatibility tags for old image names during transition window

## 4.6 Operational identifiers

Current patterns include ACTIVESYNC_* env vars and activesync_* metric names, plus data paths such as ~/.activesync and activesync.db.

Targets:

1. NODALMERGE_* env vars with ACTIVESYNC_* aliases
2. nodalmerge_* metrics with compatibility policy for existing dashboards
3. ~/.nodalmerge and nodalmerge.db defaults, with backward-compatible detection of legacy locations

## 5. Phased rollout

### Phase A: Inventory freeze and guardrails

Deliverables:

1. complete rename inventory per directory/surface
2. compatibility policy matrix with owner per surface
3. CI guardrails to prevent new hardcoded ActiveSync names where not allowed

Required artifact updates:

1. Update status/evidence for rows `RNM-001` to `RNM-018` in `docs/NODALMERGE_RENAME_INVENTORY_CHECKLIST.md`.

Exit criteria:

1. all external surfaces are classified as internal-only, external-compatible, or breaking
2. migration windows approved

### Phase B: Additive rename and dual-publish

Deliverables:

1. nodalmerge crate/package/bin names introduced
2. old names retained as aliases/shims/wrappers
3. dual docs and migration notes published

Exit criteria:

1. existing consumers continue working without required immediate change
2. nodalmerge paths validated end-to-end

### Phase C: Default switch

Deliverables:

1. docs, examples, templates, scripts default to nodalmerge names
2. release pipelines publish nodalmerge artifacts as primary
3. warnings emitted when legacy names are used

Exit criteria:

1. fresh install path uses nodalmerge-only naming by default
2. compatibility shims remain available

### Phase D: Legacy cleanup

Deliverables:

1. remove legacy aliases after deprecation window
2. archive migration docs and finalize post-rename baseline

Exit criteria:

1. no runtime code path depends on ActiveSync naming
2. all supported integrations validated against nodalmerge names only

## 6. Compatibility matrix requirements

Minimum compatibility mechanisms to implement:

1. Rust:
   - legacy crate wrappers or transitional re-export crates
   - legacy binary alias command that delegates to nodalmerge
2. .NET:
   - compatibility NuGet package ids that depend on new ids
   - dual key read for ActiveSync:* and NodalMerge:* config prefixes
3. npm:
   - deprecated activesync-sdk-js wrapper that re-exports nodalmerge-sdk-js
4. Docker:
   - dual image tags and entrypoint alias
5. Ops:
   - dual env var parsing with precedence to NODALMERGE_*
   - metric rename strategy documented for dashboard migration

## 7. Risk register

1. Hidden string references in scripts/docs cause broken release automation.
   Mitigation: scripted scan and CI rename lint step.
2. External package consumers break on crate/package id changes.
   Mitigation: dual-publish with compatibility wrappers.
3. Dashboard/alerts break on metric prefix changes.
   Mitigation: phased metric migration and temporary dual emission or translation.
4. Data path rename causes startup failures.
   Mitigation: legacy path auto-detection and one-time migration helper.

## 8. Verification gates

Required checks before each phase promotion:

1. cargo workspace build/test using nodalmerge primary names
2. dotnet solution build/test with NodalMerge package/config names
3. npm package install/import smoke for nodalmerge-sdk-js and compatibility wrapper
4. docker build/run with nodalmerge image and CLI entrypoint
5. benchmark and ops scripts pass with NODALMERGE_* env vars
6. docs command snippets validated by smoke scripts

## 9. Detailed work checklist

1. Rust workspace manifests and dependency references
2. Rust bin names and cargo run scripts
3. FFI artifact names and probing logic
4. .NET csproj package ids, project names, namespaces
5. dotnet-host scripts and NuGet packing pipelines
6. npm package names and bridge import paths
7. Dockerfile, compose snippets, and deployment docs
8. env vars and config key bindings
9. metrics names, dashboards, and alert rules
10. file paths for key material and sqlite defaults
11. benchmark scripts and acceptance harnesses
12. tactical showcase integration references
13. documentation sweep and migration guide

## 10. Suggested execution order

1. complete inventory and compatibility matrix
2. implement CLI and binary alias strategy first
3. implement package-level dual publish strategy
4. switch docs and templates to nodalmerge defaults
5. execute compatibility soak period
6. remove legacy names in cleanup phase
