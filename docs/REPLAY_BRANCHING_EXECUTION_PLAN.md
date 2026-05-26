# Replay Branching Execution Plan

Status: Draft (proposed)
Owner: Core + host runtime streams
Last Updated: 2026-05-25

## 1. Purpose

Define first-class branch/fork semantics in ActiveSync core/runtime so rooms can fork from historical DAG cuts deterministically, not as app-only metadata.

This plan treats branching as core lineage semantics, not a UI convenience.

## 2. Why This Is Core

Branching impacts:
1. DAG/node lineage semantics.
2. Replay and deterministic rebuild semantics.
3. Frontier semantics and sync diffs.
4. Snapshot/compaction semantics.
5. Host command/event and admin operation semantics.
6. Identity/audit semantics for exported/imported timelines.

If branching is implemented independently per app, interoperability and deterministic tooling parity are lost.

## 3. Design Principles

1. First-class lineage metadata in core/runtime.
2. Deterministic fork reproduction from the same source cut.
3. Canonical-first: initial fork semantics operate on canonical state timeline.
4. Backward-compatible wire and storage evolution where possible.
5. Auditability: every fork must be attributable and reproducible.

## 4. Scope

In scope (v1):
1. Branch identity primitives and fork metadata.
2. Deterministic historical cut selection and ancestor-closed extraction.
3. Room fork command path (host-core + server admin path).
4. Import/export contract for fork payloads.
5. Conformance vectors for deterministic hash/state equivalence.

Out of scope (v1):
1. Cross-branch merge UI/workflows.
2. Automatic conflict-resolution across branches.
3. Branch GC and retention automation beyond basic policy hooks.

## 5. Core Primitives (minimum v1)

Proposed minimum types:
1. `BranchId` (stable identifier).
2. `ParentBranchId` (optional, null for root branch).
3. `ForkPoint`:
   - `Frontier(Vec<NodeId>)`
   - `SnapshotHash(Hash)`
4. `BranchMeta`:
   - `branch_id`
   - `parent_branch_id`
   - `fork_point`
   - `created_at_hlc`
   - `created_by` (peer/service identity)

Proposed host/runtime command shape:
1. `ForkRoom { source_room, source_branch, fork_point, target_room, target_branch }`.

## 6. Invariants

1. Fork payload node set is ancestor-closed for the selected cut.
2. Replay hash equality holds: source-at-cut hash equals target-initial hash.
3. Source room history is immutable after fork creation.
4. Post-fork writes in target room do not mutate source lineage.
5. Fork metadata survives snapshot/compaction/rebuild cycles.

## 7. Execution Phases

## Phase A: Contract Freeze

Goal:
1. Freeze branch/fork core contract before implementation.

Deliverables:
1. Branch primitive type definitions and wire/storage compatibility notes.
2. Fork command and event contract in host-core.
3. Operator-level fork semantics in docs (permissions, audit, rollback).

Exit criteria:
1. Contract review approved by core and host maintainers.
2. No unresolved ambiguity in fork-point semantics.

## Phase B: Deterministic History-Cut Planner

Goal:
1. Implement deterministic extraction of fork-ready node sets.

Deliverables:
1. Cut planner: `frontier -> ancestor-closed node set`.
2. Snapshot-hash based cut resolution.
3. Fork payload generator (`pack` and optional snapshot metadata).

Exit criteria:
1. Deterministic vector pass: same source + fork-point -> identical payload.
2. Replay hash equality vectors pass in core tests.

## Phase C: Host-Core + Server Fork Operation

Goal:
1. Expose fork as a first-class runtime/admin operation.

Deliverables:
1. `HostCommand::ForkRoom` (or equivalent command/event pair).
2. Server admin operation mapping for room fork creation.
3. Capability gate for fork operation (for example `room.admin` or new `branch.admin`).

Exit criteria:
1. End-to-end fork integration tests pass.
2. Authorization and rejection taxonomy integration is complete.

## Phase D: SDK/Tooling + Operational Guardrails

Goal:
1. Make fork behavior operable and observable.

Deliverables:
1. SDK/admin helper surface for initiating forks.
2. Operator runbook for safe fork execution and verification.
3. Metrics/log fields for fork frequency, success/failure, and hash-equivalence checks.

Exit criteria:
1. Operator dry-run flow validates with no manual DB surgery.
2. Fork audit artifacts generated in CI acceptance drills.

## Phase E (Follow-on): Branch Compare/Merge Toolkit

Goal:
1. Add compare/merge primitives once v1 fork baseline is stable.

Deliverables:
1. Branch diff payload contract.
2. Merge precondition checks and conflict report format.

Exit criteria:
1. Compare vectors are deterministic.
2. Merge behavior is gated behind explicit policy/feature flag.

## 8. Test and Conformance Matrix

Required vectors:
1. `BRANCH-FORK-001`: fork from frontier produces identical replay hash.
2. `BRANCH-FORK-002`: fork from snapshot hash produces identical replay hash.
3. `BRANCH-FORK-003`: target writes after fork do not alter source state hash.
4. `BRANCH-FORK-004`: source writes after fork do not alter target lineage.
5. `BRANCH-FORK-005`: compaction/rebuild preserves branch metadata.
6. `BRANCH-FORK-006`: unauthorized fork request is rejected with canonical reason metadata.

## 9. Risks and Mitigations

Risk:
1. Branch metadata bloat in hot paths.
Mitigation:
1. Keep metadata compact and mostly room/branch scoped, not per-op heavy where avoidable.

Risk:
1. Snapshot/fork compatibility drift.
Mitigation:
1. Add explicit compatibility version fields and conformance vectors for roundtrips.

Risk:
1. Ambiguous fork-point interpretation between hosts.
Mitigation:
1. Freeze canonical cut resolution rules in core tests before host rollout.

## 10. Dependency and Sequencing Notes

1. Speculative/authoritative semantics should define canonical-lane boundaries first.
2. Replay branching v1 should fork canonical lineage only (exclude speculative-only lanes by default).
3. After speculative/authoritative v1 is stable, evaluate optional intent-lane fork modes.

## 11. Recommended Implementation Order

Recommended order:
1. Execute Speculative/Authoritative plan Phase A-B first.
2. Execute Replay Branching plan Phase A-C second.
3. Execute both plans' SDK/operator phases in parallel after core contracts settle.

Rationale:
1. Canonical lane semantics reduce branching ambiguity.
2. Fork correctness vectors become simpler and stronger when canonical boundaries are frozen.
