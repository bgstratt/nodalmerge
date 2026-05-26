# Authority and Room Topology Execution Plan

Owner: Runtime + host + operator streams
Status: Planned
Last updated: 2026-05-25

Companion guidance:

1. `docs/MANAGER_WORKER_TOPOLOGY_PLAYBOOK.md`

## 1. Why this plan exists

ActiveSync already has per-room authority controls and replay/fork direction, but multi-room systems need a frozen contract for:

1. what authority means in each room role
2. how parent and child rooms are linked for replay and audit
3. how child outputs are promoted into parent/mainline deterministically

Without this contract, deployments risk policy drift, weak auditability, and non-replayable orchestration behavior.

## 2. Scope and non-goals

In scope:

1. authority role model per room type
2. parent/child room linkage metadata contract
3. promotion/merge-to-mainline workflow contract
4. replay and audit semantics for room families
5. host-core and websocket parity requirements

Out of scope (v1):

1. multi-authority federation across trust domains
2. automatic semantic conflict resolution between unrelated child outputs
3. product-specific workflow DSL design

## 3. Authority model (v1)

### 3.1 Authority definitions

1. Server authority: runtime validates policy/capability and controls canonical acceptance in a room.
2. Owner authority: designated identity/service allowed to change control-plane policy for a room family.
3. Worker authority: scoped ability to emit intent/canonical updates only within assigned room namespaces.

### 3.2 Room role taxonomy

1. Mainline room: canonical product/system truth.
2. Child work room: bounded task stream for an agent/user/workstation.
3. Shared context room: read-mostly context/memory distribution for many workers.

### 3.3 Core rule

Authority remains per room, but room family governance is enforced through explicit linkage and promotion contracts.

## 4. Parent/child linkage contract (v1)

Parent/child should not rely on implicit naming only.

Each child room records immutable lineage metadata:

1. `parent_room_id`
2. `parent_checkpoint`:
   - parent frontier
   - parent canonical hash
   - optional parent policy timeline hash
3. `child_purpose` (task/workstream code)
4. `created_by`
5. `created_at_hlc`
6. `promotion_policy_id`

Recommended linkage principle:

1. tie child to parent checkpoint/hash, not to ad-hoc latest state assumptions
2. promote child outputs through explicit payloads with provenance

## 5. Promotion contract (child -> parent)

### 5.1 Conceptual operations

1. `CreateChildRoomFromParentCheckpoint`
2. `DescribeRoomLineage`
3. `ProposePromotion`
4. `ValidatePromotion`
5. `ApplyPromotion`

### 5.2 Promotion invariants

1. Promotion payload includes source child room id and child checkpoint hash.
2. Parent-side validator checks compatibility against declared parent checkpoint lineage.
3. Applied promotion emits canonical audit metadata (who, when, from where, what hash set).
4. Replay can reconstruct parent state including promotion history deterministically.

## 6. Replay and audit semantics

1. Replaying parent room at checkpoint C must recover the same accepted promotion artifacts.
2. Child replay remains independent and immutable after promotion.
3. Parent replay does not require loading full child DAG, only promotion artifact references and declared hashes.
4. Optional deep-audit mode can dereference child artifacts for full provenance trails.

## 7. Phased implementation

### Phase A - Contract freeze

Deliverables:

1. authority terminology and role matrix
2. room lineage metadata schema
3. promotion command/event drafts

Acceptance criteria:

1. approved by runtime, host, and operator maintainers
2. no ambiguity about parent checkpoint semantics

### Phase B - Runtime lineage metadata

Deliverables:

1. child room creation path with required parent-checkpoint metadata
2. room lineage read/describe operations
3. compatibility notes for existing rooms without lineage metadata

Acceptance criteria:

1. new child rooms always carry lineage metadata
2. lineage queries are deterministic and replay-stable

### Phase C - Promotion pipeline

Deliverables:

1. propose/validate/apply promotion flow in host-core + server mapping
2. deterministic rejection taxonomy for invalid promotions
3. audit artifact persistence and retrieval

Acceptance criteria:

1. promotion pass/fail behavior is deterministic under reconnect/replay
2. parent canonical hash parity vectors pass

### Phase D - CLI/operator workflows

Deliverables:

1. CLI commands for child creation, lineage inspect, promotion propose/apply
2. runbooks for manager-agent and distributed worker topologies
3. metrics for promotion throughput, rejection class, and lag

Acceptance criteria:

1. headless pod workflows run without browser SDK dependency
2. operational drills pass for restart and retry scenarios

### Phase E - Hardening and scale

Deliverables:

1. lineage index optimization and retention policy
2. large-room-family benchmark baselines
3. backpressure and fairness checks for promotion queues

Acceptance criteria:

1. performance and durability SLOs are documented and repeatable
2. no replay determinism regressions in room-family scenarios

## 8. Conformance vectors

1. `AUTH-ROOM-001`: unauthorized control-plane change rejected deterministically
2. `AUTH-ROOM-002`: child room missing parent checkpoint metadata rejected
3. `AUTH-ROOM-003`: valid promotion yields identical parent canonical hash across peers
4. `AUTH-ROOM-004`: invalid promotion lineage rejected with stable reason class
5. `AUTH-ROOM-005`: parent replay reconstructs accepted promotions deterministically
6. `AUTH-ROOM-006`: lineage metadata survives snapshot/compaction/rebuild

## 9. Risks and mitigations

1. Risk: authority semantics overlap with speculative/canonical lifecycle terms.
   Mitigation: reuse existing rejection taxonomy and lane terminology from spec/auth plan.
2. Risk: child linkage bloat.
   Mitigation: compact lineage metadata plus optional deep-audit dereference mode.
3. Risk: operational complexity.
   Mitigation: opinionated CLI workflows and explicit runbook patterns.

## 10. Recommended sequencing

1. Run after Spec/Auth Phase A-B contract freeze.
2. Run in parallel with headless runtime/persistence Phase B-C.
3. Gate broad manager/worker pod rollout on Phase C completion.
4. Use Phase D-E for productization and scale hardening.
