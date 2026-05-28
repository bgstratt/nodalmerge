# Archive Alert Incident Ticket Template

Use this template when opening an incident for archive alert policy events.

## Title

`[archive-alert-policy] <warn|critical> <cache_miss_ratio|parity_drift|absolute_latency> <env>`

## Description

### 1. Trigger summary

- Route: `warn` or `critical`
- Trigger time (UTC):
- Trigger source:
- Trigger metric/query:
- Trigger threshold/window:
- Sample guard status and values:

### 2. Ownership and handoff

- L1 owner (ack):
- L1 ack timestamp (UTC):
- L2 owner:
- L2 handoff timestamp (UTC):
- L3 owner (if critical):
- L3 handoff timestamp (UTC):

### 3. Scope and impact

- Environment:
- Affected rooms/services:
- User-visible impact:
- Change freeze status (`yes`/`no`):

### 4. Triage log

- [ ] Dashboard annotation added
- [ ] Sample guard validated
- [ ] Correlated cache/path or parity-lane evidence captured
- [ ] Recovery condition observed
- [ ] Recovery window complete (30 minutes below warn)

### 5. Resolution

- Resolution summary:
- Recovery observed at (UTC):
- Recovery window complete at (UTC):
- Root cause category:
- Corrective action:

### 6. Follow-up

- Follow-up owner:
- Follow-up due date:
- Linked acceptance artifact update:
