# Archive Alert Dashboard Annotation Template

Use this template when acknowledging an archive alert on dashboards.

## Required fields

- Incident ID:
- Alert route: `warn` or `critical`
- Alert source: `cache_miss_ratio` | `parity_drift` | `absolute_latency`
- Triggered metric/query:
- Trigger threshold and window:
- Sample guard status (`pass`/`fail`) and values:
- Acknowledged by (L1):
- Acknowledged at (UTC):
- Current impact scope (room/environment):
- Escalated to (L2/L3):
- Escalated at (UTC):
- Change freeze declared (`yes`/`no`):
- Recovery condition observed at (UTC):
- Recovery window complete at (UTC):
- Follow-up owner:
- Follow-up due date:

## Copy/paste annotation block

```text
[archive-alert-policy]
incident_id=<INC-YYYYMMDD-###>
route=<warn|critical>
source=<cache_miss_ratio|parity_drift|absolute_latency>
metric=<metric_or_query>
threshold=<value + window>
sample_guard=<pass|fail>;samples=<n>
ack_l1=<name>;ack_utc=<timestamp>
scope=<room/env>
escalation=<none|l2|l3>;escalate_utc=<timestamp>
change_freeze=<yes|no>
recovery_observed_utc=<timestamp>
recovery_window_complete_utc=<timestamp>
follow_up_owner=<name>
follow_up_due=<YYYY-MM-DD>
```

## Notes

- Do not escalate on miss-ratio alerts unless sample guards are satisfied.
- For critical parity drift, add "release hold candidate" in annotation text.
