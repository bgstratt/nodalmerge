# Row 19 Acceptance Execution Plan

Status: Phase B in progress (backend parity milestone reached)
Owner: Host-runtime migration stream
Scope: Close row 19 in hostedMigrationPlan with evidence-driven acceptance, not ad-hoc slices.

Phase A status update:

- Task 1 complete: row-19 acceptance matrix created at `docs/acceptance/row19/ROW19_ACCEPTANCE_MATRIX.md`.
- Task 2 complete: validation-surface mapping captured per scenario (`legacy-demo-direct`, `speechslate-direct`, `speechslate-proxy`).
- Baseline SHA locked in matrix artifact.

Phase B status update (current milestone):

- Non-regression suite gates are green in partitioned form:
   - `cargo test -p activesync-host-core` passed (250 tests).
   - `cargo test -p activesync-host-ffi` passed (17 tests).
   - `.NET` suite slices passed:
      - `dotnet test ... --filter "FullyQualifiedName~FfiBindingTests"` passed (5 tests).
      - `dotnet test ... --filter "FullyQualifiedName~RuntimeWebSocketEndpointTests|FullyQualifiedName~RuntimeWebSocketLoopRunnerTests"` passed (61 tests).
- Hosted runtime path is live and healthy with explicit native binding:
   - Host running on `http://127.0.0.1:7878` with `ACTIVESYNC_HOST_FFI_DLL` pinned.
   - Health probe `GET /ffi/abi-version` returned `{"abiVersion":1}`.
   - Demo static server running on `http://127.0.0.1:8080` (`index.html` returned HTTP 200).
- Runtime parity blocker for same-room fanout is resolved by .NET room broker + relay tests (peer join/leave and pack relay coverage).

Phase B remaining work:

- Capture explicit legacy-demo UX scenario evidence rows in `docs/acceptance/row19/ROW19_ACCEPTANCE_MATRIX.md` (rows remain `Pending` until recorded).
- Investigate/record the occasional full-suite `--blame-hang` host abort behavior as a test harness stability item (does not invalidate passing partitioned gates).

## 1. Purpose

Row 19 is the parity acceptance layer for two UX surfaces:

1. Legacy web demo flows.
2. SpeechSlate-shape flows.

This plan defines a single execution path, clear gates, and evidence requirements so row 19 is closed intentionally.

## 2. Success criteria (row 19 can be marked Covered only when all are true)

1. Legacy web demo runs end-to-end on host-owned runtime backend with no legacy-only fallback behavior for covered rows.
2. SpeechSlate-shape acceptance flows pass against host-owned backend (or a documented proxy acceptance harness if the full SpeechSlate app is not available in this workspace).
3. Remaining partial rows (3-6) are either:
   - moved to Covered with evidence, or
   - explicitly deferred with written rationale and retained as Partial (in which case row 19 cannot be closed).
4. Standard non-regression gates are green:
   - cargo test -p activesync-host-core
   - cargo test -p activesync-host-ffi
   - dotnet test dotnet-host/tests/ActiveSync.DotNetHost.Tests/ActiveSync.DotNetHost.Tests.csproj --blame-hang --blame-hang-timeout 60s

## 3. Out of scope

1. New protocol expansion unrelated to acceptance.
2. Re-architecting SDK transport internals.
3. Product-level redesign work unrelated to parity evidence.

## 4. Execution model (phase-gated)

## Phase A: Baseline lock and acceptance matrix setup

Goals:
1. Freeze the candidate baseline SHA for row 19 execution.
2. Define exact acceptance scenarios and expected outcomes before editing code.

Tasks:
1. Build a row-19 acceptance matrix with pass/fail columns for:
   - map CRUD flows
   - text flows
   - list flows
   - blob flows (including fallback/direct behavior where applicable)
   - presence/subscription/policy/auth/sync/conflict/metrics observability checks
2. Identify which scenarios are validated in:
   - legacy demo directly,
   - SpeechSlate-shape flow directly,
   - proxy acceptance harness (if SpeechSlate app runtime is unavailable).

Exit criteria:
1. Matrix exists and is committed.
2. Every row has a deterministic expected result.

## Phase B: Legacy web demo acceptance pass

Goals:
1. Execute legacy demo flows against host-owned runtime backend.
2. Capture failures with minimal reproduction steps.

Tasks:
1. Run demo against host backend and execute matrix scenarios.
2. Record observed mismatches with exact command/event mismatch notes.
3. Classify each mismatch:
   - host runtime gap,
   - SDK integration gap,
   - demo-only wiring issue,
   - test/fixture issue.

Exit criteria:
1. Legacy demo scenarios are all passing, or all remaining failures are fully triaged and assigned.

## Phase C: SpeechSlate-shape acceptance pass

Goals:
1. Validate SpeechSlate-shape integration path against host-owned backend.

Primary path:
1. Run available SpeechSlate-shape flow checks from this repo/integration harness.

If full SpeechSlate app is unavailable in this workspace:
1. Use a proxy acceptance harness based on docs/integration shape (Mongo + S3/delegated presign assumptions).
2. Capture explicit limitation notes and evidence boundaries.

Exit criteria:
1. SpeechSlate-shape required scenarios pass, or hard blockers are documented with concrete external dependency requirements.

## Phase D: Close partial rows 3-6 via acceptance evidence

Goals:
1. Convert remaining partial rows to Covered using validated behavior.

Tasks:
1. For each row (3, 4, 5, 6), verify:
   - parity behavior works in acceptance scenarios,
   - no adapter fallback violates host-owned path assumptions,
   - regression tests exist or are added where gaps were found.
2. Update hostedMigrationPlan row notes from Partial to Covered when evidence is complete.

Exit criteria:
1. Rows 3-6 are Covered with evidence references.

## Phase E: Row 19 closure and final evidence

Goals:
1. Close row 19 in hostedMigrationPlan with auditable evidence.

Tasks:
1. Add completion log entry for row 19 summarizing:
   - what was validated,
   - what was changed,
   - test gates run,
   - any acknowledged limitations.
2. Update matrix status for row 19 to Covered.

Exit criteria:
1. hostedMigrationPlan row 19 is Covered.
2. All three standard gates are green in the closing run.

## 5. Evidence artifact template

For each acceptance scenario, capture:

1. Scenario ID and description.
2. Surface: legacy demo or SpeechSlate-shape/proxy harness.
3. Precondition/setup.
4. Steps executed.
5. Expected result.
6. Actual result.
7. Evidence pointer (test name/log/snippet).
8. Resolution status.

Recommended storage location:
1. docs/acceptance/row19/ (matrix, run logs, issue notes).

## 6. Working rules during execution

1. No status change to Covered without explicit scenario evidence.
2. Keep fixes minimal and scoped to acceptance blockers.
3. After each meaningful fix batch, re-run the three standard gates.
4. Preserve deterministic behavior contracts already established in rows 0-18.

## 7. Risks and mitigations

Risk: SpeechSlate runtime environment not present in workspace.
Mitigation: Use documented proxy acceptance harness and record boundary clearly.

Risk: Demo-only wiring issues hide runtime parity success.
Mitigation: Separate runtime parity checks from UX wiring checks in the matrix.

Risk: Long acceptance runs drift without closure.
Mitigation: Phase exit criteria are hard stop/go checkpoints.

## 8. Definition of done (final)

1. Rows 3-19 are Covered (or explicitly justified Not Applicable where allowed by policy).
2. Row 19 has a completion entry in hostedMigrationPlan.
3. Acceptance evidence is present and traceable.
4. Three standard gates are green on closing commit.
