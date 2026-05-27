# Wave 0 Contract Sign-off Package

Owner: Platform/runtime
Status: Complete (Phase A approved)
Last updated: 2026-05-26

## 1. Purpose

Establish the Wave 0 sign-off package needed to freeze contracts before implementation expansion.

Wave 0 contract sources:

1. docs/SPECULATIVE_AUTHORITATIVE_EXECUTION_PLAN.md (Phase A-B)
2. docs/REPLAY_BRANCHING_EXECUTION_PLAN.md (Phase A-C)
3. docs/WAVE0_VECTOR_OWNER_MATRIX.md (vector owners + executable stub targets)

## 2. Entry Preconditions

1. Wave R runtime/testable rename surfaces are complete and verified.
2. Canonical authz conformance parity is green (`docs/acceptance/authz-conformance-parity.json`, run `20260526-215618`).
3. Local persistence timing-lane artifact is recorded for handoff context (`docs/acceptance/large-room-hydration-20260526.log`).

## 3. Required Sign-off Decisions

## 3.1 Speculative/Authoritative (Phase A)

1. Approve lane terminology and namespace policy (`intent/**` vs canonical namespaces).
2. Approve intent lifecycle states (`pending`, `accepted`, `rejected`, `superseded`).
3. Approve minimal metadata contract (`intent_id`, `intent_author`, `intent_status`, optional `canonical_refs`).
4. Approve rejection taxonomy mapping for intent rejects.

## 3.2 Replay Branching (Phase A)

1. Approve branch primitives (`BranchId`, `ParentBranchId`, `ForkPoint`, `BranchMeta`).
2. Approve deterministic fork-point interpretation (`Frontier` and `SnapshotHash`).
3. Approve host command contract baseline (`ForkRoom` shape).
4. Approve capability gate strategy for fork operations (`room.admin` vs dedicated `branch.admin`).

## 3.3 Cross-plan invariants

1. Canonical lane remains deterministic and replay-stable.
2. Branching v1 is canonical-lane-first by default.
3. Pending speculative intents do not mutate canonical state.
4. Fork payload remains ancestor-closed and reproducible from the same cut.

## 4. Wave 0 Review Checklist

Mark each item Complete with evidence links in review notes.

1. [x] Phase A contract review meeting held (core + host + SDK maintainers).
2. [x] Unresolved ambiguity list reduced to zero for lane and fork-point semantics.
	Review logs to complete during sign-off:
	- docs/SPECULATIVE_AUTHORITATIVE_EXECUTION_PLAN.md (Phase A decision log + ambiguity closure table)
	- docs/REPLAY_BRANCHING_EXECUTION_PLAN.md (Phase A decision log + ambiguity closure table)
3. [x] Vector ownership assigned for SPEC-AUTH-001..006 and BRANCH-FORK-001..006. Evidence: docs/WAVE0_VECTOR_OWNER_MATRIX.md.
4. [x] Acceptance harness plan drafted and materialized as executable stubs with artifacts under docs/acceptance/.
5. [x] Sign-off record added to roadmap and relevant execution plans after Phase A approval meeting.

## 5. Initial Owner Assignment (Kickoff)

1. Core stream: deterministic semantics and replay invariants.
2. Host runtime stream: command/event contract shape and authorization gates.
3. SDK stream: app-facing semantics clarity and misuse-resistant surface review.
4. Docs stream: finalized terminology and contract summary publication.

## 6. Immediate Next Actions

1. Freeze Wave 0 sign-off package as completed baseline for downstream waves.
2. Begin Query/materialization Phase A contract freeze implementation.
3. Begin Export/import Phase A manifest and compatibility schema draft.
4. Track residual Wave R docs/dashboard follow-ups as non-blocking backlog work.

Review execution note:
1. Decision logs are approved and ambiguity rows are closed in both execution plans; Wave 0 sign-off checklist is complete.
