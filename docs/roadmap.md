# NodalMerge Combined Roadmap

Owner: Platform/runtime
Status: Active execution baseline
Last updated: 2026-05-27

## 1. Executive decisions

1. Promotion-based convergence from child rooms into parent canonical state is a hosted authority concern, not a core engine primitive.
2. Core engine remains room-local and deterministic.
3. Parent/child room governance and promotion workflows live in host-core plus server/admin contracts.
4. Two topology modes are supported:
   - reference-only room topology
   - promotion-based convergence topology

### 1a. Program map (where this file sits vs earlier ActiveSync planning)

An older **ActiveSync Combined Roadmap** used the same wave names (R, 0, 1, …) and **§7 immediate next items** (Wave 0 sign-off, rename inventory, query Phase A, export Phase A, topology CLI draft). That document is **superseded** by this NodalMerge roadmap; the work did not disappear, it was **executed and folded into the status bullets below**.

Plain-language position:

1. **Wave R (rename)** — done for runtime-critical surfaces; residual items are low-priority docs/dashboard sweep.
2. **Wave 0** — contract freeze and vectors are **done** (see Wave 0 status).
3. **Wave 1** — **Query/materialization** progressed through Phases A–D and **Phase E** (benchmarks + guardrails). **Phase E “run-NN”** is only an internal evidence label: run-01/02 captured vector-suite SLOs and Criterion pressure baselines; **run-03** adds an **end-to-end** vector: replay to a **fixed checkpoint**, build the **prefix projection**, then **paginate reads** and prove **digest continuity** (see `docs/acceptance/query-phasee-benchmark-baseline-run03.json`). **Export/import** Phases A–C and operational **Phase D** are **closed**; remaining work is **post-closeout monitoring** artifacts, not new conformance code.
4. **Wave 2** — **peer-local** + **headless** (depth, Phase E ops, composite/registry pilot) and **authority/topology Phases A–D closed**. Evidence: `docs/acceptance/headless-persistence-phaseb-composite-pilot-run01.json`, `docs/acceptance/authority-topology-phased-closeout.json`. **Wave 3 topology hardening (declared slice) closed** — `docs/acceptance/authority-topology-wave3-closeout.json`. Phase A sign-offs: out of band.

## 2. Canonical source plans

Core execution plans:

1. docs/NODALMERGE_RENAME_EXECUTION_PLAN.md
2. docs/NODALMERGE_RENAME_INVENTORY_CHECKLIST.md
3. docs/SPECULATIVE_AUTHORITATIVE_EXECUTION_PLAN.md
4. docs/REPLAY_BRANCHING_EXECUTION_PLAN.md
5. docs/WAVE0_CONTRACT_SIGNOFF_PACKAGE.md
6. docs/QUERY_MATERIALIZATION_EXECUTION_PLAN.md
7. docs/EXPORT_IMPORT_PORTABILITY_EXECUTION_PLAN.md
8. docs/HEADLESS_RUNTIME_PERSISTENCE_EXECUTION_PLAN.md
9. docs/AUTHORITY_AND_ROOM_TOPOLOGY_EXECUTION_PLAN.md
10. docs/MANAGER_WORKER_TOPOLOGY_PLAYBOOK.md
11. docs/EXECUTION_CHECKLIST.md

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

1. Rename wave execution is complete for runtime/testable surfaces across dotnet host, Rust host/core, host FFI, web bridge imports/artifacts, and local NuGet packaging.
2. DotNet host rename and compatibility work is validated with passing solution tests (`dotnet test nodalmerge-host/NodalMerge.DotNetHost.slnx`).
3. Runtime DAG persistence regression discovered during rename was fixed and host test suite remains green.
4. WASM bridge artifacts and web SDK imports switched to `nodalmerge_bridge` naming and rebuilt successfully.
5. Demo flow is validated against the dotnet host runtime endpoint; web serving is static Python (`web/serve.py`) and supports both dotnet (`/ws/runtime`) and Rust (`/ws/{room}`) endpoint forms.
6. Environment/config migration is in place with NODALMERGE-first precedence and legacy ActiveSync fallback where required.
7. Rust server manifest/bin mismatch is resolved: `authz-conformance-runner` source has been restored at `server/src/bin/authz_conformance_runner.rs` and package verification is no longer blocked by missing targets.
8. Docs/sweep items remain for low-priority historical references and supporting migration notes.
9. Canonical authz conformance workflow is green with parity pass: `docs/acceptance/authz-conformance-parity.json` (`run_id=20260526-215618`, `status=pass`; Rust 21/21 pass, DotNet 16/16 pass).
10. Dedicated timing lane now has completed evidence: `large_room_hydrates_quickly` passes at 10k with stage timers and canonical hash equivalence assertion (pre/post re-hydration hash match).
11. Import path regression was resolved by preserving batch input order in `import_nodes`, removing MissingParent retry amplification for linear dependency chains.
12. Wave R docs gate is closed for runtime/testable surfaces; remaining work is low-priority docs sweep and dashboard migration follow-up outside Wave 0 critical path.

Goals:

1. Rename ActiveSync to NodalMerge across all external surfaces.
2. Preserve compatibility aliases for consumers during transition.

Primary plans:

1. NODALMERGE_RENAME_EXECUTION_PLAN Phase A-C
2. NODALMERGE_RENAME_INVENTORY_CHECKLIST ownership and completion

Exit criteria:

1. nodalmerge command and package surfaces are primary. (Met for runtime/testable paths)
2. legacy ActiveSync aliases are validated and documented. (Met for host/runtime critical paths; residual docs sweep remains)
3. rust server package verification is green with all declared bins present. (Met; 10k persistence timing + canonical hash re-hydration check now passes)

### Wave 0: Foundation contract freeze

Current status:

1. Wave 0 is now active and seeded with a contract sign-off package artifact (`docs/WAVE0_CONTRACT_SIGNOFF_PACKAGE.md`).
2. Phase A kickoff scope is narrowed to semantic freeze and review sign-off for speculative/canonical lanes and replay fork contracts before implementation expansion.
3. Wave 0 vector owner mapping artifact is established at `docs/WAVE0_VECTOR_OWNER_MATRIX.md`.
4. All required Wave 0 vectors are now Passing with acceptance artifacts recorded (`docs/acceptance/spec-auth-001.json` .. `docs/acceptance/spec-auth-006.json`, `docs/acceptance/branch-fork-001.json` .. `docs/acceptance/branch-fork-006.json`).
5. Phase A contract decisions are approved and ambiguity items are closed in both execution plans; Wave 0 sign-off package is marked complete.

Goals:

1. Freeze canonical lane semantics.
2. Freeze replay fork semantics.

Primary plans:

1. SPECULATIVE_AUTHORITATIVE Phase A-B
2. REPLAY_BRANCHING Phase A-C

Exit criteria:

1. Canonical lane invariants approved. (Met)
2. Fork-cut determinism vectors passing. (Met)

### Wave 1: Core platform capability build-out

Current status:

1. Query/materialization Phase A stub coverage is in place with host/core vector evidence for `QUERY-DET-001`, `QUERY-DET-002`, `QUERY-REPLAY-001`, and `QUERY-INVAL-001`.
2. Runtime query/projection mapper and processor paths now include local deterministic paging (`limit` + `page_token`) and digest continuity checks in host tests.
3. Query/materialization Phase B stub lift has started: projection build rows are now sourced from canonical runtime map state (with `map-set`/`map-delete` mutation tracking) rather than fixed synthetic rows.
4. Query/materialization Phase C host replay stub is active: projection builds can resolve explicit canonical checkpoint cuts (`selector=seq`, `canonical_seq`) with passing replay parity and deterministic checkpoint-not-found rejection tests.
5. Checkpoint selector equivalence is now in host stub coverage: sequence, canonical hash, and frontier selector forms resolve the same canonical cut with digest parity and deterministic unknown-hash rejection.
6. Compatibility lane is now covered for selector payload validation: malformed frontier tokens, mixed selector fields, and canonical hash format failures deterministically reject with bounded reason classes (`reject.checkpoint_selector_invalid`, `reject.checkpoint_not_found`).
7. Query/materialization Phase C parity is now complete across host/core/server/ws lanes, including mismatch diagnostics coverage (checkpoint + digest metadata) and ws rejection-mapping parity for selector validation reason classes/messages.
8. Query/materialization slice has completed Phase D deliverables for this scope and is now in Phase E kickoff.
9. Export/import portability parallel track has started in Wave 1 with initial core/server deterministic vector coverage and acceptance artifacts.
10. Phase D implementation has started in `sdk-js`: query/projection runtime helpers now cover register/build/read/invalidate/list request paths with canonical checkpoint selector validation and runtime response matching tests.
11. Phase D hardening increment landed: `sdk-js` now includes rejected-path parity tests for `query.register.rejected` and `projection.build.rejected`, and `docs/operator.md` now documents operator lifecycle + failure triage for register/build/read/invalidate flows.
12. Query/materialization Phase D closeout is now complete for this slice: sdk-js rejected-path parity spans register/build/read/invalidate/list (`*.rejected`) and operator runbook coverage now includes rejection-specific triage and recovery guidance.
13. Query/materialization Phase E kickoff is active: core/server vector lanes now include deterministic multi-page ordering and digest continuity coverage as baseline performance-hardening guardrails.
14. Query/materialization Phase E benchmark run-01 is now recorded (`docs/acceptance/query-phasee-benchmark-slo-run01.json`, `docs/acceptance/query-phasee-benchmark-baseline-run01.json`) with warm latency/memory ceilings and baseline profile evidence.
15. Query/materialization Phase E benchmark run-02 is now recorded (`docs/acceptance/query-phasee-benchmark-baseline-run02.json`) with pressure-cardinality projection build/read/rebuild loop evidence and memory ceiling validation.
16. Query/materialization Phase E benchmark run-03 is now recorded (`docs/acceptance/query-phasee-benchmark-baseline-run03.json`): end-to-end replay to a fixed checkpoint, prefix projection materialization, and paginated read digest continuity across independent replays/imports (`query_phasee_replay_003_e2e_checkpoint_pagination_digest_parity`, `server_query_phasee_replay_003_e2e_checkpoint_pagination_digest_parity`). Minimal Phase E slice closeout (including deferral of cooperative projection-build backpressure) is in `docs/acceptance/query-phasee-closeout.json`.
17. Export/import Phase A checkpoint advanced: `docs/EXPORT_IMPORT_PORTABILITY_EXECUTION_PLAN.md` now includes host/ws parity matrix headings and an initial deterministic rejection taxonomy draft for compatibility and integrity failure classes.
18. Export/import Phase A checkpoint advanced again: concrete host/ws envelope drafts and parity examples are now recorded for `archive.describe`, `archive.validate`, and `archive.import`, with acceptance artifacts `docs/acceptance/archive-phasea-envelope-draft.json` and `docs/acceptance/archive-phasea-ws-parity-draft.json`.
19. Export/import Phase A host implementation stub is now in place across mapper/processor/tests: runtime mapper supports `archive.describe`/`archive.validate`/`archive.import`, ws event mapping includes completed/rejected archive responses, and deterministic reject class stubs are validated via focused host tests and `docs/acceptance/archive-phasea-host-stub.json`.
20. Export/import Phase A parity vectors in core/server now include archive describe envelope metadata assertions plus deterministic validation/import rejection classes (`reject.archive_manifest_invalid`, `reject.archive_digest_mismatch`) with passing `archive_portability_vectors` suites in both lanes.
21. Export/import Phase A contract promotion is now in place: shared archive envelope/taxonomy types live in `nodalmerge-core` and both core/server archive parity vectors consume these shared contract types (`ArchiveWsResponse`, `ArchiveReasonClass`, archive checkpoint/provenance payload structs).
22. Export/import Phase A runtime adapter adoption is now in place for host-core/server integration paths: server dispatch routes archive control-plane commands, runtime ingress emits shared `ArchiveWsResponse` envelopes via host-core serializer helpers, and focused adapter parity tests are passing.
23. Export/import Phase A runtime processors are now persistence-backed in server paths: `server/src/archive_adapter.rs` resolves `room://` archive refs from persisted nodes/blobs, emits deterministic describe/validate/import contract payloads, enforces expected checkpoint mismatch rejection on import, and is covered by host migration parity fixture `archive_runtime_adapter_room_ref`.
24. Export/import runtime processors now support external archive manifest providers (`file://`, `object://`) with signature verification path and deterministic negative rejection parity (`reject.archive_unsupported_format`, `reject.archive_signature_invalid`, `reject.archive_checkpoint_not_found`) validated in host migration fixtures.
25. Export/import Phase A is now closed out with contract/runtime/negative-path evidence, and Phase B deterministic export builder kickoff is active for signed manifest generation and external-ref roundtrip vectors.
26. Export/import Phase B runtime wiring is now active: `archive.export` is routed through server adapter + websocket control-plane authorization, emits `archive.export.result` / `archive.export.rejected` envelopes, and is covered by host migration parity fixture `archive_runtime_export_file_ref`.
27. Export/import Phase B roundtrip parity vectors are now active for generated manifests: file/object export destinations are materialized by runtime builders and consumed by import parity tests that assert deterministic canonical hash equality.
28. Export/import Phase B conformance hardening is now complete: export manifest output includes compatibility-window metadata and payload digest policy declarations, with deterministic validation rejection mappings (`reject.archive_unsupported_format`, `reject.archive_policy_timeline_mismatch`) pinned by websocket-facing and server vector fixtures.
29. Export/import Phase B is now closed out with runtime/export/roundtrip/conformance evidence, and Phase C scope is open for richer portability semantics (expanded compatibility windows, policy timeline parity, and ws conformance lock for those semantics).
30. Export/import Phase C initial execution is now active: export outputs include `policy_timeline_hash` metadata (signed in manifest payload), and runtime validate/import enforce deterministic policy timeline parity checks for external manifests.
31. Phase C parity vectors now include deterministic policy timeline mismatch lanes in server and websocket-facing harnesses (`reject.archive_policy_timeline_mismatch`) while preserving existing archive suite stability.
32. Phase C compatibility semantics are broadened to explicit range behavior: export now declares a compatibility support window (`min_supported`..`max_supported`), and runtime validation accepts overlapping ranges while deterministically rejecting unsupported windows.
33. Phase C policy timeline parity now includes both hash and cutover metadata (`policy_timeline_hash`, `policy_timeline_cutover_lamport`) in signed manifests and `archive.export.result`, with deterministic mismatch rejection lanes pinned in core/server/websocket fixtures.
34. Phase C portability semantics now include multi-step policy timeline transition metadata (`policy_timeline_transition_cutovers`) in signed manifests and `archive.export.result` envelopes, with deterministic runtime progression-shape validation and rejection vectors (`reject.archive_manifest_invalid`) for invalid transition ordering.
35. Phase C mixed-range migration boundary conformance vectors are now in place across server/ws lanes: no-overlap compatibility windows reject deterministically, and boundary edge-overlap windows accept deterministically with pinned fixture parity.
36. Phase C positive non-zero policy timeline transition progression conformance is now active: room runtime tracks monotonic policy cutover history, archive export emits non-zero cutover progression metadata when policy updates occur, and server/ws vectors pin deterministic parity for export/validate envelopes.
37. Export/import Phase C closeout evidence is now recorded after full core/server/websocket conformance rerun (`docs/acceptance/archive-phasec-closeout.json`), with compatibility range and policy timeline progression semantics locked across deterministic vectors.
38. Export/import Phase D scope is now open: migration drill matrix and baseline benchmark target gates are defined and recorded in `docs/acceptance/archive-phased-scope-open.json`.
39. Phase D drill run-01 is now recorded for ARCHIVE-DRILL-001/002 (`docs/acceptance/archive-phased-drill-run01.json`) with passing file/object migration conformance lanes.
40. Phase D drill run-01 is now recorded for ARCHIVE-DRILL-003/004 (`docs/acceptance/archive-phased-drill-run01-003-004.json`) with passing compatibility-boundary and policy-transition conformance lanes.
41. Phase D benchmark baseline run-01 is now recorded for ARCHIVE-DRILL-001/002 (`docs/acceptance/archive-phased-benchmark-baseline-run01.json`): peak memory is within gate, while latency lanes are currently above target and queued for optimization.
42. Phase D benchmark baseline run-02 is now recorded for ARCHIVE-DRILL-001/002 (`docs/acceptance/archive-phased-benchmark-baseline-run02.json`): runtime-aligned p95 latency gates and memory gate are all passing.
43. Phase D benchmark baseline run-03 is now recorded for ARCHIVE-DRILL-001/002 (`docs/acceptance/archive-phased-benchmark-baseline-run03.json`): second-slice manifest metadata cache optimization reduced validate/import p95 further while preserving gate pass.
44. Cache-hit telemetry is now instrumented for manifest metadata loads with focused cache behavior test coverage.
45. Phase D object-manifest parity benchmark run-04 is now recorded (`docs/acceptance/archive-phased-benchmark-baseline-run04.json`): file/object p95 deltas remain within 5 ms and all latency/memory gates stay green across both lanes.
46. Operator alert thresholds are now defined and recorded (`docs/acceptance/archive-phased-alert-thresholds-run01.json`) for cache miss ratio, parity drift, and absolute latency safety rails.
47. Operator runbook/dashboard wiring is now complete (`docs/acceptance/archive-phased-operator-alert-runbook-run01.json`) with explicit dashboard panel contract, ownership, and escalation flow.
48. Alert-route tabletop drill run-01 is now complete (`docs/acceptance/archive-phased-alert-route-tabletop-run01.json`) with warn/critical path acknowledgement and escalation timings meeting runbook SLAs.
49. Dashboard annotation and incident ticket templates are now published (`docs/acceptance/archive-phased-alert-template-publication-run01.json`) and linked from the operator runbook.
50. Live dashboard annotation + incident ticket dry-run run-01 is now complete (`docs/acceptance/archive-phased-alert-dryrun-run01.json`) with operator timing capture against runbook SLAs.
51. Critical-route live dry-run run-02 is now complete (`docs/acceptance/archive-phased-alert-dryrun-run02.json`) with L3 freeze and rollback decision logging evidence.
52. Warn + critical dry-run evidence is consolidated and Phase D operational closeout recommendation is now recorded (`docs/acceptance/archive-phased-operational-closeout-recommendation-run01.json`).
53. Runtime-owner signoff is complete and Phase D closeout is approved (`docs/acceptance/archive-phased-operational-closeout-signoff-run01.json`).
54. Next checkpoint: maintain post-closeout monitoring cadence and record threshold recalibration updates when triggered (see `docs/EXECUTION_CHECKLIST.md` §1.2).

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

Current status:

1. **Headless runtime / peer-local persistence — Phase A:** working draft recorded in `docs/HEADLESS_RUNTIME_PERSISTENCE_EXECUTION_PLAN.md` (persistence facets, lifecycle hooks, bounded error taxonomy, compatibility policy). Evidence: `docs/acceptance/headless-persistence-phasea-contract-freeze-run01.json`. Formal SDK/runtime/host maintainer sign-off remains out of band.
1b. **Headless persistence — Phase B:** `nodalmerge-runtime-local` ships `PeerLocalPersistence`, built-in backends (`memory`, `file` via `PersistBackendKind`), and extensibility notes for custom stores (plan §4b). Evidence: `docs/acceptance/headless-persistence-phaseb-memory-adapter-run01.json`, `docs/acceptance/headless-persistence-phaseb-filesystem-adapter-run01.json`.
1c. **Headless worker — Phase D (initial):** `nodalmerge-headless` joins a room over WS, applies catch-up `pack`s into peer-local persistence, flush/checkpoint on exit; env-configurable backend. Evidence: `docs/acceptance/headless-run-phased-worker-run01.json`. Packaging: §4a (browser WASM vs pod vs future NuGet).
2. **Authority and room topology — Phase A:** working draft recorded in `docs/AUTHORITY_AND_ROOM_TOPOLOGY_EXECUTION_PLAN.md` (authority role matrix, lineage metadata field table, promotion command/event table). Evidence: `docs/acceptance/authority-topology-phasea-contract-freeze-run01.json`. Cross-stream sign-off remains out of band.
3. **Manager/worker playbook — Phase A CLI plan + implementation:** frozen §7a commands implemented in `nodalmerge-cli` (`topology`, `run`, `archive`, `query`). Evidence: `docs/acceptance/manager-worker-cli-workflow-plan-run01.json`, `authority-topology-phased-cli-run01.json`, `pre-ai-workspace-integration-closeout.json`.
4. **Authority/topology — Phase B:** `RoomLineage` in core; server `create_child_room` / `list_children` / WS `topology.*` lineage commands (`topology.admin` cap). Evidence: `docs/acceptance/authority-topology-phaseb-lineage-run01.json`.
5. **Authority/topology — Phase C:** propose/validate/apply promotion on server (in-memory proposals + parent audit node). Evidence: `docs/acceptance/authority-topology-phasec-promotion-run01.json`.
6. **Authority/topology — Phase D + closeout:** `nodalmerge` binary in `nodalmerge-cli` crate; frozen §7a topology commands over WebSocket. Evidence: `docs/acceptance/authority-topology-phased-cli-run01.json`, `docs/acceptance/authority-topology-phased-closeout.json`.
7. **Headless worker depth:** IBF + MST in worker loop (`headless/src/sync.rs`), file restart catch-up vector `HEADLESS-RUN-003`, container image `headless/Dockerfile`. Evidence: `docs/acceptance/headless-run-phased-depth-run01.json`.
8. **Headless Phase E (ops slice):** session report JSON (`--report-json`), operator runbook section, `LOCAL-PERSIST-002`, CI `wave2-runtime-smoke.yml`. Evidence: `docs/acceptance/headless-run-phased-phasee-run01.json`.

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

1. **Pre–AI workspace integration — closed (2026-05-27):** `docs/INTEGRATION_READINESS.md`, `scripts/integration-smoke.ps1`, CLI (`run` / `topology` / `archive` / `query`), `runtime-local-ffi` + `RuntimePeerLocalPersistenceService`. Evidence: `docs/acceptance/pre-ai-workspace-integration-closeout.json`. Next product lane: AI workspace / agent memory (out of scope here). Observability vendor/dashboards remain deferred.
2. Execute export/import **weekly post-closeout monitoring** per `docs/acceptance/archive-phased-post-closeout-monitoring-run01.json` (next review **2026-06-03**).
3. Close **Wave 2 Phase A** sign-off threads out of band: SDK/runtime/host (headless plan) and runtime/host/operator (authority plan) acknowledge contract drafts or request edits.
4. Keep **query/materialization** smoke commands in CI when touching runtime: `cargo test -p nodalmerge-core query_ --test query_materialization_vectors` and `cargo test -p nodalmerge-server server_query_ --test query_materialization_vectors`.
5. Sweep **Wave R** residual docs/dashboard follow-ups into `docs/future-state-enhancements.md` with owners (non-blocking).
