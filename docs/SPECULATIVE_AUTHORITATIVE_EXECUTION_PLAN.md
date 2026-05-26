# Speculative vs Authoritative Execution Plan

Status: Draft (proposed)
Owner: Core + host runtime + SDK streams
Last Updated: 2026-05-25

## 1. Purpose

Define first-class runtime semantics for speculative intent and authoritative canonical state so optimistic UX and hosted authority converge deterministically.

This plan formalizes the path:
1. speculative local intent
2. authority validation/refinement
3. canonical reconciliation

## 2. Current Baseline

Existing signals in codebase:
1. Bridge exposes canonical/speculative read surfaces (`read_canonical`, `read_speculative`, `resolve_json_canonical`).
2. Authorization/capability model already supports namespace-scoped writes (for example `write:intent/**`).
3. Host/runtime paths already support rejection metadata and control-plane gating.

Gap:
1. There is no frozen cross-surface contract for intent-to-canonical lifecycle semantics.

## 3. Core Problem Statement

This is not a CRDT failure mode.
It is authority semantics layered over deterministic convergence.

Without a shared runtime model, each app reimplements:
1. optimistic layering
2. intent tagging
3. authoritative collapse
4. rollback/refinement behavior

That creates inconsistent behavior and tooling drift.

## 4. Scope

In scope (v1):
1. Runtime-level intent/canonical lane semantics.
2. Projection-layer reconciliation contract.
3. Authority acceptance/rejection/refinement event model.
4. SDK-host parity for rejection and reconciliation behavior.
5. Conformance vectors and benchmark guardrails.

Out of scope (v1):
1. Domain-specific business policies.
2. App-specific simulation/game rule engines.
3. Full workflow orchestration/approval engines.

## 5. Proposed Runtime Model (v1)

## 5.1 Lanes

1. Intent lane:
   - optimistic local writes, signed and replicated.
   - logically under intent namespace families.
2. Canonical lane:
   - authority-approved state used for durable world/workspace truth.

## 5.2 Lifecycle

1. Client emits intent op (optimistic local apply).
2. Authority validates/transforms/rejects intent.
3. Authority emits canonical op(s) and optional intent disposition event.
4. Client reconciles projection:
   - accepted: speculative state collapses into canonical.
   - rejected: speculative state rolls back/refines.

## 5.3 Minimal v1 metadata

1. `intent_id` (stable correlation id).
2. `intent_author`.
3. `intent_status` (`pending`, `accepted`, `rejected`, `superseded`).
4. optional `canonical_refs` (canonical op ids produced from intent).

## 6. Invariants

1. Canonical projection is deterministic across peers for the same canonical lane inputs.
2. Pending intents never silently mutate canonical state.
3. Every non-expired intent eventually reaches a terminal disposition (`accepted` or `rejected`) under authority mode.
4. Reconciliation is idempotent and replay-stable.
5. Rejections include canonical reason taxonomy/metadata.

## 7. Execution Phases

## Phase A: Semantic Contract Freeze

Goal:
1. Freeze intent/canonical lane contract and terminology.

Deliverables:
1. Lane semantics and namespace guidance (`intent/**` vs canonical namespaces).
2. Intent lifecycle state machine.
3. Host command/event additions for intent disposition signaling.

Exit criteria:
1. Contract approved by core, host, and SDK maintainers.
2. Rejection taxonomy mapping agreed for intent rejects.

## Phase B: Core Projection + Replay Semantics

Goal:
1. Ensure replay/projection semantics are deterministic with lane separation.

Deliverables:
1. Core projection layering support for lane-aware resolution.
2. Canonical-only replay path guarantees.
3. Optional lane-aware replay diagnostics for testing.

Exit criteria:
1. Replay vectors prove canonical hash stability independent of pending intents.
2. Projection parity vectors pass across deterministic replay runs.

## Phase C: Host Authority Pipeline

Goal:
1. Add runtime authority path for intent validation and canonical emission.

Deliverables:
1. Host command/event flow for intent ingestion and disposition.
2. Authority adapter hooks for validate/transform/reject.
3. Canonical emission mapping with intent correlation metadata.

Exit criteria:
1. End-to-end intent acceptance/rejection tests pass in host-core and server adapter.
2. Deny metadata and metrics are emitted with bounded-cardinality labels.

## Phase D: SDK and App-Facing Ergonomics

Goal:
1. Make intent/canonical usage explicit and hard to misuse.

Deliverables:
1. SDK APIs for intent submission and disposition subscriptions.
2. Projection helpers for optimistic UI and canonical confirmation.
3. Developer guidance for namespace and capability setup.

Exit criteria:
1. Sample flows validated in tactical showcase or equivalent demo harness.
2. No hidden behavior change in existing canonical read APIs.

## Phase E: Conformance + Performance Gates

Goal:
1. Make this release-gatable and regression-resistant.

Deliverables:
1. Cross-host conformance vectors for intent lifecycle parity.
2. Benchmark guardrails for projection overhead and reconciliation latency.
3. Replay/fork compatibility vectors (canonical lane only in v1).

Exit criteria:
1. Canonical conformance baseline green.
2. Benchmark thresholds pass for critical interaction paths.

## 8. Test and Conformance Matrix

Required vectors:
1. `SPEC-AUTH-001`: optimistic intent visible in speculative projection before authority response.
2. `SPEC-AUTH-002`: accepted intent yields canonical state convergence across peers.
3. `SPEC-AUTH-003`: rejected intent is removed/refined from speculative projection with reason metadata.
4. `SPEC-AUTH-004`: replay canonical hash is unchanged by pending or rejected intents.
5. `SPEC-AUTH-005`: intent disposition events are idempotent and order-safe under reconnect/replay.
6. `SPEC-AUTH-006`: capability-gated intent namespaces reject unauthorized writes deterministically.

## 9. Risks and Mitigations

Risk:
1. Extra cognitive surface for app teams.
Mitigation:
1. Keep v1 API narrow and provide clear SDK guidance with defaults.

Risk:
1. Projection overhead regressions in hot paths.
Mitigation:
1. Add benchmark gates before broad rollout.

Risk:
1. Divergent host behavior.
Mitigation:
1. Require cross-host conformance vectors and shared rejection taxonomy.

## 10. Recommended Priority

Priority recommendation:
1. Implement this plan first (Phase A-B immediately).
2. Start Replay Branching plan contract phase in parallel after Phase A freeze.
3. Gate Replay Branching implementation on canonical-lane invariants from this plan.

Rationale:
1. It clarifies what timeline is forkable in v1.
2. It reduces ambiguity for signed-op authority semantics.
3. It creates a stable substrate for branchable DAG tooling.

## 11. Integration with Replay Branching Plan

1. Fork v1 operates on canonical lineage only.
2. Intent lane inclusion in forks is explicitly deferred to a later milestone.
3. Both plans share replay/conformance infrastructure and should publish compatible vector IDs and artifact contracts.
