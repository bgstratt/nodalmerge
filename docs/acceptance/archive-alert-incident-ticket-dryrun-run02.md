# [archive-alert-policy] critical parity_drift prod-us-east

## 1. Trigger summary

- Route: critical
- Trigger time (UTC): 2026-05-27T05:25:33Z
- Trigger source: parity_drift
- Trigger metric/query: abs(p95_archive_import_object_ms - p95_archive_import_file_ms)
- Trigger threshold/window: > 5 ms over 15m
- Sample guard status and values: pass; total_samples=412

## 2. Ownership and handoff

- L1 owner (ack): runtime-oncall-01
- L1 ack timestamp (UTC): 2026-05-27T05:26:44Z
- L2 owner: platform-performance-01
- L2 handoff timestamp (UTC): 2026-05-27T05:29:06Z
- L3 owner (if critical): runtime-techlead-01
- L3 handoff timestamp (UTC): 2026-05-27T05:31:48Z

## 3. Scope and impact

- Environment: prod-us-east
- Affected rooms/services: archive.import object lane ring-2
- User-visible impact: elevated import completion delay on object lane
- Change freeze status (yes/no): yes

## 4. Triage log

- [x] Dashboard annotation added
- [x] Sample guard validated
- [x] Correlated cache/path or parity-lane evidence captured
- [x] Recovery condition observed
- [x] Recovery window complete (30 minutes below warn)

## 5. L3 freeze and rollback decision log

- Freeze declared at (UTC): 2026-05-27T05:33:05Z
- Freeze authority: runtime-techlead-01
- Rollback option evaluated: ring-2 archive object-lane release
- Rollback decision: approved
- Rollback initiated at (UTC): 2026-05-27T05:37:22Z
- Rollback verification complete at (UTC): 2026-05-27T05:44:51Z
- Decision rationale: parity drift exceeded critical threshold with sustained sample volume and rising queue depth.

## 6. Resolution

- Resolution summary: parity drift returned below warn after ring-2 rollback and queue drain.
- Recovery observed at (UTC): 2026-05-27T05:55:10Z
- Recovery window complete at (UTC): 2026-05-27T06:25:10Z
- Root cause category: rollout regression
- Corrective action: block ring-2 promote until parity profile diff is re-baselined.

## 7. Follow-up

- Follow-up owner: platform-performance-01
- Follow-up due date: 2026-05-29
- Linked acceptance artifact update: docs/acceptance/archive-phased-alert-dryrun-run02.json
