# NodalMerge Combined Roadmap

Owner: Platform/runtime
Status: Active execution baseline
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
5. docs/WAVE0_CONTRACT_SIGNOFF_PACKAGE.md
6. docs/QUERY_MATERIALIZATION_EXECUTION_PLAN.md
7. docs/EXPORT_IMPORT_PORTABILITY_EXECUTION_PLAN.md
8. docs/HEADLESS_RUNTIME_PERSISTENCE_EXECUTION_PLAN.md
9. docs/AUTHORITY_AND_ROOM_TOPOLOGY_EXECUTION_PLAN.md
10. docs/MANAGER_WORKER_TOPOLOGY_PLAYBOOK.md

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
8. Query/materialization slice is now moving to Phase D deliverables (SDK/operator surface).
9. Export/import portability parallel track has started in Wave 1 with initial core/server deterministic vector coverage and acceptance artifacts.
10. Phase D implementation has started in `sdk-js`: query/projection runtime helpers now cover register/build/read/invalidate/list request paths with canonical checkpoint selector validation and runtime response matching tests.
11. Phase D hardening increment landed: `sdk-js` now includes rejected-path parity tests for `query.register.rejected` and `projection.build.rejected`, and `docs/operator.md` now documents operator lifecycle + failure triage for register/build/read/invalidate flows.
12. Export/import Phase A checkpoint advanced: `docs/EXPORT_IMPORT_PORTABILITY_EXECUTION_PLAN.md` now includes host/ws parity matrix headings and an initial deterministic rejection taxonomy draft for compatibility and integrity failure classes.
13. Export/import Phase A checkpoint advanced again: concrete host/ws envelope drafts and parity examples are now recorded for `archive.describe`, `archive.validate`, and `archive.import`, with acceptance artifacts `docs/acceptance/archive-phasea-envelope-draft.json` and `docs/acceptance/archive-phasea-ws-parity-draft.json`.
14. Export/import Phase A host implementation stub is now in place across mapper/processor/tests: runtime mapper supports `archive.describe`/`archive.validate`/`archive.import`, ws event mapping includes completed/rejected archive responses, and deterministic reject class stubs are validated via focused host tests and `docs/acceptance/archive-phasea-host-stub.json`.
15. Export/import Phase A parity vectors in core/server now include archive describe envelope metadata assertions plus deterministic validation/import rejection classes (`reject.archive_manifest_invalid`, `reject.archive_digest_mismatch`) with passing `archive_portability_vectors` suites in both lanes.
16. Export/import Phase A contract promotion is now in place: shared archive envelope/taxonomy types live in `nodalmerge-core` and both core/server archive parity vectors consume these shared contract types (`ArchiveWsResponse`, `ArchiveReasonClass`, archive checkpoint/provenance payload structs).
17. Export/import Phase A runtime adapter adoption is now in place for host-core/server integration paths: server dispatch routes archive control-plane commands, runtime ingress emits shared `ArchiveWsResponse` envelopes via host-core serializer helpers, and focused adapter parity tests are passing.
18. Export/import Phase A runtime processors are now persistence-backed in server paths: `server/src/archive_adapter.rs` resolves `room://` archive refs from persisted nodes/blobs, emits deterministic describe/validate/import contract payloads, enforces expected checkpoint mismatch rejection on import, and is covered by host migration parity fixture `archive_runtime_adapter_room_ref`.
19. Export/import runtime processors now support external archive manifest providers (`file://`, `object://`) with signature verification path and deterministic negative rejection parity (`reject.archive_unsupported_format`, `reject.archive_signature_invalid`, `reject.archive_checkpoint_not_found`) validated in host migration fixtures.
20. Export/import Phase A is now closed out with contract/runtime/negative-path evidence, and Phase B deterministic export builder kickoff is active for signed manifest generation and external-ref roundtrip vectors.
21. Export/import Phase B runtime wiring is now active: `archive.export` is routed through server adapter + websocket control-plane authorization, emits `archive.export.result` / `archive.export.rejected` envelopes, and is covered by host migration parity fixture `archive_runtime_export_file_ref`.
22. Export/import Phase B roundtrip parity vectors are now active for generated manifests: file/object export destinations are materialized by runtime builders and consumed by import parity tests that assert deterministic canonical hash equality.
23. Export/import Phase B conformance hardening is now complete: export manifest output includes compatibility-window metadata and payload digest policy declarations, with deterministic validation rejection mappings (`reject.archive_unsupported_format`, `reject.archive_policy_timeline_mismatch`) pinned by websocket-facing and server vector fixtures.
24. Export/import Phase B is now closed out with runtime/export/roundtrip/conformance evidence, and Phase C scope is open for richer portability semantics (expanded compatibility windows, policy timeline parity, and ws conformance lock for those semantics).
25. Export/import Phase C initial execution is now active: export outputs include `policy_timeline_hash` metadata (signed in manifest payload), and runtime validate/import enforce deterministic policy timeline parity checks for external manifests.
26. Phase C parity vectors now include deterministic policy timeline mismatch lanes in server and websocket-facing harnesses (`reject.archive_policy_timeline_mismatch`) while preserving existing archive suite stability.
27. Phase C compatibility semantics are broadened to explicit range behavior: export now declares a compatibility support window (`min_supported`..`max_supported`), and runtime validation accepts overlapping ranges while deterministically rejecting unsupported windows.
28. Phase C policy timeline parity now includes both hash and cutover metadata (`policy_timeline_hash`, `policy_timeline_cutover_lamport`) in signed manifests and `archive.export.result`, with deterministic mismatch rejection lanes pinned in core/server/websocket fixtures.
29. Next Phase C checkpoint: extend portability semantics for multi-step policy timeline transitions (non-zero cutover progression) and add conformance vectors for mixed-range migrations across version boundaries.

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

1. Start Query/materialization Phase A contract freeze implementation.
2. Start Export/import Phase A manifest and compatibility schema draft.
3. Record residual Wave R docs/dashboard follow-ups in backlog with owners and explicit non-blocking status.
4. Define Wave 1 parity vectors and acceptance artifact targets for query/materialization.
5. Define Wave 1 portability vectors and acceptance artifact targets for export/import compatibility.
