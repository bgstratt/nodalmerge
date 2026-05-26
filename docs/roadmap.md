# ActiveSync Combined Roadmap

Owner: Platform/runtime
Status: Active planning baseline
Last updated: 2026-05-26

## 1. Executive decisions

1. Promotion-based convergence from child rooms into parent canonical state is a hosted authority concern, not a core engine primitive.
2. Core engine remains room-local and deterministic.
3. Parent/child room governance and promotion workflows live in host-core plus server/admin contracts.
4. Two topology modes are supported:
   - reference-only room topology
   - promotion-based convergence topology

## 2. Canonical source plans

Core execution plans:

1. docs/NODALMERGE_RENAME_EXECUTION_PLAN.md
2. docs/NODALMERGE_RENAME_INVENTORY_CHECKLIST.md
3. docs/SPECULATIVE_AUTHORITATIVE_EXECUTION_PLAN.md
4. docs/REPLAY_BRANCHING_EXECUTION_PLAN.md
5. docs/QUERY_MATERIALIZATION_EXECUTION_PLAN.md
6. docs/EXPORT_IMPORT_PORTABILITY_EXECUTION_PLAN.md
7. docs/HEADLESS_RUNTIME_PERSISTENCE_EXECUTION_PLAN.md
8. docs/AUTHORITY_AND_ROOM_TOPOLOGY_EXECUTION_PLAN.md
9. docs/MANAGER_WORKER_TOPOLOGY_PLAYBOOK.md

Backlog tracker:

1. docs/future-state-enhancements.md

Inventory index:

1. docs/operations-inventory.md

## 3. Priority map

P1:

1. Speculative vs authoritative semantics
2. Replay branching
3. Query/materialization
4. Authority model plus parent/child topology (FSE-10)

P2:

1. Export/import portability
2. Runtime scheduler/backpressure (FSE-03)
3. Fine-grained replay subscriptions (FSE-04)
4. Headless runtime plus peer-local persistence adapters (FSE-09)

P3:

1. Presence/session durability polish (FSE-05)
2. Schema/version ergonomics (FSE-06)
3. Native text acceleration structures (FSE-07)

P4:

1. Distributed authority/federation semantics (FSE-08, deferred)

## 4. Ordered execution waves

### Wave R: Rename and compatibility first

Current status:

1. Phase A started via `docs/NODALMERGE_RENAME_INVENTORY_CHECKLIST.md`.
2. Initial inventory batch RNM-001 through RNM-006 marked InProgress.
3. Batch 2 RNM-007 through RNM-011 marked InProgress with concrete implementation steps.
4. RNM-007 mechanical namespace migration completed in host C# files and validated with successful `dotnet build` + `dotnet test`.
5. RNM-008 dual-key configuration path implemented (NodalMerge primary with ActiveSync fallback) and validated with successful `dotnet build` + `dotnet test`.
6. RNM-009 started with runtime/script dual env-var support for host FFI probe (`NODALMERGE_HOST_FFI_DLL` primary, `ACTIVESYNC_HOST_FFI_DLL` fallback).
7. RNM-011 started with Docker primary entrypoint switch to `nodalmerge-server` while retaining `activesync-server` compatibility alias.
8. RNM-010 now covers dotnet-host and Rust server dual metric emission (NodalMerge primary names with ActiveSync compatibility window aliases).
9. RNM-009 extended to server env parsing with `NODALMERGE_SCOPE_MAX_FILTERED_CATCHUP_{NODES,BYTES}` and `NODALMERGE_CAPABILITY_PROFILE_PATH` plus `ACTIVESYNC_*` fallback aliases.
10. RNM-011 Docker build and compatibility validation now pass with Docker Desktop running.
11. Origin migration started: new GitHub repository `bgstratt/nodalmerge` created; local remotes now use `origin` -> `nodalmerge` and preserve `origin-legacy` -> `activeSync`.
12. DotNet host folder rename started: local path migrated from `dotnet-host` to `nodalmerge-host` with active benchmark/task scripts updated to the new path.

Goals:

1. Rename ActiveSync to NodalMerge across all external surfaces.
2. Preserve compatibility aliases for consumers during transition.

Primary plans:

1. NODALMERGE_RENAME_EXECUTION_PLAN Phase A-C
2. NODALMERGE_RENAME_INVENTORY_CHECKLIST ownership and completion

Exit criteria:

1. nodalmerge command and package surfaces are primary.
2. legacy ActiveSync aliases are validated and documented.

### Wave 0: Foundation contract freeze

Goals:

1. Freeze canonical lane semantics.
2. Freeze replay fork semantics.

Primary plans:

1. SPECULATIVE_AUTHORITATIVE Phase A-B
2. REPLAY_BRANCHING Phase A-C

Exit criteria:

1. Canonical lane invariants approved.
2. Fork-cut determinism vectors passing.

### Wave 1: Core platform capability build-out

Goals:

1. Add deterministic read and portability layers.
2. Keep host/ws/sdk parity while contracts are still narrow.

Primary plans:

1. QUERY_MATERIALIZATION Phase A-C
2. EXPORT_IMPORT_PORTABILITY Phase A-C

Exit criteria:

1. Live versus replay query parity vectors passing.
2. Archive roundtrip and compatibility vectors passing.

### Wave 2: Service topology and worker architecture

Goals:

1. Enable manager-worker deployments with explicit room governance.
2. Add headless runtime workflows for pods and workstation tools.

Primary plans:

1. HEADLESS_RUNTIME_PERSISTENCE Phase A-C
2. AUTHORITY_AND_ROOM_TOPOLOGY Phase A-C
3. MANAGER_WORKER_TOPOLOGY operationalization

Exit criteria:

1. Child room lineage metadata enforced.
2. Promotion propose/validate/apply path deterministic.
3. Headless worker parity with browser/runtime paths confirmed.

### Wave 3: Operational and SDK surfaces

Goals:

1. Expose stable user/operator interfaces.
2. Harden observability and runbooks.

Primary plans:

1. SDK/operator phases from core plans
2. HEADLESS_RUNTIME_PERSISTENCE Phase D-E
3. AUTHORITY_AND_ROOM_TOPOLOGY Phase D-E
4. FSE-03 scheduler/backpressure
5. FSE-04 replay subscriptions

Exit criteria:

1. CLI workflows for topology and promotion are production-ready.
2. Performance and reliability baselines documented.

### Wave 4: Hardening and deferred advanced tracks

Goals:

1. Continue quality and ecosystem depth.
2. Prepare long-horizon federation work only after earlier invariants are stable.

Primary plans:

1. FSE-05, FSE-06, FSE-07
2. FSE-08 (deferred, gated)

Exit criteria:

1. Hardening vectors and migration guidance complete.
2. Federation preconditions explicitly validated.

## 5. Promotion-based convergence policy

Decision:

1. Required for durable child-to-parent canonical updates.
2. Optional when parent only catalogs child references and summaries.

Policy gates for promotion mode:

1. Child must include parent checkpoint lineage metadata.
2. Promotion must pass deterministic validation.
3. Parent apply step must emit audit metadata and stable reason classes.

## 6. Tracking checklist

Use this checklist at each iteration:

1. Confirm current wave scope and freeze non-wave changes.
2. Confirm required vectors for the wave are implemented and green.
3. Update docs/future-state-enhancements.md status fields.
4. Update docs/operations-inventory.md plan index and sequencing notes if changed.
5. Record any contract freeze decisions in relevant execution plans.

## 7. Immediate next 5 execution items

1. Finalize Wave 0 contract review sign-off package.
2. Execute rename inventory freeze and compatibility matrix from NODALMERGE_RENAME plan.
3. Start Query/materialization Phase A contract freeze implementation.
4. Start Export/import Phase A manifest and compatibility schema draft.
5. Draft topology CLI command contract from manager-worker playbook.
