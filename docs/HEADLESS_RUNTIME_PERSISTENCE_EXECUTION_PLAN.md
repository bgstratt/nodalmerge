# Headless Runtime and Peer Persistence Execution Plan

Owner: Runtime + SDK + host streams
Status: ClosedForDeclaredSlice (Wave 2 depth + Phase C SDK + demo cutover + Phase E ops slice)
Last updated: 2026-05-27

## 1. Why this plan exists

NodalMerge currently has:

1. Browser SDK over WASM bridge with app-managed local persistence patterns (for example IndexedDB in demo code).
2. Server runtime persistence adapters (`NoPersistence`, `DirPersistence`, `S3BlobStore`).
3. Host-neutral runtime surfaces (`host-core`, `host-ffi`) that can power non-browser embeddings.

What is missing is a first-class, shared contract for peer-local persistence and a headless runtime entrypoint for pod/service workloads.

## 2. Scope and non-goals

In scope:

1. a pluggable peer-local persistence adapter contract
2. browser and non-browser adapter implementations
3. headless runtime module + CLI for pods/workers
4. config-driven backend selection (memory, file, embedded DB, browser DB)
5. parity vectors across SDK/browser and headless/runtime paths

Out of scope (v1):

1. replacing core DAG/engine semantics
2. replacing websocket protocol as default data plane
3. introducing product-specific business workflows

## 3. Design principles

1. Additive compatibility: existing `createDoc` and websocket flows continue to work unchanged.
2. Layering: persistence plugin is a module used by SDK/runtime, not a separate fork of engine logic.
3. Determinism: persistence backend choice must not change canonical DAG/replay behavior.
4. Capability parity: browser, node, and headless pod paths expose equivalent core operations.
5. Operational clarity: backend selection is explicit in config and observable in metrics/logs.

## 4. Proposed module boundaries (v1)

1. `nodalmerge-runtime-local` (conceptual crate name TBD): shared local persistence adapter contract and default implementations.
2. `nodalmerge-headless` (conceptual crate name TBD): runtime module that hosts engine + transport + adapter wiring.
3. `nodalmerge` CLI extension (or sibling CLI): headless room worker and local runtime operations.

### 4a. Packaging and embedding layers (browser vs pod vs .NET)

This plan is **not** “replace the server” or “fold persistence into host-ffi immediately.” It adds a **peer-local** layer that sits beside existing embeddings:

| Layer | Role today / target | Packaging |
|---|---|---|
| **Core engine** | `nodalmerge-core` — deterministic DAG/replay | Rust crate; WASM via `bridge` for browsers |
| **Browser peer** | SDK + `nodalmerge_bridge` (WASM) — many peers, app-managed storage optional | npm / static assets; **not** the headless persistence crate |
| **Reflector / server** | `nodalmerge-server` — room authority, `DirPersistence` for **server-side** room store | Container / binary |
| **Hosted .NET runtime** | `NodalMerge.DotNetHost` + `host-ffi` native RID packs — engine in-process via FFI | **NuGet** (`NodalMerge.DotNetHost`, `*.Native.win-x64`, etc.) |
| **Peer-local persistence (new)** | `nodalmerge-runtime-local` — **pod/workstation peer** durability before/after sync | Rust **rlib** first; consumed by future `nodalmerge-headless` worker |
| **Headless worker (future)** | Long-lived peer: WS to server + `PeerLocalPersistence` + same engine semantics as browser | Rust binary / **container image**; optional later FFI surface for .NET hosts |

**Design intent for packaging (no pivot required now):**

1. **`nodalmerge-runtime-local` stays lean** — no dependency on `server`, `host-core`, or `tokio`; safe to link from a headless binary, tests, or (later) a thin FFI shim.
2. **Browser path unchanged** — IndexedDB adapter (Phase B) implements the **same trait contract** in TS/WASM or via bridge hooks; it does not require NuGet.
3. **Kubernetes pods** — mount a volume at `data_dir`, set `FileLocalPersistence::open(data_dir)` in the headless worker; server persistence remains separate unless you explicitly export/import archives.
4. **.NET / NuGet later** — two viable patterns when productizing: (a) headless worker as a **sidecar/process** invoked by the dotnet host, or (b) optional **native RID pack** that exposes hydrate/append/recover C ABI over `runtime-local` (same pattern as today’s `host-ffi`, but scoped to persistence, not full engine). **Do not block Phase B on this**; keep `PeerLocalPersistence` stable so either packaging path can bind without semantic drift.
5. **Server `DirPersistence` vs peer `FileLocalPersistence`** — similar on-disk layout family (SQLite + blob dir) but **different contracts and directories**; a pod must not confuse server room roots with peer-local roots.

### 4b. Extensible / configurable peer-local backends (future state)

v1 ships **built-in** backends selected by config (`memory`, `file`). The contract is intentionally **plugin-shaped** so operators can substitute stores without changing engine or server semantics:

| Backend class | Examples | Notes |
|---|---|---|
| Built-in (in-tree) | `memory`, `file` | `PersistBackendKind` + `PersistenceHandle::open` in `nodalmerge-runtime-local` |
| Embedded local DB | SQLite (dedicated schema), LocalDB, LevelDB/Rocks | Same `PeerLocalPersistence` trait; may share layout ideas with `FileLocalPersistence` or use separate schema |
| Document / KV | MongoDB, DynamoDB, etcd | Peer-scoped prefix per `room_id`; latency + offline semantics documented per adapter |
| Distributed cache | Redis, memcached (+ optional backing store) | Often `memory` hot + async flush; define durability claims explicitly |
| Composite | memory front + file/DB back | e.g. write-through cache; must preserve `LOCAL-PERSIST-001` hash parity on recover |

**Configuration surface (evolving):**

1. **Today:** `NODALMERGE_HEADLESS_BACKEND` / `--backend` with `memory` \| `file` + `data_dir`.
2. **Follow-on:** registry key `backend = "custom"` + `backend_factory` / dynamic load (out of scope for v1); or host-provided `Arc<dyn PeerLocalPersistence>` for .NET sidecar embedding.
3. **Conformance:** each adapter must pass `LOCAL-PERSIST-*` vectors; optional `nodalmerge-runtime-local` conformance harness crate (mirror `node-stores/conformance`).

**Non-goals for plugin v1:** automatic backend migration, multi-writer quorum, or replacing server `NodePersistence` traits.

**Testing stance:** conformance and headless worker tests use **`file` or `memory`** built-ins; **`composite`** and **`register_backend`** pilot recorded in `docs/acceptance/headless-persistence-phaseb-composite-pilot-run01.json`.

**Pilot implementation (2026-05-28):**

1. `CompositeLocalPersistence` — memory cache + file write-through (`NODALMERGE_HEADLESS_BACKEND=composite`).
2. `registry.rs` — `register_backend` / `open_registered` / `PersistenceHandle::Dyn` for embedding custom stores.
3. Config: `registered:<name>` (e.g. `registered:composite`).

Conceptual adapter facets:

1. Node log persistence (append/load/checkpoint metadata)
2. Blob CAS persistence (put/get/list live references)
3. Optional metadata/index store (cursors, checkpoints, projection markers)
4. Durability and crash-recovery semantics reporting

Initial backend targets:

1. in-memory backend (tests/ephemeral pods)
2. filesystem backend (single-host durable)
3. embedded DB backend (for example SQLite for node metadata + optional blob mapping)
4. browser backend wrapper (IndexedDB implementation of same contract)

## 5. Front-side extensibility conclusion

Current front side is close but not fully modularized as a formal persistence plugin API.

1. SDK surface is storage-agnostic, which is good.
2. Current browser persistence appears app-managed around SDK/SyncStore.
3. To support pod/headless and configurable peer-local backends consistently, a dedicated persistence module is recommended.

Implication:

1. not "instead of sdk/wasm"
2. rather "sdk/wasm plus pluggable persistence module"

## 6. Phased implementation

### Phase A - Contract freeze

Deliverables:

1. local persistence trait/interface draft shared across browser and non-browser runtimes
2. durability/error taxonomy contract
3. compatibility policy (additive introduction, fallback behavior)

Acceptance criteria:

1. contract reviewed by SDK, runtime, and host maintainers
2. no ambiguity in lifecycle semantics (startup hydrate, flush, checkpoint, recovery)

Phase A working draft (2026-05-27) — evidence: `docs/acceptance/headless-persistence-phasea-contract-freeze-run01.json`

1. **Shared local persistence interface (conceptual)**  
   Facets (implementations may split traits or use one facade with sub-handles):
   - **Node log:** append ordered node payloads for a `room_id`, load from a durable cursor or checkpoint marker, list checkpoint metadata the runtime needs for hydrate/replay.
   - **Blob CAS:** `put` opaque blob bytes, `get` by content id, `list_live_refs` for GC/eligibility (semantics align with existing server blob adapters where possible).
   - **Optional index:** projection markers, client cursors, adapter-local caches — must not be required for canonical replay if the node log is complete.

   Lifecycle hooks (names illustrative; language-neutral contract):

   - `open(config) -> adapter` — idempotent where configs are unchanged; fails closed on version skew.
   - `hydrate(room_id) -> HydrateReport` — returns highest durable checkpoint visible to this peer; may be partial if corruption (see error taxonomy).
   - `append_nodes(room_id, batch, expected_tail) -> AppendReport` — optimistic tail optional for CAS-style writers; rejection if tail mismatch.
   - `flush(room_id) -> FlushReport` — barrier for durable backends; no-op semantic for memory backend.
   - `checkpoint(room_id, meta) -> CheckpointReport` — runtime-advised compaction marker; backend may no-op if unsupported.
   - `recover(room_id) -> RecoveryReport` — explicit post-crash scan; deterministic ordering of replay inputs from persisted log.

2. **Durability and error taxonomy (bounded `reason_class`)**  
   Minimum stable classes (extend only additively):

   - `reject.local_persist_unavailable` — backend not reachable or misconfigured.
   - `reject.local_persist_version_skew` — persisted schema/format newer than runtime supports.
   - `reject.local_persist_corruption` — checksum or structural read failure.
   - `reject.local_persist_tail_conflict` — concurrent writer or stale tail.
   - `reject.local_persist_quota` — disk or backend quota exceeded.
   - `reject.local_persist_readonly` — backend mounted read-only when write required.

   Each failure must map to a **recovery posture**: retryable (transient), operator-fix (config), data-loss-risk (corruption — escalate).

3. **Compatibility and fallback policy**  
   - New optional adapter capabilities are **additive**; runtimes must behave with defaults equivalent to today’s “no local durability” path when unset.
   - Config keys are **NodalMerge-first** with documented legacy aliases only where migration requires them (same pattern as host env).
   - Switching backend with existing data requires an explicit **export/import or migration** step; silent auto-migration is out of scope for v1 contract.

### Phase B - Adapter implementations

Deliverables:

1. memory adapter
2. filesystem adapter
3. embedded DB adapter
4. IndexedDB adapter aligned to shared contract

Acceptance criteria:

1. adapter conformance vectors pass for all backends
2. crash/restart recovery tests pass for durable backends

Phase B execution record (memory adapter, 2026-05-27):

1. Crate `nodalmerge-runtime-local` implements `PeerLocalPersistence`, bounded `LocalPersistReason` taxonomy, and `MemoryLocalPersistence` + `MemoryLocalStore`.
2. Conformance vectors in `runtime-local/tests/local_persist_vectors.rs`:
   - `LOCAL-PERSIST-001`: restart recovery (store handoff) restores identical canonical hash after replay.
   - `LOCAL-PERSIST-004` (partial): tail conflict and read-only rejection reason classes.
3. Evidence: `docs/acceptance/headless-persistence-phaseb-memory-adapter-run01.json`.
4. Next: filesystem adapter + durable crash/restart vectors; SDK/headless worker wiring (Phase C/D).

Phase B execution record (filesystem adapter, 2026-05-27):

1. `FileLocalPersistence` in `nodalmerge-runtime-local` (`runtime-local/src/fs.rs`): durable SQLite node log + `blobs/<room>/` CAS files under a configurable `data_dir`; `is_durable() == true`.
2. Vectors: `local_persist_001_fs_restart_recovery_restores_identical_canonical_hash` (drop + reopen path), `local_persist_003_fs_blob_roundtrip` (partial).
3. Evidence: `docs/acceptance/headless-persistence-phaseb-filesystem-adapter-run01.json`.
4. Next: headless worker binary (Phase D) wiring; optional FFI/NuGet packaging per §4a.

Phase D execution record (headless worker initial slice, 2026-05-27):

1. Crate `nodalmerge-headless` binary `nodalmerge-headless`: WS hello/catch-up, apply `pack` nodes into `PeerLocalPersistence`, flush + checkpoint on exit.
2. Config: `NODALMERGE_HEADLESS_*` env + `--server-url` / `--room` / `--data-dir`; backend via `NODALMERGE_HEADLESS_BACKEND` (`memory` \| `file`).
3. Vectors: `HEADLESS-RUN-001`, `HEADLESS-RUN-002` (partial) in `headless/tests/headless_run_vectors.rs`.
4. Evidence: `docs/acceptance/headless-run-phased-worker-run01.json`.
5. Next: Phase E observability/runbooks; custom backend registry per §4b.

Phase depth execution record (2026-05-27):

1. `headless/src/sync.rs`: IBF + `mst_root` in hello; post-welcome MST descent (`mst-request` / `mst-done`) and pack apply.
2. `HEADLESS-RUN-003`: file backend restart catches server-appended node (`headless_run_003_file_restart_catches_up_server_append`).
3. `headless/Dockerfile` + `.dockerignore` for pod deployments (`NODALMERGE_HEADLESS_BACKEND=file`, volume `/data`).
4. Evidence: `docs/acceptance/headless-run-phased-depth-run01.json`.

Phase E execution record (2026-05-27):

1. `WorkerSessionReport` JSON (`NODALMERGE_HEADLESS_REPORT_JSON`, `--report-json`) with `timings_ms` and backend/durable fields.
2. Operator runbook: `docs/operator.md` — Headless peer worker section.
3. `LOCAL-PERSIST-002` vector: memory vs file canonical hash parity.
4. CI smoke: `.github/workflows/wave2-runtime-smoke.yml`.
5. Evidence: `docs/acceptance/headless-run-phased-phasee-run01.json`.
6. Deferred: Prometheus metrics exporter, formal Criterion baselines per backend.

### Phase C - SDK integration

Deliverables:

1. optional `persistence` configuration in SDK/bridge wiring
2. default behavior preserved for existing users
3. migration notes from app-managed persistence to adapter-driven mode

Acceptance criteria:

1. existing SDK apps run unmodified
2. configured adapters pass browser parity tests

Phase C execution record (2026-05-27):

1. `nodalmerge-sdk-js`: optional `persistence: { enabled, adapter: "indexeddb", dbName, debounceMs }` on `createNodalMergeSdk`; default off.
2. `sdk-js/persistence/peer-local-indexeddb.js` — hydrate on `initialize()`, debounced flush on graph edits and inbound packs/blobs; `sdk.persistence.*` surface.
3. `sdk-js/PERSISTENCE_MIGRATION.md` — cutover from app-managed IndexedDB (e.g. `web/demo.js`).
4. `NODALMERGE_HEADLESS_BACKEND` aliases `embedded` \| `sqlite` → file (SQLite) backend in `nodalmerge-runtime-local`.
5. `nodalmerge-headless --health` — JSON probe of backend label + durable flag.
6. Evidence: `docs/acceptance/headless-persistence-phasec-sdk-run01.json`.
7. Demo cutover: `web/demo.js` + `web/sdk.js` use shared adapter — evidence `docs/acceptance/demo-persistence-cutover-run01.json`.
8. Phase E declared slice closeout — evidence `docs/acceptance/headless-persistence-phasee-closeout.json`. Prometheus exporter and Criterion baselines deferred.

### Phase E - Hardening and operations (closeout record)

Phase E execution record (declared slice closed, 2026-05-27):

1. Ops slice (session report, runbook, LOCAL-PERSIST-002, CI smoke) — `docs/acceptance/headless-run-phased-phasee-run01.json`.
2. `--health` probe on `nodalmerge-headless`; `embedded`/`sqlite` backend aliases.
3. Integration readiness index — `docs/INTEGRATION_READINESS.md`.
4. **Deferred:** Prometheus exporter, hosted dashboards, formal per-backend Criterion baselines.

### Phase D - Headless module + CLI

Deliverables:

1. headless runtime binary/command group for room sync workers
2. config file/env schema for backend selection and paths
3. pod-friendly commands for bootstrap, run, health, and diagnostics

Acceptance criteria:

1. headless worker can join room, sync DAG, and persist locally without browser runtime
2. lifecycle behavior validated in containerized runs

### Phase E - Hardening and operations

Deliverables:

1. observability for adapter health, durability mode, and flush latency
2. operator runbook for backend selection and failure handling
3. performance baselines for each backend mode

Acceptance criteria:

1. reproducible benchmark and recovery drills documented
2. no determinism regressions across backend switches

## 7. Conformance vectors

1. `LOCAL-PERSIST-001`: restart recovery restores identical canonical hash
2. `LOCAL-PERSIST-002`: backend switch with same data preserves replay hash
3. `LOCAL-PERSIST-003`: blob roundtrip parity across adapters
4. `LOCAL-PERSIST-004`: deterministic rejection classes for adapter failures
5. `HEADLESS-RUN-001`: headless worker sync parity with browser SDK path
6. `HEADLESS-RUN-002`: container restart continuity and recovery parity

## 8. Risks and mitigations

1. Risk: API churn in SDK create options.
   Mitigation: additive `persistence` option with backward-compatible defaults.
2. Risk: backend-specific behavior leaks into app semantics.
   Mitigation: strict conformance vectors on canonical hash parity.
3. Risk: operational complexity for pod deployments.
   Mitigation: constrained backend matrix and clear CLI runbooks.

## 9. Recommended sequencing

1. Start after speculative/authoritative and replay branching contracts stabilize.
2. Run Phase A in parallel with query/materialization Phase B-C.
3. Run Phase B-C before broad headless pod rollout.
4. Run Phase D-E as productization and operator adoption accelerators.
