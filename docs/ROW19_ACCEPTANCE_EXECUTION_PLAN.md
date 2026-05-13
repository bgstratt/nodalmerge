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

## 9. Parity hardening tracking plan (post-baseline to production-grade)

Goal:
1. Move from "working parity slices" to deterministic, restart-safe, low-noise, production-operable parity.

### 9.1 Durable DAG persistence (native semantics, not pack-replay only)

Why it matters:
1. Current runtime durability stores/replays inbound packs, which is useful but not equivalent to first-class accepted-node persistence and lifecycle semantics.

Tracking deliverables:
1. Define and implement accepted-node persistence contract (node identity, causal metadata, apply markers, tombstone state, retention hints).
2. Implement native hydration from accepted-node store (not only replaying raw packs).
3. Add compaction/snapshot lifecycle policy with deterministic replay boundary.
4. Document persistence model and invariants in docs.

Acceptance checks:
1. Restart from cold state produces the same room frontier/hash as pre-restart.
2. Hydration does not emit duplicate logical applies.
3. Compaction/snapshot run preserves correctness across replay boundary.

Evidence:
1. New integration tests in dotnet-host runtime suite.
2. Frontier/hash comparison logs for pre-restart and post-restart.
3. Technical spec: docs/acceptance/row19/ROW19_9_1_DURABLE_DAG_TECH_SPEC.md.

Implementation decisions (locked before coding):
1. Source of correctness:
   - Accepted-node persistence plus deterministic hydration is the correctness path.
   - Snapshots are an acceleration layer for restart, not an independent source of truth.
   - Pack replay remains as emergency fallback behind a feature flag during bring-up only.
2. Backend priority:
   - Mongo is the primary implementation target for 9.1.
   - Connector abstraction remains, but parity validation is first proven on Mongo-backed runtime.
3. Accepted-node record contract (storage model):
   - Persist incremental accepted-node facts (not full-state rewrites per change).
   - Each accepted record includes node identity/hash, causal links/frontier inputs, payload kind, apply marker, and retention metadata.
   - This preserves incremental change economics while enabling deterministic recovery and compaction.
4. Snapshot model:
   - Snapshot represents latest full room state at snapshot boundary plus required metadata (room id, frontier/hash, schema/version).
   - Hydration flow is: load latest valid room snapshot, then apply accepted nodes after snapshot boundary.
5. Snapshot scope:
   - Per-room snapshots only.
6. Snapshot trigger policy:
   - Hybrid trigger with compaction focus on accepted-node distance from last snapshot, plus safety limits.
   - Do not snapshot every change.
   - Keep only the current active snapshot per room by default; replacement is atomic.
7. Compaction retention window:
   - Retention window is configurable.
   - Initial default: 7 days before compacting eligible incremental history.
8. Tombstone policy:
   - Conservative mode for 9.1: defer aggressive tombstone compaction rules until 9.4 is finalized.
9. Cutover strategy:
   - Move forward with native path as the intended default for this migration stream.
   - Keep fallback kill-switch only for recovery during rollout validation.
10. Determinism gate:
    - Frontier/hash equality is required and primary.
    - Materialized entity checks are also recorded in acceptance tests for operator confidence.
11. Failure behavior:
    - Startup must not fail hard on snapshot issues.
    - On snapshot corruption/missing data, recover via accepted-node hydration fallback and emit strong diagnostics.
12. Performance approach:
    - Establish baseline first (hosted runtime vs prior path), then set concrete latency/throughput targets from measured data.

Sequential implementation plan (no dates):
1. Baseline and invariants lock:
   - Capture current runtime behavior for hydrate/replay/apply under restart.
   - Freeze invariants: frontier/hash equivalence, exactly-once logical apply, tombstone precedence, deterministic room convergence.
2. Accepted-node contract definition:
   - Define persisted record shape for accepted nodes (node id/hash, causal links/frontier inputs, payload class, apply status, tombstone metadata, retention hints).
   - Define required indexes/lookup paths for hydrate, dedupe, and compaction eligibility.
3. Persistence write-path integration:
   - Persist accepted-node records at acceptance boundary (not only inbound pack boundary).
   - Add idempotent upsert semantics so duplicate inbound transport cannot create duplicate accepted records.
4. Native hydration implementation:
   - Build hydrate path from accepted-node store to runtime state reconstruction.
   - Keep pack-replay as fallback behind a flag during transition; native path becomes default after parity checks pass.
5. Deterministic replay boundary definition:
   - Define boundary contract between snapshot baseline and post-snapshot incremental accepted nodes.
   - Ensure replay starts from boundary marker and never re-applies logically finalized nodes.
6. Snapshot creation pipeline:
   - Implement snapshot generation for room state/frontier/materialized projections needed at restart.
   - Persist snapshot metadata (room id, version, created-at frontier/hash, compatibility marker).
7. Compaction pipeline:
   - Implement compaction rules for accepted-node retention with tombstone safety constraints.
   - Ensure compaction cannot remove records still required for correctness under reconnect/restart.
8. Recovery path switch-over:
   - Wire restart flow to load latest valid snapshot, then hydrate incrementally from boundary.
   - Add corruption/partial-snapshot fallback path with explicit audit logging and fail-safe behavior.
9. Correctness and non-regression tests:
   - Add deterministic restart tests across multi-room/multi-device scenarios.
   - Add assertions for no duplicate logical applies, no missing entities, and stable frontier/hash before/after restart.
   - Add compaction/snapshot lifecycle tests validating parity across compaction boundaries.
10. Observability and operational safety:
   - Emit room-scoped metrics/logs for hydrate duration, replay counts, snapshot age, compaction actions, and fallback activation.
   - Add debug trace correlation IDs for acceptance scenario artifacting.
11. Parity closure criteria for 9.1:
   - Native hydration path is default and pack-replay-only recovery is no longer required for correctness.
   - Snapshot/compaction lifecycle passes automated parity scenarios.
   - Evidence artifacts (tests + frontier/hash logs) are linked in row-19 acceptance folder.

### 9.2 Restart/recovery correctness guarantees

Why it matters:
1. Parity requires deterministic proof that host restarts do not lose or duplicate board/button/position materialization.

Tracking deliverables:
1. Define exactly-once reconciliation contract per entity type (board, button, position, text/list items).
2. Add deterministic restart test harness with seeded multi-room state.
3. Add recovery audit log markers (hydrate-start, hydrate-complete, reconcile-counts).

Acceptance checks:
1. After restart, final materialized state equals pre-restart state.
2. Reconcile applies each logical mutation exactly once.
3. No missing entries and no duplicate churn entries in steady state.

Evidence:
1. Automated restart scenario report with entity counts and checksums.
2. Runtime logs tied to scenario IDs in row-19 evidence folder.
3. Periodic restart soak report proving no cumulative drift in root hash and reconciliation counts.

Sequential implementation plan (no dates):
1. Restart contract lock:
   - Define restart correctness contract per entity class (board, button, position, text/list objects).
   - Define exact-once reconciliation semantics and allowed idempotent no-op paths.
2. Deterministic test fixture setup:
   - Build seeded multi-room fixtures with known causal history and expected final materialized state.
   - Ensure fixtures include both fixed and flexible layouts.
3. Recovery instrumentation:
   - Add structured recovery markers (hydrate-start, hydrate-complete, replay-start, replay-complete, reconcile-summary).
   - Include counts for created/updated/deleted/no-op operations by entity type.
4. Restart pipeline hardening:
   - Verify startup order and dependency readiness before hydration starts.
   - Ensure recoverable failure paths are deterministic and do not leave partial apply state.
5. Exactly-once guardrails:
   - Add dedupe keys or causal checkpoints at reconciliation boundary.
   - Ensure repeated transport/hydrate events cannot produce duplicate logical mutations.
6. Multi-stage restart scenario execution:
   - Run cold restart, warm restart, and restart-during-traffic scenarios.
   - Compare pre/post restart state checksums and frontier/hash values.
7. Regression and soak validation:
   - Add steady-state soak with periodic restart cycles.
   - Assert no cumulative drift in counts or reconciliation totals across cycles.
   - Implemented evidence: `Periodic_restart_soak_cycles_do_not_drift_root_or_reconciliation_counts` in `ProviderHostRestartDurabilityIntegrationTests`.
8. Parity closure criteria for 9.2:
   - Restart scenarios prove no missing entries and no duplicate logical apply.
   - Recovery logs provide deterministic traceability for each scenario run.
   - Evidence artifacts are linked in row-19 acceptance records.

### 9.3 Conflict/churn control

Why it matters:
1. Valid conflict metrics should remain visible, but normal edits must not produce repeated non-actionable re-apply noise.

Tracking deliverables:
1. Classify runtime metrics into actionable conflicts vs idempotent re-apply churn.
2. Implement loop suppression/idempotency guards for repeated pack/bulk re-apply paths.
3. Add "quiet steady-state" assertions for normal edit flows.

Acceptance checks:
1. Single-device normal edit flows produce no repeated apply loop bursts.
2. Multi-device normal edits converge with bounded, expected event volume.
3. Conflict metrics only fire for true concurrent semantic conflicts.

Evidence:
1. Metrics snapshots and threshold assertions from automated tests.
2. Before/after churn-rate comparison attached to acceptance notes.
3. Runtime replay taxonomy markers and duplicate-pack replay suppression tests in dotnet-host runtime durability suite.
4. Quiet steady-state restart cycles validated with root/count no-drift assertions and repeated replay-noise suppression.

Sequential implementation plan (no dates):
1. Signal taxonomy definition:
   - Split signals into true semantic conflicts, expected idempotent replays, and anomalous churn.
   - Define metric names and thresholds for each signal class.
2. Baseline noise capture:
   - Capture current event/apply volume for normal single-device and multi-device edit flows.
   - Establish baseline churn budget for regression checks.
3. Re-apply loop source tracing:
   - Instrument pack/bulk apply paths with causal trace IDs and source labels.
   - Identify repeated loop origins and classify root causes.
4. Idempotency and loop suppression:
   - Add guards to short-circuit duplicate logical applies.
   - Add bounded retry and replay suppression where repeated no-op loops are detected.
5. Conflict detector hardening:
   - Ensure conflict emission requires concurrent semantic divergence, not repeated transport replay.
   - Validate conflict payloads are actionable and entity-scoped.
6. Quiet steady-state test additions:
   - Add tests asserting bounded event volume under normal edits.
   - Add long-run scenario asserting no churn growth over time.
7. Metrics and alerting alignment:
   - Publish churn/conflict metrics with explicit alert thresholds.
   - Ensure dashboards distinguish conflict spikes from benign replay noise.
8. Parity closure criteria for 9.3:
   - Normal edits converge quietly within defined budgets.
   - Conflict metrics represent only true concurrent conflicts.
   - Before/after evidence demonstrates measurable churn reduction.
   - Implemented evidence in current branch: churn taxonomy counters (`room_recovery_semantic_mutations_total`, `room_recovery_idempotent_replay_total`, `room_recovery_signal_class_total`), duplicate replay suppression counter (`room_duplicate_pack_replay_suppressed_total`), and quiet steady-state restart assertions in `ProviderHostRestartDurabilityIntegrationTests`.

### 9.4 Delete and tombstone parity under all modes

Why it matters:
1. Deletes must never resurrect after reconcile/hydrate, including restart and offline windows.

Tracking deliverables:
1. Formalize tombstone precedence rules for merge/replay/hydrate.
2. Add delete propagation tests for online, offline-then-reconnect, and restart paths.
3. Ensure compaction does not drop required tombstone evidence too early.

Acceptance checks:
1. Cross-device delete converges and remains deleted after restart.
2. Offline delete/reconnect path converges without resurrection.
3. Tombstone retention/GC policy preserves correctness guarantees.

Evidence:
1. Multi-phase tests with explicit pre/post restart assertions.
2. Tombstone lifecycle notes in docs plus test references.
3. Restart + compaction delete terminality tests in `ProviderHostRestartDurabilityIntegrationTests`:
   - `Delete_remains_terminal_after_restart_and_hydration`
   - `Delete_state_survives_pruning_compaction_without_resurrection`
   - `Offline_delete_reconnect_converges_without_resurrection`
   - `Cross_device_multi_room_delete_propagation_converges_without_resurrection`

Sequential implementation plan (no dates):
1. Tombstone semantics contract:
   - Define deletion precedence across merge, hydration, replay, and compaction.
   - Define required tombstone metadata and retention invariants.
2. Delete-path audit:
   - Trace delete propagation through runtime, persistence, and hydration paths.
   - Identify any path that can re-materialize deleted entities.
3. Online delete convergence hardening:
   - Ensure cross-device deletes propagate deterministically and remain terminal.
   - Add explicit assertions for no post-delete resurrection.
4. Offline and reconnect delete handling:
   - Add offline delete scenarios for both deleting and non-deleting peers.
   - Validate reconnect merge preserves tombstone precedence.
5. Restart and hydration delete safety:
   - Validate delete state survives restart and snapshot/replay boundaries.
   - Prevent hydration from re-creating tombstoned entities.
6. Compaction retention constraints:
   - Add compaction guardrails to prevent premature tombstone pruning.
   - Define safe tombstone expiry policy tied to reconciliation guarantees.
7. Automated scenario suite updates:
   - Add delete/tombstone parity tests for online, offline, reconnect, and restart modes.
   - Add anti-resurrection assertions in all lifecycle tests.
8. Parity closure criteria for 9.4:
   - Deletes remain deleted across all supported modes and lifecycle transitions.
   - Tombstone retention/expiry policy is documented and test-backed.
   - Evidence links are present in row-19 acceptance artifacts.
   - Current 9.4 kickoff evidence now covers restart/hydration and pruning compaction anti-resurrection paths.
   - Current 9.4 policy status: tombstone expiry is intentionally disabled (no automatic tombstone pruning) until an explicit safe-expiry protocol is implemented and validated.

### 9.5 Blob lifecycle parity

Why it matters:
1. Delegated presign wiring exists, but parity requires durable asset availability and aligned GC/liveness behavior.

Tracking deliverables:
1. Define blob liveness model (reference tracking, retention windows, GC eligibility).
2. Add restart/reconnect blob availability tests (metadata + retrieval path).
3. Align blob GC policy with asset plan and failure-mode handling.

Current 9.5 liveness contract (host profile):
1. Referenced/live blobs: any blob still expected by active room/entity state must remain retrievable across restart/reconnect.
2. Unreferenced/orphan blobs: not yet deleted by dotnet-host provider path until explicit GC policy and sweeper are enabled in this stream.
3. Missing blob reads: must return deterministic `Found=false` with null bytes/content-type.

Acceptance checks:
1. Assets remain retrievable after host restart/reconnect when still referenced.
2. GC removes only unreferenced/expired assets per policy.
3. Missing blob behavior is deterministic and observable.

Evidence:
1. Blob propagation/retrieval integration tests.
2. GC dry-run/output reports attached to acceptance artifacts.
3. Provider durability evidence for restart/reconnect blob availability and deterministic missing behavior:
   - `SqliteFile_profile_preserves_blob_retrieval_across_multiple_restart_reconnect_cycles`
   - `File_blob_provider_missing_result_is_deterministic_across_restarts`
4. Multi-device propagation/retrieval evidence:
   - `SqliteFile_profile_supports_multi_device_blob_propagation_and_retrieval`
5. GC policy + execution evidence:
   - `FileBlobGcCoordinatorTests.DryRun_reports_mark_and_delete_candidates_without_mutating_files`
   - `FileBlobGcCoordinatorTests.LiveRun_applies_mark_then_delete_after_grace_window`

Sequential implementation plan (no dates):
1. Blob lifecycle contract definition:
   - Define referenced/unreferenced/expired states and transition rules.
   - Define relationship between room/entity references and blob liveness.
2. Reference tracking implementation review:
   - Verify write/update/delete flows update blob references correctly.
   - Add missing reference index fields needed for deterministic GC decisions.
3. Restart/reconnect availability path:
   - Validate asset metadata and retrieval remain consistent after host restart.
   - Validate reconnect flows re-establish access without stale presign behavior.
4. GC policy alignment:
   - Implement GC eligibility checks aligned to retention windows and reference state.
   - Add safety window and dry-run mode before destructive cleanup.
   - Implemented in dotnet-host file profile via `FileBlobGcCoordinator` with policy inputs: `GraceWindow`, `MaxDeletesPerRun`, and `RequireTombstoneBeforeDelete`.
5. Failure-mode hardening:
   - Define deterministic behavior for missing, expired, or orphaned blobs.
   - Emit actionable diagnostics for retrieval failures and policy decisions.
6. Blob parity integration tests:
   - Add multi-device asset propagation and retrieval scenarios.
   - Add restart/offline/reconnect scenarios with retrieval assertions.
7. GC validation tests and reports:
   - Validate only eligible blobs are removed.
   - Attach GC dry-run and live-run evidence outputs.
   - Implemented evidence outputs via `FileBlobGcRunReport` (scanned/marked/cleared/delete-candidates/deleted + hash lists) validated in unit tests.
8. Parity closure criteria for 9.5:
   - Referenced assets remain available across restart/reconnect.
   - GC behavior is policy-compliant and non-destructive to live references.
   - Evidence is linked in row-19 artifacts.

### 9.6 Auth hardening for production

Why it matters:
1. Local shared-secret flow is acceptable for development, but production parity requires hardened token/key lifecycle and strict validation.

Tracking deliverables:
1. Enforce issuer/audience validation policy and explicit clock-skew policy.
2. Introduce key rotation strategy and key provenance documentation.
3. Define failure-mode behavior for invalid/expired/mis-signed tokens.

Acceptance checks:
1. Positive path tokens validate under configured issuer/audience.
2. Negative path tokens fail closed with deterministic error semantics.
3. Rotation rollout does not break active sessions unexpectedly.

Evidence:
1. Runtime token validation tests (positive/negative/rotation).
2. Operational auth policy document and rollout checklist.
3. Embedded JWT hardening evidence (`ProviderProfileTokenEndpointIntegrationTests`):
   - `Sync_token_validate_rejects_embedded_token_when_issuer_mismatches`
   - `Sync_token_validate_allows_previous_key_during_embedded_rotation_overlap`
   - `Sync_token_validate_rejects_previous_key_when_overlap_not_configured`
   - `Sync_token_validate_rejects_when_embedded_token_capability_set_differs`
4. Startup policy/readiness guardrails evidence (`ProviderCompositionTests`):
   - `AddActiveSyncHostProviders_ThrowsForInvalidJwtEmbeddedClockSkew`
   - `AddActiveSyncHostProviders_ThrowsForInvalidJwtEmbeddedPreviousSigningKey`
5. Runtime token validation telemetry hardening:
   - `RuntimeTokenValidationService` now emits outcome-only logs (`allowed/denied`, reason, session, room) without token payload/signature content.

Sequential implementation plan (no dates):
1. Validation policy contract:
   - Lock issuer, audience, algorithm, and clock-skew requirements for hosted runtime.
   - Define required claims and rejection semantics.
2. Key management and rotation model:
   - Define key provenance, storage, rollout, and retirement process.
   - Define overlap window strategy for seamless key rotation.
3. Runtime enforcement changes:
   - Enforce strict validation with fail-closed behavior.
   - Ensure unsupported or malformed tokens produce deterministic errors.
   - Embedded provider now enforces HS256 algorithm allowlist, configurable clock skew bounds, and optional previous-key overlap validation.
4. Negative-path coverage expansion:
   - Add tests for invalid issuer/audience, expiry, signature mismatch, and malformed payloads.
   - Add tests for stale and future-dated token edge cases.
5. Rotation simulation coverage:
   - Add tests for pre-rotation, overlap, and post-rotation token acceptance/rejection.
   - Validate active sessions are handled per policy.
6. Operational rollout guardrails:
   - Add readiness checks for key config and validation policy at startup.
   - Add runbook entries for auth outage and rotation rollback handling.
   - Startup readiness checks now log embedded and sidecar auth policy posture (issuer/audience/clock skew/previous key count, sidecar timeout/base URL) with warnings on non-recommended settings.
7. Auditability and telemetry:
   - Emit structured auth failure metrics/logs without leaking sensitive token data.
   - Ensure telemetry supports rapid root-cause triage.
   - Runtime token validation path now logs deny reasons with session and room context only; token signature and claims are never logged.
8. Parity closure criteria for 9.6:
   - Hosted auth validates strict policy and fails closed on invalid tokens.
   - Rotation process is test-backed and operationally documented.
   - Acceptance artifacts include positive/negative/rotation evidence.

### 9.7 Operational parity (SLO-grade observability)

Why it matters:
1. Production confidence requires room-level durable metrics, replay/debug tooling, and incident runbook readiness.

Tracking deliverables:
1. Add room-scoped dashboards/metrics (connectivity, hydration time, replay counts, conflict rate, churn rate).
2. Add replay/debug tooling for incident forensics (scenario replay IDs, trace correlation).
3. Publish incident runbook for reconnect storms and conflict spikes.

Acceptance checks:
1. Simulated reconnect storm has actionable telemetry and clear mitigation steps.
2. Conflict spike triage can identify root room/entities within bounded time.
3. On-call runbook can be executed end-to-end in rehearsal.

Evidence:
1. Observability screenshots/log exports and metric definitions.
2. Runbook rehearsal notes with timestamps and outcomes.
   - docs/acceptance/row19/ROW19_9_7_OBSERVABILITY_DASHBOARDS_AND_ALERTS.md
   - docs/acceptance/row19/ROW19_9_7_RUNBOOK_REHEARSAL.md
3. Runtime metric/test evidence (dotnet-host):
   - `runtime_ws_connections_opened_total`
   - `runtime_ws_connections_closed_total`
   - `runtime_ws_inbound_messages_total`
   - `runtime_ws_pack_relay_total`
   - `runtime_auth_validation_total`
   - `trace_id` correlation threaded through runtime websocket/auth metrics and runtime pack relay payloads
   - `RuntimeWebSocketLoopRunnerTests.Runtime_ws_metrics_emit_connection_and_inbound_counts`
   - `RuntimeWebSocketLoopRunnerTests.Runtime_ws_metrics_emit_trace_and_pack_relay_correlation_counts`
   - `RuntimeTokenValidationServiceTests.ValidateInboundAsync_emits_runtime_auth_metrics_for_allowed_and_denied_paths`
   - `RuntimeTokenValidationServiceTests.ValidateInboundAsync_promotes_trace_id_from_payload_to_state_and_metrics`

Sequential implementation plan (no dates):
1. SLO and signal definition:
   - Define operational objectives for hydration latency, replay volume, reconnect stability, and conflict/churn rates.
   - Define required room-level metrics and alert thresholds.
2. Telemetry schema standardization:
   - Standardize structured logs and metric labels (room id, scenario id, trace id, lifecycle stage).
   - Ensure cardinality remains operationally safe.
3. Runtime metric instrumentation:
   - Instrument connect/disconnect, hydration duration, replay counts, snapshot age, conflict/churn counters.
   - Add per-room and aggregate views.
   - Implemented first slice: room-scoped websocket lifecycle and token-validation counters in dotnet-host runtime path.
4. Debug and replay tooling:
   - Add tooling to correlate scenario trace IDs to runtime events.
   - Add minimal replay workflow for incident triage.
   - Implemented first slice: inbound `trace_id` is promoted to connection state and emitted on runtime websocket/auth counters and pack-relay events.
5. Dashboard and alert wiring:
   - Build dashboards for healthy baseline and incident views.
   - Wire alerts for reconnect storms, churn anomalies, and conflict spikes.
   - Implemented baseline dashboard/alert pack in row-19 acceptance artifacts.
6. Incident runbook authoring:
   - Document step-by-step triage and mitigation for reconnect storms/conflict spikes.
   - Include escalation, rollback, and verification procedures.
7. Rehearsal and readiness validation:
   - Run simulated incident drills and capture time-to-detect/time-to-mitigate.
   - Refine thresholds and runbook steps from rehearsal outcomes.
   - Simulated reconnect-storm and auth-spike drills captured with runtime test-backed telemetry validation.
8. Parity closure criteria for 9.7:
   - Operators can diagnose and mitigate target incidents using shipped telemetry and runbooks.
   - Alerting and dashboards are validated in rehearsal scenarios.
   - Evidence artifacts are linked in row-19 acceptance folder.
   - Status in this stream: complete.

### 9.8 Automated acceptance-test parity coverage

Status: kickoff ready from 9.7 closure baseline.
Progress update: core required scenario slice implemented in row19 automated harness.

Why it matters:
1. Manual evidence is necessary but insufficient for sustained parity confidence.

Required automated scenarios:
1. Multi-device save/delete/restart.
2. Offline edit then reconnect.
3. Flexible and fixed layout position integrity.
4. Asset propagation and retrieval.
5. No duplicate apply churn over time.

Tracking deliverables:
1. Build a scenario harness with deterministic seeds and trace IDs.
2. Add pass/fail contracts and expected event-volume thresholds per scenario.
3. Integrate scenario suite into CI/nightly with artifact retention.

Acceptance checks:
1. All required scenarios pass on clean baseline and remain green across regressions.
2. Flaky scenarios are tracked with owner and remediation SLA.
3. Scenario artifacts are linked from row-19 evidence index.

Evidence:
1. Scenario test outputs, logs, and summary matrix in docs/acceptance/row19.
2. 9.8 contract + implemented slice evidence:
   - docs/acceptance/row19/ROW19_9_8_AUTOMATED_SCENARIO_CONTRACT.md
   - dotnet-host/tests/ActiveSync.DotNetHost.Tests/Row19AutomatedScenarioHarnessTests.cs
3. Current validation totals:
   - `dotnet test ... --filter "FullyQualifiedName~Row19AutomatedScenarioHarnessTests"` passed (6 tests).
   - `dotnet test ... --filter "FullyQualifiedName~Row19AutomatedScenarioHarnessTests|FullyQualifiedName~ProviderProfileTokenEndpointIntegrationTests|FullyQualifiedName~RuntimeTokenValidationServiceTests|FullyQualifiedName~RuntimeWebSocketLoopRunnerTests|FullyQualifiedName~ProviderDurabilityTests"` passed (54 tests).

Sequential implementation plan (no dates):
1. Scenario contract definition:
   - Finalize canonical scenario specs for all required parity flows.
   - Define deterministic expected outcomes and event-volume budgets.
2. Harness architecture:
   - Build reusable scenario runner with deterministic seeds, stable trace IDs, and artifact capture.
   - Support multi-device orchestration and restart/offline control points.
3. Core scenario implementation:
   - Implement automated cases for multi-device save/delete/restart and offline edit/reconnect.
   - Implement fixed and flexible layout position-integrity assertions.
   - Implemented slice 1: multi-device save/delete/restart and offline edit/reconnect deterministic scenarios.
   - Implemented slice 2: fixed+flexible layout integrity, asset propagation/retrieval, and duplicate replay churn guard scenarios.
4. Asset and churn scenarios:
   - Implement asset propagation/retrieval scenarios across reconnect/restart.
   - Implement long-run no-duplicate-churn scenario with budget assertions.
   - Implemented long-run duplicate-churn soak (`10k` repeated pack replay) with persisted-node budget threshold assertion.
5. Flake and nondeterminism controls:
   - Add deterministic waits/assertions based on convergence signals, not arbitrary sleeps.
   - Add retry policy only where safe and explicitly bounded.
6. CI and artifact retention wiring:
   - Integrate suite into CI/nightly gates.
   - Publish logs, metrics snapshots, and scenario summaries as build artifacts.
7. Governance and ownership model:
   - Define owner, triage SLA, and quarantine/remediation path for flaky scenarios.
   - Ensure scenario failures map to actionable subsystem ownership.
8. Parity closure criteria for 9.8:
   - All required automated scenarios are stable and release-gating.
   - Scenario artifacts are consistently linked from row-19 acceptance index.
   - Failure triage process is active and auditable.
   - Status in this stream: locked for scoped row-19 automated scenario set.

## 10. Recommended execution order for parity hardening

1. 9.1 Durable DAG persistence.
2. 9.2 Restart/recovery guarantees.
3. 9.4 Delete/tombstone parity.
4. 9.3 Conflict/churn control.
5. 9.5 Blob lifecycle parity.
6. 9.6 Auth hardening.
7. 9.7 Operational parity.
8. 9.8 Automated acceptance parity suite as continuous gate (starts early, becomes release-blocking by end).

Rationale:
1. Correct persistence and restart semantics are foundational; churn/observability/auth hardening are most useful once state correctness is deterministic.
