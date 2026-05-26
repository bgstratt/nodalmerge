# Headless Runtime and Peer Persistence Execution Plan

Owner: Runtime + SDK + host streams
Status: Planned
Last updated: 2026-05-25

## 1. Why this plan exists

ActiveSync currently has:

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

1. `activesync-runtime-local` (new): shared local persistence adapter contract and default implementations.
2. `activesync-headless` (new): runtime module that hosts engine + transport + adapter wiring.
3. `activesync` CLI extension (or sibling CLI): headless room worker and local runtime operations.

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

### Phase B - Adapter implementations

Deliverables:

1. memory adapter
2. filesystem adapter
3. embedded DB adapter
4. IndexedDB adapter aligned to shared contract

Acceptance criteria:

1. adapter conformance vectors pass for all backends
2. crash/restart recovery tests pass for durable backends

### Phase C - SDK integration

Deliverables:

1. optional `persistence` configuration in SDK/bridge wiring
2. default behavior preserved for existing users
3. migration notes from app-managed persistence to adapter-driven mode

Acceptance criteria:

1. existing SDK apps run unmodified
2. configured adapters pass browser parity tests

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
