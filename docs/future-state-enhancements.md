# Future-State Enhancements Tracker

Owner: Platform/runtime
Status: Planned
Last updated: 2026-05-25

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
| FSE-01 | Query/materialization layer | P1 | Planned | Deterministic indexed views and replay-safe projections for large apps | Spec/Auth A-B, Replay Branching A-C |
| FSE-02 | Export/import portable archives | P2 | Planned | Enterprise portability, reproducibility, audit, legal retention | Replay Branching A-C |
| FSE-03 | Runtime scheduler/backpressure semantics | P2 | Planned | Fairness, bounded latency, predictable degradation under load | Spec/Auth A-B |
| FSE-04 | Fine-grained replay subscriptions | P2 | Planned | History UIs and analytics without full-room replay scans | Replay Branching A-C |
| FSE-05 | Presence/session durability polish | P3 | Planned | Better operational continuity and reconnect semantics | Spec/Auth A-B |
| FSE-06 | Schema/version ergonomics | P3 | Planned | Safer long-lived app migrations and compatibility windows | Existing migration docs |
| FSE-07 | Native text acceleration structures | P3 | Planned | Larger-doc performance headroom while preserving determinism | Benchmark baselines |
| FSE-09 | Headless runtime + peer-local persistence adapters | P2 | InProgress | Pod/worker embedding; pluggable peer-local backends (memory/file today; LocalDB/NoSQL/distributed follow-on) | Spec/Auth A-B, Replay Branching A-C |
| FSE-10 | Authority model + parent/child room topology | P1 | Planned | Freezes mainline/worker governance and replayable promotion semantics | Spec/Auth A-B, Replay Branching A-C |
| FSE-08 | Distributed authority/federation semantics | P4 | Deferred | Multi-authority and trust-domain deployments | Identity continuity + lineage foundation |

## 3. Workstream details

### FSE-01 Query/materialization layer

Execution plan: `docs/QUERY_MATERIALIZATION_EXECUTION_PLAN.md`

Near-term checkpoints:

1. freeze query contract and parity mapping
2. add canonical execution and replay parity vectors
3. expose SDK/operator read surfaces

### FSE-02 Export/import portable archives

Execution plan: `docs/EXPORT_IMPORT_PORTABILITY_EXECUTION_PLAN.md`

Planned deliverables:

1. archive manifest schema (versioned)
2. branch/checkpoint export semantics
3. import verification and compatibility checks

Success criteria:

1. deterministic roundtrip hash parity
2. cross-host import compatibility evidence

### FSE-03 Runtime scheduler/backpressure semantics

Planned deliverables:

1. fairness model (room and peer-level)
2. explicit queue/backpressure policy contract
3. overload behavior and recovery runbook

Success criteria:

1. no starvation under mixed workloads
2. bounded memory/latency behavior under pressure tests

### FSE-04 Fine-grained replay subscriptions

Planned deliverables:

1. historical range/cursor subscription contract
2. replay filter semantics aligned with canonical lane
3. protocol and host-core parity vectors

Success criteria:

1. stable cursor progression semantics
2. deterministic filtered replay results

### FSE-05 Presence/session durability polish

Planned deliverables:

1. reconnect continuity semantics (stale/join/leave edge cases)
2. host/ws parity for presence sweep behavior
3. conformance and operator troubleshooting guide

### FSE-06 Schema/version ergonomics

Planned deliverables:

1. migration cookbook templates and anti-pattern checks
2. compatibility-window conventions
3. SDK/operator guidance for staged rollouts

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

1. embedded DB adapter (SQLite schema dedicated to peer-local, distinct from server store)
2. document/KV and distributed cache adapters behind same trait
3. composite memory + durable tier; optional backend registry / host-injected `Arc<dyn PeerLocalPersistence>`
4. SDK IndexedDB adapter; .NET packaging via sidecar or persistence FFI (§4a)

Success criteria:

1. restart/hash parity across adapter backends (memory + file met for LOCAL-PERSIST-001)
2. headless worker parity with browser SDK flows (initial HEADLESS-RUN-001/002 met; IBF/MST depth remains)

### FSE-10 Authority model + parent/child room topology

Execution plan: `docs/AUTHORITY_AND_ROOM_TOPOLOGY_EXECUTION_PLAN.md`
Companion playbook: `docs/MANAGER_WORKER_TOPOLOGY_PLAYBOOK.md`

Planned deliverables:

1. authority role matrix for mainline/child/shared-context room types
2. parent-checkpoint lineage metadata contract for child rooms
3. deterministic child-to-parent promotion command/event pipeline

Success criteria:

1. deterministic promotion acceptance/rejection behavior
2. replay-stable lineage and audit artifacts for room families

### FSE-08 Distributed authority/federation semantics

Planned deliverables (future):

1. trust-domain and authority handoff model
2. federation-safe policy timeline semantics
3. cross-authority convergence and audit vectors

## 4. Recommended sequence

1. FSE-01 Query/materialization (immediately after current two core plans reach Phase C readiness).
2. FSE-02 and FSE-03 in parallel.
3. FSE-04 and FSE-05 next as productization accelerators.
4. FSE-06 and FSE-07 as ongoing hardening.
5. FSE-09 and FSE-10 as productization enablers before broader federation tracks.
6. FSE-08 remains explicitly deferred until earlier invariants are stable.

## 5. Tracking cadence

Update this tracker when:

1. a new execution plan is created
2. a workstream changes status or priority
3. acceptance vectors for a workstream are added or closed
