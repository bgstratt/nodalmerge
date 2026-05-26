# Platform Foundation Execution Plan

Status: Active
Owner: Core + host runtime streams
Last Updated: 2026-05-22
Companion Tracker: [AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md](AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md)

## 1. Purpose

This plan focuses platform work on architecture-shaping capabilities while keeping workspace/product delivery moving.

Principles:
1. Build only the minimum viable foundational infrastructure needed to avoid architectural dead ends.
2. Keep implementation slices small, testable, and releasable.
3. Tie every phase to explicit conformance and runtime evidence.

## 2. Scope Priorities

Priority order:
1. Partial replication and subscription scopes (foundation).
2. Snapshot + replay compaction hardening (foundation).
3. Conformance + hardening expansion (continuous gate health).
4. Policy/capability stabilization (finish promotion evidence).
5. Identity continuity + key rotation v1 (incremental but mandatory path).

Out of scope for this cycle:
1. Federated trust mesh and enterprise delegation DSLs.
2. Semantic query subscriptions and dynamic policy-aware filtering.
3. Advanced storage tiering and distributed compaction orchestration.

## 3. Execution Phases

## Phase A (Week 1-2): Scoped Replication v1

Status:
1. Complete (2026-05-22).

Goal:
1. Ensure subscription scopes are operationally safe and measurable for large rooms.

Deliverables:
1. Scope budget contract for catch-up and relay paths:
   - max filtered catch-up payload bytes
   - max filtered node count
   - deterministic fallback behavior when budget exceeded
2. Runtime metrics for scoped delivery:
   - filtered_nodes_total
   - filtered_bytes_total
   - filtered_pack_dropped_total
3. Conformance vectors for scope correctness:
   - matching namespace receives data
   - non-matching namespace excluded
   - reconnect catch-up obeys scopes

Exit criteria:
1. Scope vectors pass in Rust host and DotNetHost mapped parity slices.
2. Large-room smoke test demonstrates bounded catch-up with subscription patterns.

Execution evidence (2026-05-22):
1. Scope vectors/parity are green in canonical run:
   - `scope.vector_count=4`
   - `scope.dotnet.mapped_record_count=4`
   - `scope.parity_mismatch_count=0`
2. Runtime scoped filtering counters are instrumented in the Rust server runtime:
   - `activesync_filtered_nodes_total`
   - `activesync_filtered_bytes_total`
   - `activesync_filtered_pack_dropped_total`
3. Conformance parity artifact now emits scoped runtime metric rollups:
   - `scope.runtime_metrics.rust.filtered_incoming_total`
   - `scope.runtime_metrics.rust.filtered_nodes_total`
   - `scope.runtime_metrics.rust.filtered_nodes_kept_total`
   - `scope.runtime_metrics.rust.filtered_pack_dropped_total`
4. Bounded catch-up checks for large-room joins are enforced with deterministic fallback:
   - budget env vars: `ACTIVESYNC_SCOPE_MAX_FILTERED_CATCHUP_NODES`, `ACTIVESYNC_SCOPE_MAX_FILTERED_CATCHUP_BYTES`
   - budget exceed path drops filtered catch-up payload and increments `activesync_filtered_pack_dropped_total{reason="budget_exceeded"}`

## Phase B (Week 2-3): Snapshot + Replay Hardening v1

Status:
1. In progress (slice 1 complete: drill script + CI wiring + runbook guidance).
2. Local nightly-equivalent evidence pass recorded (2026-05-22); awaiting scheduled GitHub nightly artifact proof.

Goal:
1. Make compaction/replay operations operationally predictable and auditable.

Deliverables:
1. Compaction policy contract:
   - compaction trigger guidance (manual + scheduled)
   - replay truncation watermark semantics
2. Snapshot restore verification path:
   - deterministic hash verification on restore
   - explicit restore diagnostics in logs
3. CI replay/restore drill:
   - snapshot restore + forward replay + hash equality assertion

Exit criteria:
1. Deterministic restore drill passes in CI.
2. Operator runbook section for compaction cadence and rollback path is published.

Execution evidence (2026-05-22, slice 1):
1. Deterministic drill script added: `docs/acceptance/Run-SnapshotRestoreDrill.ps1`.
2. Drill assertions include both required checks:
   - `assertions.snapshot_restore_forward_replay`
   - `assertions.snapshot_hash_equality`
3. Canonical nightly workflow wiring added in `.github/workflows/authz-conformance-nightly.yml`:
   - runs the drill in `canonical-admin-command-only`
   - uploads `docs/acceptance/snapshot-restore-drill.json` with canonical artifacts
4. Operator guidance published in `docs/deployment.md` under "Compaction Cadence And Replay Truncation (Phase B)".
5. Local deterministic validation passed:
   - `status=pass`
   - `assertions.snapshot_restore_forward_replay=true`
   - `assertions.snapshot_hash_equality=true`
6. Local nightly-equivalent pipeline pass includes canonical + supplemental + CAPCOMP + snapshot drill artifacts:
   - canonical parity: `status=pass`, `scope.parity_mismatch_count=0`
   - snapshot drill: `status=pass`
   - CAPCOMP supplemental parity: `status=pass`, `capcomp.promotion_signal=supplemental_pass`
   - promotion readiness: `streak.gate_verdict=NOT_EVALUATED` (benchmark evidence not yet provided)

## Phase C (Week 3-4): Identity Continuity + Rotation v1

Status:
1. Complete (2026-05-22; slice 1 + slice 2 complete).

Goal:
1. Define and implement minimum actor continuity semantics without full federation.

Deliverables:
1. Identity continuity contract doc:
   - actor continuity statement format
   - overlap window semantics
   - revocation behavior
2. Host admission validation for rotated identity continuity proof.
3. Conformance vectors:
   - continuity success during overlap
   - reject after overlap expiry
   - reject revoked predecessor key

Exit criteria:
1. Continuity vectors pass in canonical host slices.
2. SDK/host docs include concrete migration flow for device switch and key rotation.

Execution evidence (2026-05-22, slice 1):
1. Identity continuity contract doc published: `docs/IDENTITY_CONTINUITY_V1_CONTRACT.md`.
2. Rust host admission seam now performs optional continuity-v1 checks when `token.continuity` is present:
   - predecessor key format and inequality vs current peer key
   - overlap window enforcement (`overlap_not_after`)
   - revoked predecessor rejection (`revoked_predecessors`)
3. Conformance vectors added:
   - `IDENTITY-CONTINUITY-001`
   - `IDENTITY-CONTINUITY-002`
   - `IDENTITY-CONTINUITY-003`
4. Slice 2 migration flow docs published:
   - SDK flow in `docs/sdk.md` (device switch + key rotation with continuity token provider examples)
   - DotNet host flow in `nodalmerge-host/README.md` (runtime continuity validation and migration rollout steps)
5. DotNet targeted conformance mappings wired in `docs/acceptance/Run-AuthzConformance.ps1`:
   - continuity vectors now map to explicit `RuntimeProtocolTests.Identity_continuity_proof_*` tests
   - canonical dotnet mapped record count now includes continuity vectors (`mapped_record_count=14`)
6. Local nightly-equivalent validation includes continuity mappings and remains green:
   - canonical parity `status=pass`
   - supplemental hybrid/replicated/capcomp parity `status=pass`
   - snapshot drill `status=pass`
   - promotion summary `streak.gate_verdict=NOT_EVALUATED` (benchmark evidence still pending)

## Phase D (Week 1-4, continuous): Conformance + Policy Stabilization

Status:
1. In progress.
2. CAPCOMP streak evidence started (local history: `capcomp_consecutive_passes=1`).
3. Benchmark streak evidence started (`benchmark_status=pass`, `benchmark_consecutive_passes=1`).

Goal:
1. Keep canonical gates healthy while promoting CAPCOMP from supplemental evidence to release-gating readiness.

Deliverables:
1. Nightly gate health dashboard fields remain green:
   - canonical parity
   - CAPCOMP supplemental parity
   - benchmark gate status
2. CAPCOMP + benchmark streak evidence accumulation to 14 consecutive qualified passes.
3. Reason-detail mapping freeze for CAPCOMP reject classes in docs/spec.

Exit criteria:
1. 14 consecutive qualified-run streak evidence is present in history summary artifacts.
2. Promotion recommendation packet is ready (no open parity mismatches).

Evidence cadence policy:
1. Qualified runs are any full local/CI execution that produces canonical parity + CAPCOMP supplemental parity + promotion-readiness artifacts with comparable vector/config scope.
2. Nightly schedule remains the minimum drift sentinel and long-tail environment guard.
3. Integration-triggered qualified runs should be executed after substantial feature slices (for example identity, CAPCOMP, replay/restore, websocket/runtime path changes) and are allowed to advance streak counters.
4. Calendar days are not the unit of progress; consecutive qualified runs are.

Execution evidence (2026-05-22, benchmark-backed qualified run):
1. Full qualified run completed with all required steps passing:
   - artifact: `docs/acceptance/local-nightly-equivalent-qualified-20260522-03.json`
2. Promotion readiness evaluated and streak-eligible:
   - `streak.gate_verdict=PASS_FOR_STREAK`
   - artifact: `docs/acceptance/promotion-readiness-local-qualified-20260522-03.json`
3. History summary shows streak progression has started:
   - `capcomp_consecutive_passes=1`
   - `benchmark_consecutive_passes=1`
   - `combined_consecutive_passes=1`
   - artifact: `docs/acceptance/_history/promotion-readiness-history-summary.json`

Milestone checklist: change types that require a qualified run:
1. Identity/auth token handling changes:
   - continuity-v1 validation, overlap-window semantics, revocation logic, signer normalization, token-provider contract updates.
2. Capability composition (CAPCOMP) changes:
   - profile expansion, DAG traversal/limits, profile-version compatibility, capability flattening payload shape.
3. Replay/restore and compaction changes:
   - snapshot materialization, replay ordering, timeline cutover metadata, restore hash/checkpoint verification paths.
4. Runtime websocket ingress/dispatch changes:
   - hello/subscribe/bootstrap handling, control-plane command routing (`set-policy`, `set-room-key`, `start-tick`, `stop-tick`), deny envelope shaping.
5. Conformance harness/parity wiring changes:
   - vector set edits, mapping table changes, benchmark gate ingestion or threshold logic, promotion readiness summarization/history append logic.

Qualified-run artifact minimum for each milestone-triggering change:
1. Canonical parity pass artifact (`authz-conformance-parity.json`).
2. CAPCOMP supplemental parity pass artifact (`authz-conformance-parity-supp-capcomp-*.json`).
3. Benchmark evidence artifact (`authz-auth-path-benchmark-result*.json`) and benchmark status evaluated in promotion readiness output.
4. Promotion readiness summary/history update with non-`NOT_EVALUATED` gate fields.

## 4. Implementation Backlog (Actionable)

1. Add scope conformance category and vectors in `docs/acceptance/authz-conformance-vectors.json`.
2. Extend `authz-conformance-runner` to execute scope vectors and emit normalized results.
3. Add runtime counters and parity fields for scoped filtering outcomes.
4. Add snapshot restore drill script and wire it into nightly/CI validation job.
5. Publish identity continuity contract doc and add host-side validation seam.
6. Add identity continuity vectors and mapped host tests.
7. Complete CAPCOMP + benchmark streak gate evidence collection.
8. Keep streak continuity by running full qualified evidence flow at each major integration milestone, with nightly as fallback drift coverage.

## 5. Risks and Controls

1. Risk: overbuilding scoped replication beyond immediate need.
   Control: prefix-pattern scope only in v1, defer semantic subscriptions.
2. Risk: replay/compaction hardening drifts into storage redesign.
   Control: keep deterministic checkpoint + restore audit only in this cycle.
3. Risk: identity continuity design blocks workspace delivery.
   Control: v1 overlap-window continuity only; federation deferred.
4. Risk: conformance flakiness hides regressions.
   Control: keep targeted stability slices in canonical pipeline and track failure signatures.

## 6. Definition of Solid (Cycle Completion)

The platform foundation cycle is considered solid when all are true:
1. Scoped replication vectors and runtime metrics are green in CI.
2. Snapshot restore drill passes deterministically and has runbook support.
3. Identity continuity + rotation v1 vectors and host checks are passing.
4. CAPCOMP + benchmark gate streak reaches 14 consecutive passes.
5. No P0/P1/P2/P4 regressions in canonical conformance baseline.
