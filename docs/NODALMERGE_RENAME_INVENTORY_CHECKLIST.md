# NodalMerge Rename Inventory Checklist (Phase A)

Owner: Platform + packaging + runtime
Status: Active (Phase A started)
Last updated: 2026-05-26

Purpose: machine-checkable, owner-assigned inventory for ActiveSync -> NodalMerge rename execution.

## 1. Usage

Execution statuses:

1. NotStarted
2. InProgress
3. Blocked
4. Complete

Each row must have:

1. owner
2. scope path
3. rename class
4. compatibility requirement
5. migration window
6. status
7. verification evidence

## 2. Rename classes

1. BrandText: user-facing names in docs/messages
2. ArtifactName: crate/package/bin/image/project identifiers
3. NamespaceType: .NET namespaces and public type names
4. ConfigKey: config prefixes and key names
5. EnvVar: environment variable names
6. MetricName: metrics prefixes and labels
7. PathDefault: default files/dirs and state paths
8. ScriptRef: scripts/CI commands and references

## 3. Inventory matrix

| ID | Area | Scope path/glob | Rename class | Current example | Target example | Compatibility requirement | Migration window | Owner | Status | Verification evidence |
|---|---|---|---|---|---|---|---|---|---|---|
| RNM-001 | Rust crates | Cargo.toml + */Cargo.toml | ArtifactName | activesync-core | nodalmerge-core | Transitional aliases or dual-publish for external crates | Wave R through next full release | Rust Platform Stream | InProgress | Cargo manifests inventoried on 2026-05-25 |
| RNM-002 | Rust binaries | server/Cargo.toml, dev-server/Cargo.toml | ArtifactName | activesync-server | nodalmerge | Legacy command alias required | Wave R through next full release | Rust Runtime Stream | InProgress | Binary name surfaces identified in server and scripts |
| RNM-003 | FFI artifacts | host-ffi outputs, probes in scripts | ArtifactName | activesync_host_ffi.dll | nodalmerge_host_ffi.dll | Probe both names during transition | Wave R through next full release | Host Runtime Stream | InProgress | FFI dll references inventoried in benchmark and verify scripts |
| RNM-004 | JS package names | sdk-js/package.json, bridge pkg metadata | ArtifactName | activesync-sdk-js | nodalmerge-sdk-js | Deprecated wrapper package retained | Wave R through next full release | JS SDK Stream | InProgress | package.json naming and dependency surfaces identified |
| RNM-005 | JS bridge imports | web/sdk.js, web/sdk.d.ts, pkg paths | ScriptRef | ./pkg/activesync_bridge.js | ./pkg/nodalmerge_bridge.js | Compatibility import shim | Wave R through next full release | JS SDK Stream | InProgress | Bridge import paths identified in sdk.js and sdk.d.ts |
| RNM-006 | .NET project/package ids | nodalmerge-host/**/*.csproj | ArtifactName | ActiveSync.Host.Abstractions | NodalMerge.Host.Abstractions | Compatibility package id bridge | Wave R through next full release | DotNet Host Stream | InProgress | csproj PackageId and references inventoried |
| RNM-007 | .NET namespaces/types | nodalmerge-host/src/**/*.cs | NamespaceType | ActiveSync.* namespace | NodalMerge.* namespace | Namespace forwarding strategy | Wave R through next full release | DotNet Host Stream | InProgress | Namespace/usings migrated in host C# files on 2026-05-26; dotnet build and dotnet test succeeded |
| RNM-008 | .NET config prefixes | nodalmerge-host config binding and docs | ConfigKey | ActiveSync:* | NodalMerge:* | Dual-key read with precedence to NodalMerge | Wave R through next full release | DotNet Host Stream | InProgress | NodalMerge primary + ActiveSync fallback implemented in option/config loaders; dotnet build and dotnet test succeeded on 2026-05-26 |
| RNM-009 | Environment variables | scripts, runtime config, docs | EnvVar | ACTIVESYNC_* | NODALMERGE_* | Parse both during migration window | Wave R through next full release | Runtime + Ops Stream | InProgress | NODALMERGE_HOST_FFI_DLL primary with ACTIVESYNC_HOST_FFI_DLL fallback implemented in runtime resolver and verify script on 2026-05-26 |
| RNM-010 | Metric names | server and host metrics | MetricName | activesync_* | nodalmerge_* | Dashboard migration strategy and compatibility | Wave R through next full release | Observability Stream | InProgress | Dotnet-host and Rust server now dual-emit NodalMerge primary + ActiveSync compatibility metric names (2026-05-26) |
| RNM-011 | Docker image and entrypoint | Dockerfile, deployment docs | ArtifactName | activesync-server | nodalmerge | Dual tags and entrypoint alias | Wave R through next full release | DevOps Stream | InProgress | Dockerfile now defaults ENTRYPOINT to nodalmerge-server with activesync-server compatibility alias via symlink (2026-05-26) |
| RNM-012 | Container user/path defaults | Dockerfile, runtime defaults | PathDefault | user activesync | user nodalmerge | Legacy user/path compatibility where needed | Wave R through next full release | DevOps Stream | NotStarted | pending |
| RNM-013 | Data file defaults | sqlite/db and key path defaults | PathDefault | activesync.db, ~/.activesync | nodalmerge.db, ~/.nodalmerge | Legacy location autodetect + migration helper | Wave R through next full release | Runtime + Ops Stream | NotStarted | pending |
| RNM-014 | Bench/run scripts | *.ps1, benchmarks/**/*.ps1 | ScriptRef | cargo run -p activesync-server | cargo run -p nodalmerge-server | Legacy command compatibility wrappers | Wave R through next full release | Performance Tooling Stream | NotStarted | pending |
| RNM-015 | CI/release pipelines | workflow/release scripts | ScriptRef | activesync artifact ids | nodalmerge artifact ids | Dual publish until switch complete | Wave R through next full release | Build/Release Stream | NotStarted | pending |
| RNM-016 | Documentation titles/body | docs/**/*.md, README* | BrandText | ActiveSync | NodalMerge | Migration note and timeline section | Wave R through next full release | Docs Stream | NotStarted | pending |
| RNM-017 | Integration snippets | docs/integration.md, docs/deployment.md | ScriptRef | use activesync_server | use nodalmerge_server | Include compatibility examples for one cycle | Wave R through next full release | Docs + Rust Runtime Stream | NotStarted | pending |
| RNM-018 | Tactical showcase refs | activesync-tactical-showcase/**/* | BrandText | activesync refs | nodalmerge refs | Keep interop aliases until showcase migration done | Wave R through next full release | Showcase Stream | NotStarted | pending |

## 4. Compatibility commitments (must not regress)

1. Existing docker runbooks continue to work with old image/entrypoint naming during migration window.
2. Existing NuGet consumers can upgrade without immediate package id rename breakage.
3. Existing npm consumers can continue via deprecated wrapper package.
4. Existing environment variable and config key names continue to function with clear precedence rules.
5. Existing benchmark and acceptance scripts continue to run with compatibility aliases.

## 5. Phase A exit criteria checklist

Mark each as Complete only with linked evidence:

1. inventory rows RNM-001 through RNM-018 assigned to owners
2. all rows classified with compatibility requirement and migration window
3. blocker list resolved or accepted with owner sign-off
4. CI rename lint/check added to prevent new hardcoded ActiveSync identifiers in new code
5. phase gate review recorded in roadmap update

## 6. Verification log template

Use this format per completed row:

1. Row ID:
2. Date:
3. Owner:
4. Change summary:
5. Validation commands run:
6. Result:
7. Follow-ups:

## 7. Batch 2 implementation steps (RNM-007 to RNM-011)

### RNM-007 .NET namespaces and public types

Target files first pass:

1. nodalmerge-host/src/** (all C# source files)
2. nodalmerge-host/tests/** (namespace references)
3. nodalmerge-host/bench/** (namespace references)

Implementation steps:

1. Rename root namespace declarations from ActiveSync.* to NodalMerge.* in source files.
2. Add compatibility type-forwarding or shim namespaces where external consumers rely on prior namespaces.
3. Update internal using/import statements and test references.

Validation commands:

1. dotnet build nodalmerge-host/ActiveSync.DotNetHost.slnx
2. dotnet test nodalmerge-host/ActiveSync.DotNetHost.slnx

Execution evidence (2026-05-26):

1. Applied mechanical rename `ActiveSync.` -> `NodalMerge.` across dotnet-host C# files (79 files changed).
2. `dotnet build nodalmerge-host/ActiveSync.DotNetHost.slnx` succeeded.
3. `dotnet test nodalmerge-host/ActiveSync.DotNetHost.slnx` succeeded.

### RNM-008 .NET config key prefixes

Target files first pass:

1. nodalmerge-host/src/**/appsettings*.json
2. nodalmerge-host/src/** config binding classes and options mapping
3. nodalmerge-host/verify.ps1 and docs using ActiveSync:* keys

Implementation steps:

1. Introduce NodalMerge:* as primary binding path.
2. Keep ActiveSync:* as compatibility alias during migration window.
3. Enforce precedence: NodalMerge:* overrides ActiveSync:* when both are provided.

Validation commands:

1. dotnet run --project nodalmerge-host/src/ActiveSync.DotNetHost/ActiveSync.DotNetHost.csproj --no-launch-profile -- --NodalMerge:Providers:NodeStorage=InMemory
2. dotnet run --project nodalmerge-host/src/ActiveSync.DotNetHost/ActiveSync.DotNetHost.csproj --no-launch-profile -- --ActiveSync:Providers:NodeStorage=InMemory

Execution evidence (2026-05-26):

1. Added NodalMerge-first section binding with ActiveSync fallback across provider and auth/storage option classes.
2. Updated runtime config reads to prefer `NodalMerge:Runtime:*` with fallback to `ActiveSync:Runtime:*`.
3. Updated host debug provider key reads and startup/provider log messaging to NodalMerge naming.
4. `dotnet build nodalmerge-host/ActiveSync.DotNetHost.slnx` succeeded.
5. `dotnet test nodalmerge-host/ActiveSync.DotNetHost.slnx` succeeded.

### RNM-009 Environment variable migration

Target files first pass:

1. run_task.ps1, run_bench.ps1, run_nightly_local.ps1, run_bench.ps1
2. benchmarks/**/*.ps1
3. nodalmerge-host/*.ps1
4. runtime env parsing code paths

Implementation steps:

1. Add NODALMERGE_* variables as primary names.
2. Preserve ACTIVESYNC_* as fallback aliases.
3. Document precedence and migration examples in roadmap and deployment docs.

Validation commands:

1. Run benchmark/start scripts with only NODALMERGE_* values.
2. Re-run with only ACTIVESYNC_* values and verify behavior parity.

Execution evidence (2026-05-26):

1. Added `NODALMERGE_HOST_FFI_DLL` as primary runtime probe in native resolver.
2. Kept `ACTIVESYNC_HOST_FFI_DLL` fallback behavior when new variable is absent.
3. Updated `nodalmerge-host/verify.ps1` to set/use `NODALMERGE_HOST_FFI_DLL` first, then legacy fallback.
4. Updated benchmark/root scripts (`run_bench.ps1`, `run_task.ps1`, `benchmarks/Run-BenchmarkMatrix.ps1`, `benchmarks/Start-BenchmarkTargets.ps1`) to set `NODALMERGE_HOST_FFI_DLL` primary with `ACTIVESYNC_HOST_FFI_DLL` compatibility alias.
5. Updated server runtime env parsing in `server/src/ws_handler.rs` to read `NODALMERGE_SCOPE_MAX_FILTERED_CATCHUP_{NODES,BYTES}` and `NODALMERGE_CAPABILITY_PROFILE_PATH` with `ACTIVESYNC_*` fallback aliases.

### RNM-010 Metric prefix migration

Target files first pass:

1. server/src/** metrics emission points
2. host runtime metrics emission points
3. docs/deployment.md and other metrics reference docs

Implementation steps:

1. Define nodalmerge_* metric namespace as target.
2. Implement temporary compatibility approach (dual emit or translation layer).
3. Update dashboards and alert queries with migration window notes.

Validation commands:

1. Start runtime and scrape metrics endpoint; verify nodalmerge_* appears.
2. Confirm compatibility strategy still supports existing activesync_* queries during window.

Execution evidence (2026-05-26):

1. Added legacy ActiveSync meter-name dual emission alongside NodalMerge meter names in dotnet-host runtime and FFI metric publishers.
2. Updated metric publishers in runtime websocket, auth validation, control-plane deny, and DAG persistence/compaction paths.
3. Added nodalmerge_* primary metric registration and dual-emission compatibility in Rust server runtime (`room.rs`, `ws_handler.rs`, `store.rs`, `metrics.rs`) while retaining activesync_* compatibility names.
4. Updated Rust metrics endpoint integration assertions to require both nodalmerge_* and activesync_* baseline series.
5. `dotnet build nodalmerge-host/ActiveSync.DotNetHost.slnx` succeeded.
6. `dotnet test nodalmerge-host/ActiveSync.DotNetHost.slnx` succeeded.
7. Restored missing server bin target file `server/src/bin/authz_conformance_runner.rs` so workspace bin resolution succeeds.
8. `cargo test -p activesync-server --test metrics_endpoint` succeeded (3/3 passing).

### RNM-011 Docker image and entrypoint migration

Target files first pass:

1. Dockerfile
2. docs/deployment.md and integration docs with docker examples
3. scripts referencing activesync-server image name

Implementation steps:

1. Build and publish nodalmerge image/entrypoint as primary.
2. Keep activesync-server tag and command alias during migration window.
3. Update runbook examples to nodalmerge defaults.

Validation commands:

1. docker build -t nodalmerge .
2. docker run --rm -p 7878:7878 nodalmerge --help
3. docker run --rm -p 7878:7878 activesync-server --help (compatibility path)

Execution evidence (2026-05-26):

1. Updated Dockerfile docs/comments to use `nodalmerge-server` as primary image/entrypoint naming.
2. Kept compatibility by preserving `/usr/local/bin/activesync-server` and adding `/usr/local/bin/nodalmerge-server` symlink.
3. Switched container `ENTRYPOINT` to `nodalmerge-server`.
4. Fixed Docker workspace manifest-stub stage for current workspace members/bench targets so `docker build -t nodalmerge .` succeeds.
5. Validated both binary aliases in-container:
	- `docker run --rm --entrypoint sh nodalmerge -c "command -v nodalmerge-server; command -v activesync-server"`
	- `docker run --rm --entrypoint sh activesync-server -c "command -v nodalmerge-server; command -v activesync-server"`
