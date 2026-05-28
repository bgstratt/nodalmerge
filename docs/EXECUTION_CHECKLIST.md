# NodalMerge Execution Checklist

Owner: Platform/runtime
Status: Active
Last updated: 2026-05-27

This checklist tracks not-yet-done roadmap work with explicit owners,
target dates, and acceptance evidence artifacts.

## 1. Immediate work (Wave 1 follow-through + ops)

### 1.1 Query/materialization Phase E benchmark and SLO lane

- Status: **Completed** for Wave 1 slice (run-01 through run-03 + closeout artifact)
- Owner: Core/runtime
- Target date: 2026-05-30 (met for declared scope)
- Source plan: `docs/QUERY_MATERIALIZATION_EXECUTION_PLAN.md`
- Required outputs:
  1. Benchmark definitions for projection build/read/rebuild under pressure
  2. Latency and memory SLO ceilings with reproducible run method
  3. Baseline result bundles for controlled profile runs
  4. End-to-end replay-to-checkpoint + paginated projection digest parity (run-03)
- Evidence artifacts:
  1. `docs/acceptance/query-phasee-benchmark-slo-run01.json`
  2. `docs/acceptance/query-phasee-benchmark-baseline-run01.json`
  3. `docs/acceptance/query-phasee-benchmark-baseline-run02.json`
  4. `docs/acceptance/query-phasee-benchmark-baseline-run03.json`
  5. `docs/acceptance/query-phasee-closeout.json`
- Completion gate:
  1. Bench lane is reproducible end-to-end and documented with p50/p95 + memory ceilings (run-01/02).
  2. Pressure-cardinality projection build/read/rebuild evidence captured (run-02).
  3. E2E replay + projection at fixed checkpoint with pagination digest parity recorded (run-03).
  4. Minimal Phase E closeout documents deferral of cooperative projection-build backpressure to Wave 3 / FSE-03.

### 1.2 Export/import post-closeout monitoring cadence

- Status: **Cadence recorded** (weekly review scheduled 2026-06-03)
- Owner: Runtime on-call + platform performance
- Target date: 2026-06-03
- Source plan: `docs/EXPORT_IMPORT_PORTABILITY_EXECUTION_PLAN.md`
- Required outputs:
  1. Weekly threshold review record after Phase D closeout
  2. Recalibration decision log (no-change or change)
  3. If changed, updated threshold artifact and runbook linkage
- Evidence artifacts:
  1. `docs/acceptance/archive-phased-post-closeout-monitoring-run01.json`
  2. `docs/acceptance/archive-phased-threshold-recalibration-run01.json` (only if thresholds change)
- Completion gate:
  1. Monitoring cadence is recorded and next review date is explicitly scheduled (met — next review 2026-06-03).

## 2. Next wave kickoff (Wave 2)

### 2.1 Headless/runtime persistence Phase A contract freeze

- Status: **Phase A draft recorded**; **Phase B memory + filesystem adapters** in `nodalmerge-runtime-local`
- Owner: Runtime + SDK + host streams
- Target date: 2026-06-07
- Source plan: `docs/HEADLESS_RUNTIME_PERSISTENCE_EXECUTION_PLAN.md`
- Required outputs:
  1. Shared local persistence trait/interface draft
  2. Durability and error taxonomy contract
  3. Compatibility/fallback introduction policy
- Evidence artifacts:
  1. `docs/acceptance/headless-persistence-phasea-contract-freeze-run01.json`
  2. `docs/acceptance/headless-persistence-phaseb-memory-adapter-run01.json`
  3. `docs/acceptance/headless-persistence-phaseb-filesystem-adapter-run01.json`
  4. `docs/acceptance/headless-persistence-phasec-sdk-run01.json`
  5. `docs/acceptance/demo-persistence-cutover-run01.json`
  6. `docs/acceptance/headless-persistence-phasee-closeout.json`
- Completion gate:
  1. SDK/runtime/host maintainers approve lifecycle semantics (hydrate, flush, checkpoint, recovery). *Draft satisfies unambiguity in-repo; sign-off tracked out of band.*
  2. Memory + filesystem adapters pass `LOCAL-PERSIST-001` (met). SDK IndexedDB adapter + `embedded` backend alias (met — Phase C).
  3. Initial headless worker (`nodalmerge-headless`) passes `HEADLESS-RUN-001` — evidence `docs/acceptance/headless-run-phased-worker-run01.json`.
  4. Headless depth: IBF/MST worker loop, `HEADLESS-RUN-003` restart catch-up, `headless/Dockerfile` — evidence `docs/acceptance/headless-run-phased-depth-run01.json`.
  5. Headless Phase E: session report JSON, operator runbook section, `LOCAL-PERSIST-002`, CI smoke — evidence `docs/acceptance/headless-run-phased-phasee-run01.json`.
  6. Custom backend pilot (§4b): composite + registry — evidence `docs/acceptance/headless-persistence-phaseb-composite-pilot-run01.json`.
  7. **Declared slice closed** for headless peer-local persistence (Phases A–E per plan); Prometheus/Criterion deferred — evidence `docs/acceptance/headless-persistence-phasee-closeout.json`.

### 2.2 Authority/topology Phase A contract freeze

- Status: **Closed for declared Wave 2 slice** (Phases A–D + closeout artifact)
- Owner: Runtime + host + operator streams
- Target date: 2026-06-09
- Source plan: `docs/AUTHORITY_AND_ROOM_TOPOLOGY_EXECUTION_PLAN.md`
- Required outputs:
  1. Authority role matrix terminology freeze
  2. Room lineage metadata schema draft
  3. Promotion command/event draft
- Evidence artifacts:
  1. `docs/acceptance/authority-topology-phasea-contract-freeze-run01.json`
  2. `docs/acceptance/authority-topology-phaseb-lineage-run01.json`
  3. `docs/acceptance/authority-topology-phasec-promotion-run01.json`
  4. `docs/acceptance/authority-topology-phased-cli-run01.json`
  5. `docs/acceptance/authority-topology-phased-closeout.json`
- Completion gate:
  1. Parent checkpoint semantics are unambiguous across runtime, host, and operator lanes. *Draft in plan; cross-stream sign-off pending.*
  2. New child rooms carry lineage metadata; create/list/describe paths have vector evidence (met).
  3. Promotion propose/validate/apply with AUTH-ROOM-003/004/005 slice (met).
  4. `nodalmerge topology` CLI runnable headless over WS with CLI-TOPOLOGY-001/002 (met). Phase E hardening deferred.

### 2.3 Manager/worker playbook to executable CLI workflow plan

- Status: **Phase A plan recorded**; **executable CLI met** via `nodalmerge-cli`
- Owner: Runtime + operator streams
- Target date: 2026-06-10
- Source plan: `docs/MANAGER_WORKER_TOPOLOGY_PLAYBOOK.md`
- Required outputs:
  1. CLI command set freeze (`create-child`, `list-children`, `propose/validate/apply-promotion`, `show-lineage`)
  2. Operator drill script for restart and retry behavior
  3. Initial conformance mapping to authority/topology vectors
- Evidence artifacts:
  1. `docs/acceptance/manager-worker-cli-workflow-plan-run01.json`
  2. `docs/acceptance/authority-topology-phased-cli-run01.json`
- Completion gate:
  1. CLI workflow is runnable in headless/pod context without browser dependency (met for topology command group).

## 3. Wave 3 preparatory queue

### 3.1 Production-ready topology and promotion CLI workflows

- Status: **Completed** for declared Wave 3 topology slice (2026-05-27)
- Owner: Runtime + operator streams
- Target date: 2026-06-17 (met for declared scope)
- Source plan: `docs/roadmap.md` (Wave 3)
- Evidence artifacts:
  1. `docs/acceptance/authority-topology-wave3-durable-promotion-run01.json` (AUTH-ROOM-006)
  2. `docs/acceptance/authority-topology-wave3-host-parity-run01.json`
  3. `docs/acceptance/authority-topology-wave3-promotion-metrics-run01.json`
  4. `docs/acceptance/authority-topology-wave3-multi-peer-run01.json` (AUTH-TOPOLOGY-007)
  5. `docs/acceptance/authority-topology-wave3-dotnet-host-parity-run01.json`
  6. `docs/acceptance/wave3-topology-cli-readiness-run01.json`
  7. `docs/acceptance/authority-topology-wave3-closeout.json`
- Completion gate:
  1. CLI + server + Rust host-core + dotnet stub parity + host_migration golden fixture (met).
  2. Operator metrics snippet in `docs/operator.md` (met). Dashboard visualization deferred.
  3. Durable lineage across restart and Phase E scale items explicitly deferred.

### 3.2 Reliability and performance baseline package

- Status: Planned
- Owner: Platform performance + operators
- Target date: 2026-06-19
- Source plan: `docs/roadmap.md` (Wave 3)
- Evidence artifacts:
  1. `docs/acceptance/wave3-reliability-performance-baseline-run01.json`

## 4. Tracking policy

1. Update this checklist when a checkpoint starts, completes, or is re-scoped.
2. Record evidence artifact names before implementation starts.
3. Keep roadmap and source execution plans as authoritative status logs; this file is the execution run sheet.
