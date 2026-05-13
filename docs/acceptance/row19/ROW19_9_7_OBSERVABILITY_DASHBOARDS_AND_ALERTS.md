# Row 19 - 9.7 Observability Dashboards and Alerts

Status: Slice 1 implemented (runtime counters + trace correlation); dashboard and alert spec published
Owner: host-runtime migration stream
Date: 2026-05-12

## 1. Objective

Define a concrete, low-cardinality dashboard and alert pack for runtime operational parity in reconnect-storm and conflict-spike incidents.

## 2. Signal catalog (runtime source of truth)

Primary counters:
1. runtime_ws_connections_opened_total
2. runtime_ws_connections_closed_total
3. runtime_ws_inbound_messages_total
4. runtime_ws_pack_relay_total
5. runtime_auth_validation_total

Primary dimensions:
1. room
2. type
3. outcome
4. reason
5. trace
6. session

Code evidence:
1. dotnet-host/src/ActiveSync.DotNetHost/Runtime/RuntimeWebSocketLoopRunner.cs
2. dotnet-host/src/ActiveSync.DotNetHost/Runtime/RuntimeTokenValidationService.cs

Test evidence:
1. RuntimeWebSocketLoopRunnerTests.Runtime_ws_metrics_emit_connection_and_inbound_counts
2. RuntimeWebSocketLoopRunnerTests.Runtime_ws_metrics_emit_trace_and_pack_relay_correlation_counts
3. RuntimeTokenValidationServiceTests.ValidateInboundAsync_emits_runtime_auth_metrics_for_allowed_and_denied_paths
4. RuntimeTokenValidationServiceTests.ValidateInboundAsync_promotes_trace_id_from_payload_to_state_and_metrics
5. RuntimeWebSocketLoopRunnerTests.Simulated_reconnect_storm_emits_actionable_room_metrics
6. RuntimeTokenValidationServiceTests.Simulated_auth_spike_emits_denied_reason_breakdown

## 3. Dashboard pack

### A. Runtime Health (global)

Panels:
1. Connections opened/min (sum rate of runtime_ws_connections_opened_total).
2. Connections closed/min (sum rate of runtime_ws_connections_closed_total).
3. Inbound messages/min (sum rate of runtime_ws_inbound_messages_total).
4. Pack relay events/min (sum rate of runtime_ws_pack_relay_total).
5. Auth denied ratio:
   - numerator: rate(runtime_auth_validation_total{outcome="denied"})
   - denominator: rate(runtime_auth_validation_total{outcome=~"allowed|denied"})

Purpose:
1. Detect transport churn and auth pressure at fleet scope.

### B. Room Hotspots

Panels:
1. Top 20 rooms by inbound message rate.
2. Top 20 rooms by pack relay rate.
3. Top 20 rooms by denied auth validations.
4. Open-close imbalance by room:
   - rate(opened) - rate(closed)

Purpose:
1. Quickly isolate high-noise or unstable rooms.

### C. Trace Correlation Drilldown

Panels:
1. Inbound messages by trace (filtered by room and 15m window).
2. Pack relay count by trace.
3. Auth validation outcomes by trace.

Purpose:
1. Follow one incident trace across transport and auth paths.

## 4. Alert rules

Rule 1: reconnect-storm-room
1. Condition: per-room open rate > 30/min for 5m AND close rate > 30/min for 5m.
2. Severity: warning.
3. Labels: room, service=dotnet-host-runtime.
4. Runbook: ROW19_9_7_RUNBOOK_REHEARSAL.md section "Reconnect Storm".

Rule 2: reconnect-storm-global
1. Condition: global open rate > 300/min for 10m AND closed/opened ratio between 0.8 and 1.2.
2. Severity: critical.
3. Purpose: sustained churn with no net stabilizing connections.

Rule 3: auth-deny-spike
1. Condition: denied ratio > 5% for 10m.
2. Severity: warning.
3. Dimension split: reason and room.

Rule 4: pack-relay-drop
1. Condition: inbound pack-like traffic observed but runtime_ws_pack_relay_total flat for 5m in active room set.
2. Severity: warning.
3. Purpose: detect fanout breakage.

Rule 5: noisy-trace-loop
1. Condition: a single trace exceeds 200 inbound messages in 5m.
2. Severity: warning.
3. Purpose: detect replay loops or client storms.

## 5. Cardinality guardrails

1. Keep trace dimension in short retention windows only (incident windows), not long-term trend dashboards.
2. Session label is allowed for short-lived forensic charts; avoid long historical aggregation by session.
3. Reason labels are allowlisted at source (token rejected, invalid token, expired, capability mismatch, issuer mismatch).

## 6. Acceptance evidence checkpoints

1. Metrics exist and increment under tests for websocket and token validation paths.
2. Trace id appears in metric tags for websocket and auth paths.
3. Pack relay events include trace_id for runtime correlation.
4. Alert definitions are documented and bound to runbook actions.

## 7. Validation commands

1. dotnet test dotnet-host/tests/ActiveSync.DotNetHost.Tests/ActiveSync.DotNetHost.Tests.csproj --filter "FullyQualifiedName~RuntimeWebSocketLoopRunnerTests|FullyQualifiedName~RuntimeTokenValidationServiceTests"
2. dotnet test dotnet-host/tests/ActiveSync.DotNetHost.Tests/ActiveSync.DotNetHost.Tests.csproj --filter "FullyQualifiedName~ProviderProfileTokenEndpointIntegrationTests|FullyQualifiedName~RuntimeTokenValidationServiceTests|FullyQualifiedName~RuntimeWebSocketLoopRunnerTests|FullyQualifiedName~ProviderDurabilityTests"

Observed results in this stream:
1. 31 passed, 0 failed.
2. 48 passed, 0 failed.
