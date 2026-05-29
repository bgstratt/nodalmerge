# Future-State Enhancements Tracker

Owner: Platform/runtime
Status: Active
Last updated: 2026-05-28

Purpose: track worthwhile post-core enhancements beyond replay branching and speculative-vs-authoritative semantics.

Combined execution ordering reference:

1. `docs/roadmap.md`

## 1. Prioritization rubric

Priority levels:

1. P1: high strategic value and near-term platform impact
2. P2: meaningful scale/productization impact
3. P3: medium-term performance or ecosystem depth
4. P4: future/advanced topology

## 2. Enhancement backlog

| ID | Area | Priority | Status | Why it matters | Depends on |
|---|---|---|---|---|---|
| FSE-01 | Query/materialization layer | P1 | InProgress | Deterministic indexed views and replay-safe projections for large apps | Spec/Auth A-B, Replay Branching A-C |
| FSE-02 | Export/import portable archives | P2 | InProgress | Enterprise portability, reproducibility, audit, legal retention | Replay Branching A-C |
| FSE-03 | Runtime scheduler/backpressure semantics | P2 | InProgress | Fairness, bounded latency, predictable degradation under load | Spec/Auth A-B |
| FSE-04 | Fine-grained replay subscriptions | P2 | InProgress | History UIs and analytics without full-room replay scans | Replay Branching A-C |
| FSE-05 | Presence/session durability polish | P3 | Complete (current scope) | Better operational continuity and reconnect semantics | Spec/Auth A-B |
| FSE-06 | Schema/version ergonomics | P3 | Complete (current scope) | Safer long-lived app migrations and compatibility windows | Existing migration docs |
| FSE-07 | Native text acceleration structures | P3 | Planned | Larger-doc performance headroom while preserving determinism | Benchmark baselines |
| FSE-09 | Headless runtime + peer-local persistence adapters | P2 | Complete (current scope) | Pod/worker embedding; pluggable peer-local backends (memory/file today; LocalDB/NoSQL/distributed follow-on) | Spec/Auth A-B, Replay Branching A-C |
| FSE-10 | Authority model + parent/child room topology | P1 | Complete (current scope) | Freezes mainline/worker governance and replayable promotion semantics | Spec/Auth A-B, Replay Branching A-C |
| FSE-08 | Distributed authority/federation semantics | P4 | Deferred | Multi-authority and trust-domain deployments | Identity continuity + lineage foundation |

Reconciliation snapshot (2026-05-28):

1. FSE-01 has completed Phase A-E declared slices (including replay-to-checkpoint + paginated digest continuity baselines) and is now in hardening/extension mode.
2. FSE-02 has completed Phase A-D declared slices (contracts, runtime adapters, parity vectors, alert/runbook wiring) and is now in post-closeout monitoring mode.
3. FSE-03 has delivered bounded/fair query projection build backpressure plus observability; broader scheduler semantics remain open.
4. FSE-10 has delivered create/describe/list/propose/validate/apply topology flows, restart durability, `.NET` native parity, and large-family Phase E baseline; advanced governance/federation tracks remain open.

## 3. Workstream details

### FSE-01 Query/materialization layer

Execution plan: `docs/QUERY_MATERIALIZATION_EXECUTION_PLAN.md`

Delivered slices:

1. contract freeze and parity mapping across host/core/server/ws
2. canonical execution and replay parity vectors
3. SDK/operator read surfaces and rejection triage coverage
4. Phase E benchmark baselines including end-to-end replay-to-checkpoint pagination digest continuity (`query-phasee-benchmark-baseline-run03`)

Follow-on:

1. extended selector semantics (`hash`/`frontier`) in live server lane under load
2. larger-cardinality/mixed-workload benchmark lanes and SLO ratcheting
3. tighter SDK ergonomics around checkpoint targeting and result paging helpers

### FSE-02 Export/import portable archives

Execution plan: `docs/EXPORT_IMPORT_PORTABILITY_EXECUTION_PLAN.md`

Delivered slices:

1. archive manifest schema (versioned)
2. branch/checkpoint export semantics
3. import verification and compatibility checks
4. file/object parity baseline and alert-threshold wiring
5. operator runbook + escalation drills + closeout signoff

Success criteria:

1. deterministic roundtrip hash parity
2. cross-host import compatibility evidence

Follow-on:

1. weekly post-closeout monitoring and threshold recalibration evidence cadence
2. optional archive drill automation when operator priorities justify it

### FSE-03 Runtime scheduler/backpressure semantics

Delivered slices:

1. query projection build guardrails (`max_rows`, `max_inflight`) plus fair bounded queue (`NODALMERGE_QUERY_BUILD_MAX_QUEUE`)
2. deterministic rejection contract (`reject.query_backpressure`) plus metrics for queue/inflight/wait
3. topology promotion queue guardrails and baseline artifact

Open deliverables:

1. broader fairness model beyond query/promotion slices (cross-room and mixed control-plane contention)
2. explicit shared scheduler policy doc covering degradation order under multi-lane load
3. overload behavior and recovery runbook consolidation across query/topology/archive

Success criteria:

1. no starvation under mixed workloads
2. bounded memory/latency behavior under pressure tests

### FSE-04 Fine-grained replay subscriptions

Delivered slices:

1. `replay.read-range` request/result/reject contract across server ingress + protocol docs
2. deterministic cursor progression and reject vectors in server lanes
3. CLI surface (`nodalmerge query replay-read-range`) and SDK helper (`sdk.query.readReplayRange`)
4. operator + SDK guidance for replay paging and reject handling

Success criteria:

1. stable cursor progression semantics
2. deterministic filtered replay results

Current status:

1. implementation slices are delivered; track remains in hardening/extension mode for future selector depth and performance tuning.

### FSE-05 Presence/session durability polish

Delivered slice (Phase A):

1. reconnect continuity contract documented in `protocol.md` (`peer-joined`/`peer-left` first-join/last-leave invariants)
2. websocket runtime now tracks duplicate sessions per pubkey and suppresses duplicate join/early leave broadcasts
3. initial server conformance lane added for duplicate-session reconnect overlap
4. operator troubleshooting matrix expanded for presence continuity (`leave` vs `stale` causes and reconnect checks)
5. host/ws parity vectors added for lease-expiry edge windows:
   - host-core: refreshed lease boundary and stale-then-close sequencing
   - websocket: duplicate-session token-expiry window without early `peer-left`

Remaining deliverables:

1. none in current scope

Current status:

1. FSE-05 is complete for current planned scope (reconnect continuity, lease/sweep parity vectors, malformed lease rejection taxonomy).

### FSE-06 Schema/version ergonomics

Delivered slice (baseline):

1. canonical migration cookbook published: `docs/MIGRATION_COOKBOOK.md`
2. compatibility-window and unsupported-window handling guidance wired into `docs/sdk.md` and `docs/operator.md`
3. anti-pattern CI/PR checklist artifact published: `docs/MIGRATION_ANTI_PATTERNS_CHECKLIST.md`

Remaining deliverables:

1. none in current scope

Current status:

1. FSE-06 is complete for current planned scope (cookbook, compatibility conventions, staged rollout guidance, anti-pattern checklist).

### FSE-07 Native text acceleration structures

Planned deliverables:

1. benchmark-driven design decision record
2. optional accelerated index structure behind feature gate
3. determinism and memory-constrained validation

### FSE-09 Headless runtime + peer-local persistence adapters

Execution plan: `docs/HEADLESS_RUNTIME_PERSISTENCE_EXECUTION_PLAN.md` (§4a packaging, §4b extensible backends)

Delivered (2026-05-27 slice):

1. `nodalmerge-runtime-local`: `PeerLocalPersistence`, memory + file adapters, `PersistBackendKind` config, `LOCAL-PERSIST-001` vectors.
2. `nodalmerge-headless` worker: WS handshake + pack → peer-local log; env-configurable backend.

Follow-on (configurable / plugin backends):

1. embedded DB adapter (SQLite schema dedicated to peer-local, distinct from server store) — hardening vectors now include read-only, version-skew, and corruption classification coverage
2. document/KV and distributed cache adapters behind same trait
3. composite memory + durable tier; optional backend registry / host-injected `Arc<dyn PeerLocalPersistence>` (initial pilot complete; hardening vectors now include stale-tail contention + blob fallback after restart)
4. SDK IndexedDB adapter; .NET packaging via sidecar or persistence FFI (§4a) (initial slices complete; production hardening remains)

Delivered hardening closeout (current scope):

1. production hardening checklist published: `docs/PEER_LOCAL_PRODUCTION_HARDENING_CHECKLIST.md`
2. composite tier hardening vectors include stale-tail contention and durable blob fallback after restart

Current status:

1. FSE-09 is complete for current scope; follow-on adapter ecosystem expansion remains optional next-cycle work.

Success criteria:

1. restart/hash parity across adapter backends (memory + file met for LOCAL-PERSIST-001)
2. headless worker parity with browser SDK flows (initial HEADLESS-RUN-001/002 met; IBF/MST depth remains)

### FSE-10 Authority model + parent/child room topology

Execution plan: `docs/AUTHORITY_AND_ROOM_TOPOLOGY_EXECUTION_PLAN.md`
Companion playbook: `docs/MANAGER_WORKER_TOPOLOGY_PLAYBOOK.md`

Delivered slices:

1. authority role matrix for mainline/child/shared-context room types
2. parent-checkpoint lineage metadata contract for child rooms
3. deterministic child-to-parent promotion command/event pipeline
4. native `.NET` topology parity via host-core/FFI routing
5. large-family Phase E baseline + retention policy tuning artifact

Success criteria:

1. deterministic promotion acceptance/rejection behavior
2. replay-stable lineage and audit artifacts for room families

Follow-on:

1. additional large-family stress profiles (multi-parent/multi-promotion concurrency) — baseline lane now includes FSE-10.A single-winner concurrent apply contention
2. richer governance policy templates and operator drill depth — FSE-10.B baseline delivered (`TOPOLOGY_GOVERNANCE_POLICY_TEMPLATES.md`, `TOPOLOGY_OPERATOR_DRILL_RUNBOOK.md`)
3. eventual federation preconditions (still gated by FSE-08 deferment) — FSE-10.C baseline delivered (`FEDERATION_PRECONDITIONS_CHECKLIST.md`)

Current status:

1. FSE-10 is complete for current non-federated scope; federation implementation remains explicitly deferred to FSE-08.

### FSE-08 Distributed authority/federation semantics

Planned deliverables (future):

1. trust-domain and authority handoff model
2. federation-safe policy timeline semantics
3. cross-authority convergence and audit vectors

## 4. Recommended sequence

1. FSE-01 Query/materialization (immediately after current two core plans reach Phase C readiness).
2. FSE-02 and FSE-03 in parallel.
3. FSE-04 hardening alongside FSE-06 ergonomics follow-on.
4. FSE-07 as benchmark-triggered hardening.
5. FSE-09 and FSE-10 as productization enablers before broader federation tracks.
6. FSE-08 remains explicitly deferred until earlier invariants are stable.

## 5. Tracking cadence

Update this tracker when:

1. a new execution plan is created
2. a workstream changes status or priority
3. acceptance vectors for a workstream are added or closed

## 6. Implementation plan for unfinished portions

Scope for this cycle (recommended):

1. prioritize FSE-04 hardening
2. keep FSE-05 and FSE-06 in maintenance mode (complete for current scope)
3. defer FSE-07 to benchmark-triggered activation
4. keep FSE-08 deferred

### 6.1 FSE-04 Fine-grained replay subscriptions (next execution slice)

Why now:

1. query/topology/archive control planes are stable enough to add cursored replay semantics safely
2. immediate value for history UIs and analytics consumers

Implementation steps:

1. freeze wire contract for replay range/cursor selectors (request + result + reject taxonomy)
2. add host-core/server parity vectors for deterministic cursor progression and filtered replay digest stability
3. add CLI and SDK read surfaces for replay-range fetch
4. document operator triage for replay-cursor mismatch or invalid selector rejects

Completion gate:

1. deterministic cursor progression under repeated reads
2. matching replay-filtered results across host-core/server lanes

### 6.2 FSE-05 Presence/session durability polish (complete for current scope)

Why now:

1. headless and mixed deployment topologies are now realistic and need stronger reconnect semantics

Implementation steps:

1. define stale/join/leave edge-case contract and timeout invariants
2. align host/ws parity behavior for presence sweep and reconnect continuity
3. add targeted conformance vectors for stale peer cleanup, reconnect replacement, and lease-expiry paths
4. extend `docs/operator.md` troubleshooting matrix with presence-specific reject/error classes

Completion gate:

1. deterministic join/update/leave transitions across reconnect edge cases
2. parity vectors green for host/ws paths

### 6.3 FSE-06 Schema/version ergonomics (complete for current scope)

Why now:

1. portability and mixed-client operation need clearer compatibility-window/operator guidance

Implementation steps:

1. publish migration cookbook templates (forward-only, dual-read, rollback-safe)
2. standardize compatibility-window language across query/archive/topology docs
3. add SDK/operator staged-rollout guidance with explicit "unsupported-window" handling patterns

Completion gate:

1. one canonical migration cookbook referenced by `sdk.md`, `operator.md`, and portability docs

### 6.4 FSE-07 Native text acceleration structures

Why later:

1. correctness and control-plane hardening currently deliver more value than index acceleration

Trigger conditions:

1. repeated benchmark evidence that text workloads exceed target latency/memory envelopes
2. agreed feature-gate + deterministic parity strategy

### 6.5 FSE-09 and FSE-10 follow-on guardrails

Next hardening (optional, after FSE-04/05 start):

1. additional headless backend adapters beyond current pilot set
2. topology multi-family stress lanes and promotion queue contention baselines
