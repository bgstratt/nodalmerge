# Repository Restructure & Host-Parity Plan

Status: **mechanical phase (M1–M5) complete** on branch `restructure-m1`;
**S1 (command registry) complete** on branch `s1-command-registry`
(2026-07-05): `engine/commands/` crate + `registry.json` are live, both
parity tests are registry-driven, `ws_command_name` gives compile-time
exhaustiveness over `HostCommand`.
**S2 (ed25519 auth via FFI) complete** on branch `s2-roomtoken-ffi`
(2026-07-05): `nm_room_token_mint_json`/`nm_room_token_validate_json` in
host-ffi expose `nodalmerge_core::RoomToken` (no jwt-bridge dependency
needed — mint/verify live in core); new config-gated
`Auth:Provider = "RoomTokenEmbedded"` provider in Host.Composition with its
own resolver (NODALMERGE_HOST_FFI_DLL override + default NuGet probing).
JwtBridgeEmbedded (HS256) unchanged as the non-native-RID fallback.
Interop proven both directions in engine/host-ffi/tests/room_token_ffi.rs.
**S3 (de-stub or de-scope) complete** on branch `s2-roomtoken-ffi`
(2026-07-05): root stub was host-core's hashing — `deterministic_hex64` was
DefaultHasher (now blake3 via `Hash::of`) and checkpoint hashes were labels
over room-id+seq (now content-derived via `content_canonical_hash`, same
`nodalmerge_core::canonical_hash` the server uses). Topology promotion trio
de-stubbed: validate enforces child-checkpoint linkage (new
`PromotionValidationRejected` event, `reject.promotion_checkpoint_not_found`),
apply materializes the child snapshot into the parent with a real resulting
hash. Archive family formally deferred via new registry `scope: deferred`
field (import's reported hash is at least honest now).
**S4 (server→engine convergence) complete** on branch `s4-graph-routes`
(2026-07-05): the four `graph.*` commands are routed on nodalmerge-server,
gated with `query.admin`, with semantics extracted into shared
`nodalmerge_host_core::engine::graph_*` functions that both `HostEngine`
(FFI/.NET) and the server's `graph_query.rs` call — one brain, two hosts.
Envelopes match the .NET event mapping. `checkpoint.promote` stays
rust_server-absent and is marked `scope: deferred` in the registry: the
server has no Canonical Checkpoint plane, and adding one is an
architectural decision, not route wiring.
**S5 (persistence schema) complete** on branch `s5-store-schema`
(2026-07-05): canonical schema is the .NET `accepted_nodes` shape plus the
Rust idempotency win — deterministic compound `_id` via `$setOnInsert`, so
legacy ObjectId docs stay valid and no migration is needed. Contract in
docs/PERSISTENCE_SCHEMA.md. Rust store rewritten to it (seq counter
retired, multi-node pack hydration, legacy `bytes` fallback); .NET write
switched from replaceOne to $set/$setOnInsert upsert (read path untouched).
Verified live against a real Mongo container: full conformance suite +
cross-runtime interop assertions both directions.
This closes the structural phase S1–S5.
M5 deviation: root-level ps1 entry points stayed at root (they derive
repoRoot from their own location; relocation was churn without gain).
Known pre-existing issue surfaced during M1 verification: `nodalmerge-server`
lib test `archive_profile_002_object_manifest_parity_reports_p50_p95` races
on `std::env::set_var(NODALMERGE_ARCHIVE_OBJECT_ROOT)` under parallel test
threads — fails in full-suite runs, passes isolated, identical pre-move.
Owner: Bradley / Claude pairing sessions
Prereq reading: `benchmarks/results/runtime-attribution-dotnet-vs-rust-engine-ffi.md`,
`server/tests/control_plane_capability_parity.rs` (and its .NET mirror test).

---

## 1. Why

Findings from the 2026-07-05 audit session:

1. The repo has ~20 top-level directories with no visible layering; the
   architectural roles (shared engine vs. hosts vs. clients vs. compat shims)
   are not discoverable from structure.
2. There are **two** engines, not one with two hosts:
   - `server/` — the mature, persistent, full-featured Rust stack
     (`Room`/`ws_handler`/archive/topology/query). `host-axum` and
     `dev-server` are thin shells around it, not separate implementations.
   - `host-core/` (`HostEngine`) — a separate, in-memory engine created in
     commit `66cf48a3` ("host extraction baseline") as the FFI boundary for
     the .NET host. The planned migration of the Rust server onto it never
     happened, so it drifted: several commands are non-functional stubs
     (archive describe/validate/import, topology promotion validation — see
     `KNOWN STUB` comments in `host-core/src/engine.rs`).
3. The .NET host reimplements the WS transport/room layer from scratch
   (`RuntimeProtocolMapper`, `RuntimeRoomBroker`) — parallel code to
   `ws_handler.rs`, guarded today only by the control-plane capability
   parity tests.
4. Auth drift: the .NET "JwtBridgeEmbedded" mode ported the jwt-bridge
   *endpoints* (mint/validate contract) but not its *crypto* — it mints and
   validates HS256 JWTs, not ed25519-signed `RoomToken`s. Embedded-mode
   credentials are not interoperable with the Rust server. Sidecar mode is
   interoperable (when pointed at a real jwt-bridge).
5. Persistence drift: the Rust mongo store (`node-stores/mongo`) and the
   .NET `MongoNodeStoreProvider` use different collections, `_id` schemes,
   and field sets. They cannot share a database. Not by design — by
   accident.

## 2. Goals / non-goals

Goals:

- Restructure the repo so layering is visible and future hosts have an
  obvious home.
- Establish one canonical command-surface registry that all hosts are
  tested against, replacing hand-maintained twin tables.
- Eliminate the auth crypto drift by sharing the Rust implementation via
  FFI instead of reimplementing it in C#.
- Make every stub either real or formally out-of-scope — no more silent
  fake-success.

Non-goals:

- Rewriting consuming services (nodalmerge-studio, demo host). See §3.
- Migrating `nodalmerge-server`'s hot sync path onto `host-core`. That is a
  separate, performance-gated decision for later (baseline:
  `runtime-attribution-dotnet-vs-rust-engine-ffi.md`).
- Renaming published packages (crate names, NuGet IDs, npm names all stay).

## 3. Compatibility contract (answer to "does this break Studio?")

Verified consumer surface (grepped nodalmerge-studio 2026-07-05):

- Extension methods: `AddNodalMergeHostProviders`, `AddNodalMergeRuntimeCore`,
  `MapNodalMergeEndpoints`.
- Direct type use from `NodalMerge.DotNetHost.Ffi` and
  `NodalMerge.DotNetHost.Runtime` (RuntimeCausalGraphService,
  RuntimeGraphPromoter, RuntimeRoomEventBroadcaster, StudioParticipantService).
- Implements `NodalMerge.Host.Abstractions.Providers` interfaces
  (`INodeStoreProvider` via `NodalMergeStudioNodeStore`, `IBlobStoreProvider`).

**Frozen for the duration of this plan** (changes here = major version + explicit consumer migration):

- NuGet package IDs and the public namespaces above.
- Signatures of the three extension methods.
- The `Abstractions.Providers` interfaces (Studio implements them).
- The WS wire protocol message shapes.

**Free to change** (invisible to consumers):

- All repo-internal paths (the entire mechanical phase).
- `host-core` internals, including de-stubbing (fake results becoming real
  results is behaviorally visible but strictly corrective).
- New commands/events (additive).

**One flagged exception — embedded auth (S2):** unifying embedded auth onto
ed25519 `RoomToken` changes the token *format* minted/validated by
`JwtBridgeEmbedded` mode. API shape (`IRoomTokenAuthProvider`) is unchanged;
in-flight tokens are invalidated at upgrade (they're ≤1h TTL). Ship it as a
new config value (`Auth:Provider = "RoomTokenEmbedded"`) with
`JwtBridgeEmbedded` kept working-as-today, so consumers opt in per config,
not per code change. Studio adjusts one appsettings value when ready.

Net: **mechanical phase is zero-impact; structural phase is additive except
S2, which is config-gated.** No consumer rewrite at any point.

## 4. Target layout

```
nodalmerge/
├── Cargo.toml, Cargo.lock          # single workspace stays at root (one target/, one lockfile)
├── core/
│   ├── crdt/                       # nodalmerge-core   (today: core/)
│   └── gc/                         # nodalmerge-gc     (today: gc/)
├── engine/                         # the shared embeddable engine — the parity target
│   ├── host-core/                  # (today: host-core/)
│   ├── host-ffi/                   # (today: host-ffi/)
│   └── commands/                   # NEW in S1 — canonical command-surface registry
├── server/                         # reference Rust host + its adapters
│   ├── server/                     # nodalmerge-server (today: server/)
│   ├── axum-embed/                 # (today: host-axum/)
│   ├── dev-server/                 # (today: dev-server/)
│   ├── jwt-bridge/                 # (today: jwt-bridge/)
│   ├── s3-blobs/                   # (today: s3-blobs/)
│   └── stores/                     # (today: node-stores/{mongo,postgres,conformance})
├── peer/
│   ├── runtime-local/              # (today: runtime-local/)
│   ├── runtime-local-ffi/          # (today: runtime-local-ffi/)
│   ├── headless/                   # (today: headless/)
│   └── cli/                        # (today: cli/)
├── hosts/
│   └── dotnet/                     # (today: nodalmerge-host/) — future hosts are siblings
├── clients/
│   ├── bridge-wasm/                # (today: bridge/) — Rust crate, JS artifact
│   ├── sdk-js/                     # (today: sdk-js/)
│   └── web/                        # (today: web/)
├── compat/                         # rename shims, delete wholesale when window closes
│   ├── rust/                       # (today: wrappers/nodalmerge-*)
│   └── npm/                        # (today: wrappers/npm/*)
└── benchmarks/  docs/  scripts/  .github/
```

Notes:

- `sdk-js`/`web` sit on the **wasm bridge**, not the FFI crates. FFI crates
  live next to what they wrap (`engine/`, `peer/`).
- Directory names ≠ crate names. Crate names are unchanged; only
  `Cargo.toml` `members` and `path =` deps update.
- npm compat wrappers were deleted with `compat/` in the 2026-07-05
  activesync purge; `clients/sdk-js` and `clients/bridge-wasm/pkg` are the
  only published npm packages.

## 5. Mechanical migration (phases M1–M5)

Rules for every phase: `git mv` only (preserve history); one PR per phase;
CI fully green + local verification before starting the next. No code
changes mixed in beyond path literals.

### M1 — Rust crates

1. `git mv` per the map in §4 (core, gc, host-core, host-ffi, server,
   host-axum, dev-server, jwt-bridge, s3-blobs, node-stores, runtime-local,
   runtime-local-ffi, headless, cli, bridge, wrappers).
2. Update root `Cargo.toml` `members` (~30 entries).
3. Update every `path = "..."` dependency in every moved crate's
   `Cargo.toml` (grep for `path = "`).
4. Dockerfile: only the `cargo build -p nodalmerge-server` output path
   matters (post-2026-07-05 fix it does `COPY . .` — no manifest list to
   maintain).

Verify: `cargo build --workspace`, `cargo test --workspace` (accepting the
pre-existing failures noted in memory), `docker build .`, parity test.

### M2 — .NET

1. `git mv nodalmerge-host hosts/dotnet`.
2. Fix relative paths in: `NodalMerge.DotNetHost.Native.{win,linux}-x64.csproj`
   (`..\..\..\target\release\` → depth change), `.slnx`, NUGET_README
   includes, test project references.

Verify: `dotnet build` slnx, `dotnet test` DotNetHost.Tests, local
`dotnet pack` per `nodalmerge_local_dev_flow` (memory note: native DLL
resolver expects `NODALMERGE_HOST_FFI_DLL` or packaged runtimes/).

### M3 — JS

1. `git mv sdk-js clients/sdk-js`, `git mv web clients/web`,
   npm wrappers → `compat/npm/`.
2. Fix `clients/sdk-js/scripts/gen-doc-module.mjs` (reads `web/sdk.js`),
   `.gitignore` entries (`sdk-js/doc.js`, `sdk-js/*.tgz`, wrapper tgz
   globs), benchmark harness imports
   (`benchmarks/Run-SdkScenarioBenchmarks.mjs` imports `../web/...`).

Verify: `npm pack --dry-run` in each package, run the benchmark harness
smoke (2-op) if servers available.

### M4 — CI + scripts sweep

1. All `.github/workflows/*.yml`: `paths:` filters and step paths
   (six workflows; `control-plane-capability-parity.yml` watches four
   specific file paths — update them).
2. `pack-local-artifacts.ps1`, `expand-local-crates.ps1`, `run_bench.ps1`,
   `run_task.ps1`, `run_nightly_local.ps1`, `.dockerignore`.
3. Exit check: `git grep` for each old top-level dir name
   (`host-core|host-axum|node-stores|nodalmerge-host/|sdk-js/|wrappers/`)
   expecting only intentional hits (docs history, this plan).

Verify: dispatch each workflow once (`gh workflow run ...`) or push to a
branch with all paths touched.

### M5 — root cleanup

`server.key`, `server.log`, `tmp/` → gitignore/delete; loose `*.ps1` →
`scripts/`. Delete `server/src/bin/authz_conformance_runner.rs` (dead since
the workflow replacement) unless repurposed by then.

## 6. Structural phase (S1–S5)

Ordered so each step permanently reduces drift.

### S1 — canonical command-surface registry (`engine/commands/`)

One data source (Rust const table exported by a small crate, or TOML read
at test time) with one row per command:

| field | example |
|---|---|
| name | `archive.import` |
| required_capability | `archive.admin` |
| request/response shape ref | link to protocol.md section |
| status.rust-server | `real` |
| status.host-core | `stub` |
| status.dotnet | `stub-via-ffi` |

Consumers of the registry:
- `server/tests/control_plane_capability_parity.rs` — asserts Rust behavior
  matches registry rows (replaces its inline TABLE).
- .NET `ControlPlaneCapabilityParityTests` — asserts .NET behavior matches
  registry rows (embed the registry file as a test asset or generate the
  TheoryData from it in a build step).
- A registry-completeness test: every `HostCommand` variant and every
  `ws_handler` route must have a row; unknown = CI failure.

The `KNOWN STUB` comment blocks in `host-core/src/engine.rs` become
`status = stub` rows — machine-readable, not prose.

### S2 — auth unification via FFI (fixes the ed25519 gap)

- Expose `jwt-bridge` mint/validate through FFI (extend `host-ffi` or add a
  tiny `jwt-bridge-ffi` crate; package into the existing Native.* NuGets).
- Add a `RoomTokenEmbedded` auth provider in `Host.Composition` that
  P/Invokes it — real ed25519 `RoomToken`s, one implementation for all hosts.
- Keep `JwtBridgeEmbedded` (HS256) working unchanged; document it as
  .NET-local-only, non-interoperable with Rust server.
- Consumer impact: config opt-in only (§3).

### S3 — de-stub or de-scope, per registry

For each `stub` row decide: implement for real in `host-core`
(Rust-to-Rust port from `server/`), or mark `out-of-scope` in the registry
with rationale. Current stub inventory: `DescribeArchive`,
`ValidateArchive`, `ImportArchive` (+ no `ExportArchive` at all),
`ProposeTopologyPromotion` digest, `ValidateTopologyPromotion`,
`ApplyTopologyPromotion` hash. Prior decision 2026-07-05: archive family is
**deferred** (operator backup/restore feature; Studio doesn't need it) —
i.e. starts as `out-of-scope` rows, revisit later.

### S4 — converge server onto engine where it pays

Wire Rust WS routes through `HostEngine` for the 5 commands where
host-core is already real and the server has no route:
`checkpoint.promote`, `graph.get-frontier`, `graph.get-causal-parents`,
`graph.get-canonical-resolution`, `graph.compute-sync-diff`. This closes
the 5-of-the-5v2 asymmetry additively and makes the engine genuinely shared
for new surface area without touching the mature sync hot path. The 2
Rust-only commands (`archive.export`, `replay.read-range`) stay Rust-only
per S3 unless/until prioritized (replay.read-range is the cheaper port —
pure read, best done as a new `HostCommand`).

### S5 — persistence schema decision

Pick one canonical node-store schema. The .NET `accepted_nodes` shape is
richer (tombstones, compaction eligibility, payload_kind); the Rust mongo
store has compound `_id` + seq ordering. Either converge the Rust store to
the canonical shape (or vice versa) or record non-interop as deliberate in
the registry. Blocking question first: is cross-runtime store interop a
real requirement or a nice-to-have? (Open decision, §7.)

## 7. Open decisions

1. ~~npm wrapper direction~~ Resolved 2026-07-05: `compat/` (Rust re-export
   crates and both npm wrappers) deleted outright in the activesync purge.
2. ~~Is cross-runtime persistence interop actually required?~~ Decided
   2026-07-05: yes in direction — the schemas diverged inadvertently while
   standing up the .NET host for SpeechSlate/Studio/demos; converge to one
   canonical schema unless a concrete reason to differ emerges. Which
   schema wins (richer .NET `accepted_nodes` vs Rust compound-`_id`+seq)
   is the remaining S5 design question.
3. ~~When does the activesync compat window close?~~ Closed 2026-07-05:
   `compat/` deleted, env vars renamed, `as_*` C ABI renamed to `nm_*`,
   dual/legacy metric emission removed in both the .NET host and
   `nodalmerge-server`, docs swept.
4. Whether `nodalmerge-server`'s core sync path ever migrates onto
   `host-core` — revisit only after S4, with benchmarks.

## 8. Post-S5 gap inventory (2026-07-05 audit follow-up)

Deferred items live in the registry (`scope: deferred`) and §6. Additional
gaps found after S5, with disposition:

| Gap | Status |
|---|---|
| .NET advertised "Postgres" node provider with no implementation behind it | **Fixed** (quick-wins): removed from SupportedNodeProviders until a real provider exists |
| Rust postgres store (`server/stores/postgres`) still on pre-canonical `nodalmerge_nodes(seq, bytes)` schema | Open — converge alongside building the .NET Postgres provider (both sides land on docs/PERSISTENCE_SCHEMA.md together). Same audit owed to SQLite (Rust `DirPersistence` vs .NET `SqliteNodeStoreProvider`) though machine-local sharing is a weaker requirement |
| `PromotionValidationRejected` (S3) unmapped in .NET protocol layer — clients got silence on rejection | **Fixed** (quick-wins): mapped to `topology.validate-promotion.rejected` |
| `replay.read-range` Rust-only | **Fixed** (quick-wins): shared brain `graph_replay_read_range` in host-core; HostCommand + .NET route; registry flipped to real on all surfaces |
| CAPCOMP dual implementation (Rust `capability_profile.rs` vs .NET `CapabilityProfileExpander`) with no parity coverage | **Fixed** (2026-07-06, `docs/CAPCOMP_PARITY_PLAN.md`): consolidated the 3 implementations (server + stale jwt-bridge fork) into shared crate `server/capability-profile`; aligned .NET (ASCII-only fold, UTF-8 payload bytes, additive error-class overload, per-field `limits` defaults matching .NET's); locked with 33 vectors in `engine/commands/capcomp-vectors.v1.json` + harnesses in both runtimes; CI extended. Verified via 2 rounds of mutation checks + live end-to-end mint on both runtimes |
| No JS/SDK-side parity coverage since Check-SdkRejectionParity died with the doc cleanup | Open — lower stakes (clients, not authorities); scope after CAPCOMP |
| Blob storage layout parity unaudited (Rust file/S3 layout vs .NET FileBlobStoreProvider/S3Delegated) | Open — audit-sized, not build-sized; needed before any cross-runtime blob sharing claim |
| `activesync-*` naming still live (compat/ crates, npm direction inconsistency, legacy metric meters, NODALMERGE_* env vars, ARCHITECTURE.md) | **Fixed** (activesync-purge, 2026-07-05): compat/ deleted; env vars renamed; `as_*` ABI → `nm_*`; legacy dual meters removed (.NET host + Rust server were double-counting after a blanket rename); docs/ARCHITECTURE.md rewritten |
| Consumer validation (Studio/demos on repacked NuGets) | Open — gates merge to main |

## 9. Session-verified facts this plan relies on

- Studio pins NuGet 0.1.4; consumer surface as listed in §3.
- PWASoundboard.Api references the *old sibling activesync checkout* by
  project reference — unaffected by anything in this repo.
- Dockerfile is `COPY . .` + cache mounts (2026-07-05 fix) — resilient to
  crate moves.
- Parity tests exist and pass on both sides (Rust 1, .NET 23 cases).
- `authz-conformance-nightly` workflow replaced by
  `control-plane-capability-parity.yml`; old runner binary is dead code.
