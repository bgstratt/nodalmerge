# [archive-alert-policy] warn cache_miss_ratio prod-us-east

## 1. Trigger summary

- Route: warn
- Trigger time (UTC): 2026-05-27T00:10:18Z
- Trigger source: cache_miss_ratio
- Trigger metric/query: miss_ratio = miss/(hit+miss)
- Trigger threshold/window: > 0.10 over 15m
- Sample guard status and values: pass; total_samples=286

## 2. Ownership and handoff

- L1 owner (ack): runtime-oncall-01
- L1 ack timestamp (UTC): 2026-05-27T00:11:37Z
- L2 owner: platform-performance-01
- L2 handoff timestamp (UTC): 2026-05-27T00:20:12Z
- L3 owner (if critical): not required for warn route
- L3 handoff timestamp (UTC): n/a

## 3. Scope and impact

- Environment: prod-us-east
- Affected rooms/services: archive.import/archive.validate high churn room prefix
- User-visible impact: none observed
- Change freeze status (yes/no): no

## 4. Triage log

- [x] Dashboard annotation added
- [x] Sample guard validated
- [x] Correlated cache/path or parity-lane evidence captured
- [x] Recovery condition observed
- [x] Recovery window complete (30 minutes below warn)

## 5. Resolution

- Resolution summary: cache miss ratio returned below warn threshold after path churn normalized.
- Recovery observed at (UTC): 2026-05-27T00:42:04Z
- Recovery window complete at (UTC): 2026-05-27T01:12:04Z
- Root cause category: deployment-path churn
- Corrective action: stabilize manifest staging writes to avoid unnecessary revision churn

## 6. Follow-up

- Follow-up owner: platform-performance-01
- Follow-up due date: 2026-05-29
- Linked acceptance artifact update: docs/acceptance/archive-phased-alert-dryrun-run01.json
