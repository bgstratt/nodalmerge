# Manager Worker Topology Playbook

Owner: Runtime + operator streams
Status: InProgress (Wave 2 CLI implemented — `nodalmerge topology` / `run` / `archive` / `query`; see `pre-ai-workspace-integration-closeout.json`)
Last updated: 2026-05-27

## 1. Decision summary

Parent child room relationships should live primarily in hosted authority and room governance layers, not as first-class CRDT semantics in the core engine.

Engine should stay focused on deterministic room-local DAG behavior.
Hosted runtime should own:

1. room family catalogs
2. authority role assignment by room type
3. child to parent promotion policy
4. audit and lineage metadata for operations

## 2. Why this boundary is correct

### 2.1 Engine responsibilities

1. deterministic DAG append replay compaction hashing
2. fork checkpoint and payload primitives
3. canonical and intent lane semantics inside one room

### 2.2 Hosted runtime responsibilities

1. parent child topology definition
2. who can write where and under what policy
3. mapping child outputs into parent aggregates
4. lifecycle of worker rooms and assignment orchestration

This keeps engine reusable and avoids embedding product workflow semantics into core replication.

## 3. Two valid parent child modes

### Mode A: Reference only topology

Parent room tracks child room ids and summary metadata.

Characteristics:

1. parent replay does not depend on child DAG replay
2. child outputs are consumed as external facts or snapshots
3. easiest operational model

Use when parent only needs awareness and indexing, not deterministic convergence from child histories.

### Mode B: Promotion based convergence topology

Child rooms produce explicit promotion artifacts that are validated then applied to parent canonical state.

Characteristics:

1. deterministic acceptance rejection behavior
2. replay stable parent outcomes from accepted promotions
3. stronger governance and audit guarantees

Use when parent state must converge from child work in a repeatable policy-bound way.

## 4. Your scenario mapping

For game and multi-agent systems, combine both modes:

1. peer-owned and ephemeral signals stay room local and do not require promotion
2. server-owned canonical state uses promotion based convergence for durable outcomes
3. manager process creates child rooms, tracks references, and selectively promotes approved outputs

This matches current design direction while enabling a higher-level convergence path only where needed.

## 5. Replay expectations

Parent replay concern depends on chosen mode.

1. Reference only mode: parent replay is about room catalog and accepted summaries, not child internals.
2. Promotion mode: parent replay includes accepted promotion artifacts and is deterministic without replaying full child DAG by default.
3. Deep audit mode can optionally dereference child artifacts for forensic rebuilds.

## 6. Practical contract to freeze next

1. room family metadata schema
2. child creation contract with parent checkpoint reference
3. promotion envelope schema with provenance and hashes
4. deterministic rejection taxonomy for invalid promotions
5. CLI workflow for manager and workers

## 7. Suggested CLI workflow shape

1. topology create-child
2. topology list-children
3. topology propose-promotion
4. topology validate-promotion
5. topology apply-promotion
6. topology show-lineage

### 7a. Phase A CLI command freeze (v1, 2026-05-27)

Evidence artifact: `docs/acceptance/manager-worker-cli-workflow-plan-run01.json`

| Command | Purpose | Required flags (conceptual) | Idempotent when |
|---|---|---|---|
| `nodalmerge topology create-child` | Create child room bound to parent checkpoint | `--parent-room`, `--parent-checkpoint`, `--purpose`, `--policy` | same inputs yield same logical child id (host may enforce) |
| `nodalmerge topology list-children` | List children for a parent | `--parent-room` | read-only |
| `nodalmerge topology propose-promotion` | Submit promotion proposal | `--parent-room`, `--child-room`, `--child-checkpoint`, `--payload-ref` | new `proposal_id` per invocation unless `--idempotency-key` matches prior |
| `nodalmerge topology validate-promotion` | Run validator on proposal | `--proposal-id` | read-only validation |
| `nodalmerge topology apply-promotion` | Apply validated proposal to parent | `--proposal-id` | no-op if already applied at same parent head |
| `nodalmerge topology show-lineage` | Print lineage chain for room | `--room` | read-only |

**Operator drill script (restart/retry outline):**

1. Bootstrap: verify parent room reachable; record `parent_checkpoint` hash used for child binding.
2. Create child via CLI; persist returned `child_room_id` and lineage JSON to operator state store.
3. Simulate worker crash: kill worker process; restart with same config and local persistence path (when headless persistence exists); confirm child room reconnects and tail catches up.
4. Propose promotion with known-good child checkpoint; if `reject.promotion_stale_parent`, refresh parent head, re-validate lineage, retry propose.
5. Validate then apply; confirm parent canonical hash matches expected acceptance artifact from rehearsal.

**Conformance mapping (initial):**

| CLI step | Authority/topology vector (target) |
|---|---|
| create-child with valid parent checkpoint | `AUTH-ROOM-002` inverse (valid child accepted); negative missing lineage in server tests |
| propose/validate/apply happy path | `AUTH-ROOM-003` |
| invalid lineage / stale parent | `AUTH-ROOM-004` |
| replay parent after apply | `AUTH-ROOM-005` |

Governance/drill companions:

1. `docs/TOPOLOGY_GOVERNANCE_POLICY_TEMPLATES.md`
2. `docs/TOPOLOGY_OPERATOR_DRILL_RUNBOOK.md`
3. `docs/FEDERATION_PRECONDITIONS_CHECKLIST.md` (non-federated readiness gates only)

## 8. Non-goals for this playbook

1. replacing existing websocket sync model
2. introducing multi-authority federation semantics
3. embedding product-specific game logic in core runtime
