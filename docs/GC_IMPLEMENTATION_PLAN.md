# ActiveSync GC Implementation Plan (Delegated CAS + DAG)

This document is the implementation plan for reusable ActiveSync garbage
collection of externally stored CAS blobs referenced from DAG-visible state.

It complements `docs/delegated-storage-gc.md`:

1. `delegated-storage-gc.md` defines contracts/interfaces.
2. This document defines execution order, rollout, validation, and guardrails.

It is also a prerequisite input to `hostedMigrationPlan.md`:

1. Host extraction starts only after the pre-host-extraction gate (Section 11A) is complete.

## 0. Scope and non-goals

Scope:

1. Reclaim externally stored CAS blobs (S3/R2/MinIO/filesystem blob stores) when no longer live.
2. Keep ActiveSync core product-agnostic and backend-agnostic.
3. Support optional datastore adapters via separate modules/crates.
4. Define GC contracts before host-runtime extraction so embedding hosts (.NET/server/WASM) share one lifecycle model.

Non-goals:

1. Do not delete immutable DAG history/nodes.
2. Do not hard-code product selectors, room naming, tenancy semantics, or admin policy in core.
3. Do not require `ListBucket` for daily safe GC.
4. Do not couple GC semantics to WebSocket/WebRTC or any specific transport/runtime.

Important model:

1. Logical removal happens by new DAG operations that remove/replace blob references.
2. Physical blob deletion happens later via GC after liveness and safety checks.

Architecture positioning model:

1. CRDT core defines what logically exists (reachability truth).
2. Storage layer defines physical retention semantics.
3. GC enforces retention semantics using storage/liveness contracts.
4. Transport only moves sync messages and must not alter GC correctness.

## 1. Architecture and ownership model

1. ActiveSync provides reusable GC contracts and orchestration.
2. Products provide policy configuration and selector mapping through adapters/config.
3. Datastore/object-store details are isolated in optional adapters.

Recommended deployment options:

1. Generic ActiveSync GC worker process using product-neutral contracts.
2. External scheduler calling ActiveSync internal GC APIs/contracts.

Portability rules:

1. Core coordinator depends only on interfaces.
2. Adapters own backend specifics (Mongo/Postgres/MySQL/MSSQL/Oracle/KV).
3. Policy hooks remain injectable per product.

Runtime/transport neutrality requirement:

1. GC coordinator behavior must be identical regardless of host runtime (tokio/.NET/in-process scheduler).
2. GC inputs are state/liveness contracts, never socket/session implementation details.
3. Future transports (HTTP streaming, WebTransport, UDS, in-process channels, brokered adapters) must not require GC model changes.

## 2. Data model (logical contracts)

Logical records used by GC (storage-neutral):

1. `cas_blob_inventory`
2. `cas_gc_runs`
3. `hash_reference_state`
4. `cas_delete_queue`
5. Optional `cas_admin_pins`

Reference: concrete trait and type contracts are specified in
`docs/delegated-storage-gc.md`.

## 3. Liveness and deletion semantics

### A1 frozen decisions (normative)

1. Authoritative liveness = reachable from authoritative state within GC domain.
2. Mark pass is source of truth; incremental deltas are acceleration path only.
3. GC eligibility is domain-scoped (tenant/bucket/prefix), not intrinsically room-scoped.
4. Pin and lease protections override delete eligibility.
5. Room-scoped runtime sweep APIs are compatibility shims during migration and must not redefine semantics.

Primary liveness model:

1. Incremental reference deltas are primary signal (`oldHashes` vs `newHashes`).
2. Mark/sweep is repair/audit and drift correction.
3. Deletion decisions are made over full delete domain, not per-room in isolation.

Deletion domain rule:

1. One GC domain per `(tenantId, bucket, keyPrefix)` where applicable.
2. Any live reference in the domain keeps hash live.

Reference ownership rule:

1. Liveness is derived from authoritative state reachability, not transport observations.
2. Hosts may provide incremental reference deltas (`oldHashes/newHashes`) but mark pass remains authoritative repair path.
3. GC must behave safely under retry/replay and eventually consistent adapters.

Hard-delete preconditions:

1. Hash is pending and older than grace window.
2. Hash is not pinned.
3. Ref-state still indicates zero.
4. Authoritative live check still reports not live.
5. Optional HEAD check passes policy gate.

## 4. Sweep algorithm

Incremental path:

1. Compute `deltaAdded = newHashes - oldHashes`.
2. Compute `deltaRemoved = oldHashes - newHashes`.
3. For `deltaAdded`: increment refcount, update lastReferencedAt, dequeue pending delete.
4. For `deltaRemoved`: decrement (floor at zero); enqueue delete when zero and not pinned.

Repair path:

1. Mark: collect live hashes from `LiveHashSource`.
2. Soft sweep: move unmarked non-pinned entries to pending.
3. Hard sweep: delete pending entries past grace after safety rechecks.
4. Reconcile drift between ref-state and authoritative live set.

## 5. Safety controls and rollout

Feature flags / modes:

1. `DryRun`
2. `MarkOnly`
3. `SweepSoft`
4. `SweepHard`

Operational controls:

1. `graceWindow`: default 24h prod / 1h staging.
2. `maxDeletesPerRun` cap.
3. `requireHeadBeforeDelete` rollout gate.
4. Circuit breaker / kill switch for hard delete.

Rollout phases:

1. Phase 0: inventory + run ledger + DryRun validation.
2. Phase 0.5: incremental reference tracking + queue (no hard deletes).
3. Phase 1: soft sweep only.
4. Phase 2: limited hard delete (small cap).
5. Phase 3: full hard delete with monitoring.

## 6. Implementation backlog (ActiveSync-first)

Execution order:

1. B0 - Contract freeze and package boundaries (required gate before host extraction/integration).
2. B1 - Coordinator skeleton (no deletes).
3. B2 - Delta engine and queue semantics with idempotency.
4. B3 - Adapter implementation (Mongo first, contract-neutral shape).
5. B4 - Hard delete path + safety controls.
6. B5 - Conformance + fault-injection tests.
7. B6 - Demo validation + operational readiness.

B0 deliverables:

1. Product-neutral API surface finalized.
2. Package map documented:
   1. core contracts/coordinator package,
   2. optional adapter packages.
3. Ownership semantics frozen: "core defines truth, storage defines retention, GC enforces retention".
4. Pre-host extraction checklist signed off (see Section 11A).

B3 deliverables:

1. First adapter (Mongo) passes conformance suite.
2. No backend types leak into shared contracts.

B6 deliverables:

1. Demo validation report.
2. Baseline dashboards/alerts.
3. Go/no-go decision record.

## 7. Conformance and validation gates

Required behavior tests:

1. Refcount floor at zero.
2. Idempotency under replay/retry.
3. Grace window enforcement.
4. Pinned hash protection.
5. Drift repair correctness.
6. Queue retry/backoff and dead-letter handling.

Track-B gate (must pass before product integration work):

1. Demo shows shared hash survives until last reference removed.
2. Zero-ref hash is queued and deleted only after grace.
3. Live-again hash cancels pending delete.
4. No false deletes under retry/replay scenarios.
5. Coupling review passes (no product-specific leakage into core).

## 8. Coupling guardrails (ActiveSync)

Forbidden in ActiveSync core/shared crates:

1. Product selector defaults (for example specific field-name constants).
2. Product-specific room naming or owner semantics.
3. Backend-specific types in generic interfaces.
4. Compile-time dependency on any product SDK/app packages.

Required in ActiveSync core/shared crates:

1. Product-neutral naming and contracts.
2. Config-driven selector/policy hooks.
3. Adapter-based backend portability.

## 9. Product integration boundary

Product-side responsibilities (outside ActiveSync core):

1. Selector mapping and policy values.
2. Scheduler wiring in product deployment.
3. Product migrations/backfills and operational runbooks.

ActiveSync responsibilities:

1. Reusable GC contracts, coordinator, and optional adapters.
2. Stable integration surface for products to plug in policy/config.

## 10. Go / no-go criteria

Go:

1. Seven consecutive DryRun/Soft runs with no false-positive protected deletes.
2. Stable mark coverage and low drift mismatch variance.
3. Canary hard-delete runs with zero restore incidents.
4. Dashboards and alerts active.

No-go:

1. Protected/pinned hash reaches pending delete.
2. Authoritative live-hash source unavailable.
3. Error rate exceeds threshold during canary.
4. Missing deletion audit trail.

## 11. Initial execution checklist

1. Confirm defaults for grace window, caps, and run modes.
2. Freeze contract package boundaries.
3. Implement idempotent delta hook before any hard-delete enablement.
4. Enable queue consumer in observe-only mode first.
5. Complete coupling review before merging core changes.

## 11A. Pre-Host-Extraction Gate (Required)

These items must be complete before starting host extraction/integration work:

1. [x] GC liveness/deletion domain semantics approved.
2. [x] Contract surface approved for `LiveHashSource`, inventory, run ledger, and object-store operations.
3. [x] Incremental delta semantics approved (`oldHashes/newHashes`, idempotency, retry behavior).
4. [x] Safety defaults approved (`graceWindow`, delete caps, HEAD gate, run modes).
5. [x] Conformance tests defined for false-delete prevention and live-again cancellation.
6. [x] Adapter boundary review completed (no transport/runtime assumptions in GC contracts).

11A status:

1. Semantics and contract freeze complete in docs.
2. Executable conformance and fault tests implemented in `gc/tests/`.
3. Compatibility adapter integration path implemented in `server` (mark-only coordinator preflight + legacy sweep fallback).
4. Adapter boundary review sign-off complete.

11A adapter boundary review notes:

1. `activesync-gc` contracts use trait interfaces and `std` types only (`SystemTime`, iterators, strings, hashes-as-values), with no Tokio/Axum/WebSocket imports.
2. Coordinator orchestration is runtime-neutral (`run_once`) and does not own sockets, tasks, or transport lifecycles.
3. Server integration is via a compatibility adapter (`server/src/gc_adapter.rs`) that translates server room state into contract calls and preserves legacy delete execution in `blob_gc_sweep`.
4. Result: contract package is host-embeddable and transport-agnostic; runtime ownership can move to host without contract breakage.
