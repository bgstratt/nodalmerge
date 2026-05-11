# Pre-Host-Extraction Implementation Plan

Purpose: define concrete implementation work to complete before deep host extraction, with GC semantics as the first-class prerequisite and host extraction kept in view.

Scope focus:

1. stabilize GC ownership semantics and contracts
2. define host-neutral orchestration surfaces needed for extraction
3. close operational and protocol documentation gaps that affect implementation safety

Out of scope for this plan:

1. full host extraction implementation
2. full .NET runtime embedding rollout
3. non-websocket transport implementation

---

## 0. Program framing

This project is in stabilization-and-extraction stage.

Top risks to reduce before extraction:

1. GC ownership and liveness ambiguity
2. blob lifecycle invariants not formalized in executable contracts
3. host command/event boundaries not frozen
4. protocol/admin operations not cataloged as implementation references

Primary dependency:

1. `docs/GC_IMPLEMENTATION_PLAN.md` Section 11A gate must be complete before host extraction starts.

---

## 1. Delivery tracks

## Track A: GC Contract Stabilization (highest priority)

Goal: freeze lifecycle ownership semantics and make them executable as shared contracts.

### A1. Freeze semantics in docs (contract language)

Deliverables:

1. finalize authoritative liveness definition
2. finalize delete domain model (`tenant/bucket/prefix` semantics)
3. finalize blob lifecycle state machine and transition invariants
4. finalize pin/lease/staging semantics

Primary files:

1. `docs/delegated-storage-gc.md`
2. `docs/GC_IMPLEMENTATION_PLAN.md`
3. `docs/operations-inventory.md`

Acceptance:

1. no contradictory semantics across the three docs
2. 11A checklist in `GC_IMPLEMENTATION_PLAN.md` marked complete

### A2. Introduce shared GC subsystem crate skeleton

Deliverables:

1. new crate: `activesync-gc` (workspace member)
2. trait contracts moved from docs into compile-checked Rust interfaces
3. state model types (`AssetState`, run mode/status, run records)

Primary files:

1. `Cargo.toml` (workspace member)
2. `activesync-gc/Cargo.toml`
3. `activesync-gc/src/lib.rs`
4. `activesync-gc/src/contracts.rs`
5. `activesync-gc/src/types.rs`

Acceptance:

1. crate builds with no transport/runtime dependencies
2. contracts match docs and are referenced from docs as canonical API

### A3. Implement GC coordinator (policy/orchestration only)

Deliverables:

1. coordinator module with run modes: `DryRun`, `MarkOnly`, `SweepSoft`, `SweepHard`
2. mark -> soft sweep -> hard sweep orchestration
3. eligibility evaluation separated from storage execution

Primary files:

1. `activesync-gc/src/coordinator.rs`
2. `activesync-gc/src/policy.rs`
3. `activesync-gc/src/errors.rs`

Acceptance:

1. coordinator consumes only contracts (no S3/filesystem specifics)
2. decision path test coverage for all run modes

### A4. Integrate server with coordinator via adapter boundary

Deliverables:

1. server runtime calls coordinator through adapters
2. keep existing `BlobPersistence::blob_gc_sweep` path as compatibility shim initially
3. preserve existing CLI behavior

Primary files:

1. `server/src/room.rs`
2. `server/src/main.rs`
3. `server/src/store.rs`

Acceptance:

1. no behavior regression in current server GC operation
2. ability to run coordinator in `DryRun` mode without deletion

### A5. Conformance and failure-injection tests

Deliverables:

1. reusable conformance suite for GC contracts
2. tests for false-delete prevention and live-again cancellation
3. retry/replay idempotency tests

Primary files:

1. `activesync-gc/tests/conformance.rs`
2. `activesync-gc/tests/fault_injection.rs`
3. adapter-specific test modules (where relevant)

Acceptance:

1. required conformance list from `GC_IMPLEMENTATION_PLAN.md` passes
2. hard delete remains disabled by default outside controlled mode

---

## Track B: Host Extraction Prerequisites (non-GC)

Goal: freeze interfaces that host extraction needs so extraction is mechanical, not exploratory.

### B1. Freeze typed command/event model

Deliverables:

1. command/event schema document and Rust draft types
2. mapping rules from websocket messages to typed commands/events
3. host error taxonomy and close/retry mapping

Primary files:

1. `hostedMigrationPlan.md`
2. new doc: `docs/HOST_COMMAND_EVENT_CONTRACT.md`

Acceptance:

1. all currently implemented wire operations map to command/event model
2. no JSON-only assumptions at host boundary

### B2. Protocol operation catalog with examples

Deliverables:

1. wire message catalog with request/response examples
2. field-level notes for required/optional/conditional fields

Primary files:

1. new doc: `docs/protocol.md`
2. references from `docs/operator.md` and `docs/sdk.md`

Acceptance:

1. every ws message in `server/src/ws_handler.rs` is documented
2. message catalog reviewed for implementation parity

### B3. Operator admin operations reference

Deliverables:

1. dedicated operator section for admin/runtime commands:
   - policy operations
   - tick controls
   - compaction controls
   - GC run mode operations

Primary files:

1. `docs/operator.md`

Acceptance:

1. operator can execute all admin operations without reading source code
2. each operation includes expected response and failure behavior

---

## Track C: Readiness gates for extraction kickoff

## Gate G0: Semantics gate

1. A1 complete, docs aligned, Section 11A complete
2. 11A pre-host-extraction checklist complete, including adapter boundary sign-off

## Gate G1: Contract gate

1. A2 complete, contracts compile as code in `activesync-gc`

## Gate G2: Orchestration gate

1. A3 complete, coordinator run modes and eligibility split implemented

## Gate G3: Runtime integration gate

1. A4 complete, server integrated without regressions

## Gate G4: Conformance gate

1. A5 complete, conformance and fault tests green

## Gate G5: Extraction interface gate

1. B1 complete (host command/event model frozen)
2. B2 complete (protocol catalog present)
3. B3 complete (operator admin runbook complete)

Extraction kickoff condition:

1. G0 through G5 must all pass before deep host extraction starts.

---

## 2. PR sequence (concrete)

1. [x] PR-01: semantics freeze edits in GC docs + operations inventory alignment
2. [x] PR-02: add `activesync-gc` crate skeleton and compile-checked types/contracts
3. [x] PR-03: add coordinator run-mode orchestration (`DryRun` and `MarkOnly` first)
4. [x] PR-04: add `SweepSoft`/`SweepHard` flow + eligibility split
5. [x] PR-05: integrate server runtime adapter path (compatibility shim retained)
6. [x] PR-06: add conformance and failure-injection tests
7. [x] PR-07: add `HOST_COMMAND_EVENT_CONTRACT.md` and freeze mapping
8. [x] PR-08: add `docs/protocol.md` with request/response examples
9. [x] PR-09: expand `docs/operator.md` admin operation reference
10. [x] PR-10: extraction kickoff review and sign-off against G0-G5

## 2.1 Progress snapshot

Completed now:

1. GC semantics freeze in docs (A1 / G0 semantics portion).
2. New `activesync-gc` workspace crate with contracts/types/errors.
3. Coordinator implementation for `DryRun`, `MarkOnly`, `SweepSoft`, `SweepHard`.
4. Conformance and fault-injection test suites in `gc/tests/`.
5. Server compatibility integration path: mark-only coordinator preflight wired into blob sweep loop, legacy delete path retained.
6. Host command/event contract baseline added in `docs/HOST_COMMAND_EVENT_CONTRACT.md`.
7. Protocol request/response catalog added in `docs/protocol.md`.
8. Operator runbook expanded with admin operation reference.

Remaining before extraction kickoff:

1. None. Plan complete; proceed to host extraction workstream implementation.

## 2.2 PR-10 extraction kickoff review record

Review date: 2026-05-09

Gate decisions:

1. G0 pass: semantics gate complete (A1 docs aligned, 11A complete).
2. G1 pass: contracts gate complete (`activesync-gc` contracts/types compile and are runtime-neutral).
3. G2 pass: orchestration gate complete (coordinator run modes and safety behavior implemented).
4. G3 pass: runtime integration gate complete (server compatibility integration merged without behavioral regression).
5. G4 pass: conformance gate complete (conformance + fault tests green for GC subsystem).
6. G5 pass: extraction interface gate complete (host command/event contract + protocol catalog + operator admin reference added).

Sign-off outcome:

1. G0-G5 satisfied.
2. Host extraction kickoff approved.

---

## 3. Minimal data model and invariants to enforce

Required invariants:

1. no hard delete without eligibility + grace + safety checks
2. mark pass is authoritative over delta fast-path
3. pinned hashes must never transition to deleted
4. live-again hash cancels pending delete deterministically
5. deletion decisions are domain-aware, not implicitly room-local

Recommended state transitions:

1. Uploading -> Live
2. Live -> Grace
3. Grace -> SweepCandidate
4. SweepCandidate -> Deleted
5. any state -> Pinned (policy override)
6. Grace/SweepCandidate -> Live (on re-reference)

---

## 4. Metrics and observability requirements

Add/confirm metric surfaces before extraction:

1. gc_runs_total by mode/status
2. gc_marked_total
3. gc_pending_total
4. gc_deleted_total
5. gc_skipped_pinned_total
6. gc_error_total
7. gc_live_again_cancellations_total

These metrics should be adapter-agnostic labels first, backend-specific labels optional.

---

## 5. Definition of done for this plan

This pre-extraction implementation plan is complete when:

1. GC contracts are frozen and executable in a shared subsystem
2. runtime integration preserves existing behavior and adds deterministic run-mode control
3. conformance tests cover false-delete and replay/retry safety
4. host command/event contract is frozen
5. protocol and operator docs are complete enough for implementation without source spelunking
6. G0-G5 extraction kickoff gates are signed off

At that point, host extraction can proceed with significantly reduced lifecycle risk.
