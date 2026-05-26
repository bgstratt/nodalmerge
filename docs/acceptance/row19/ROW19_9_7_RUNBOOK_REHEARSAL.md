# Row 19 - 9.7 Incident Runbook and Rehearsal

Status: Runbook authored; simulated drill rehearsal completed
Owner: host-runtime migration stream
Date: 2026-05-12

## 1. Scope

This runbook covers two target incidents for row-19 operational parity:
1. Reconnect storm
2. Conflict/auth spike with convergence risk

## 2. Inputs and telemetry

Required metrics:
1. runtime_ws_connections_opened_total
2. runtime_ws_connections_closed_total
3. runtime_ws_inbound_messages_total
4. runtime_ws_pack_relay_total
5. runtime_auth_validation_total

Required dimensions:
1. room
2. trace
3. type
4. outcome
5. reason

## 3. Reconnect Storm

### Trigger

1. Alert reconnect-storm-room or reconnect-storm-global fires.

### Triage (target: identify impact room set in <= 10 minutes)

1. Open Runtime Health dashboard and verify open/close surge shape.
2. Pivot to Room Hotspots and identify top rooms by open and close rates.
3. For top impacted room, inspect inbound rate and pack relay rate.
4. If inbound is high but relay is low/flat, classify as relay-path regression candidate.
5. Pull top traces for impacted room and check whether one or many traces dominate.

### Mitigation

1. If one trace dominates, isolate offending client session and block reconnect loop source.
2. If many traces dominate same room, enable room-level throttling and restart affected host shard if needed.
3. If auth denied ratio simultaneously spikes, move to Conflict/Auth Spike workflow.

### Verify recovery (target: <= 15 minutes after mitigation)

1. Open/close rates return to baseline band.
2. Inbound rates normalize.
3. Pack relay rate tracks inbound pack traffic again.

## 4. Conflict/Auth Spike

### Trigger

1. Alert auth-deny-spike fires or conflict triage indicates token/policy drift.

### Triage (target: classify root cause in <= 10 minutes)

1. Split denied auth metrics by reason.
2. Identify if one room or many rooms are affected.
3. For a hot room, pivot to trace drilldown and correlate denied outcomes with inbound message traces.
4. Determine class:
   - issuer/signing key drift
   - capability mismatch
   - expired token wave
   - malformed token payloads

### Mitigation

1. Issuer/signing key drift:
   - verify active and previous signing key overlap configuration.
   - roll key config correction and monitor denied ratio.
2. Capability mismatch:
   - roll back recent policy change or patch capability mapper.
3. Expired token wave:
   - validate minting TTL and clock skew settings.
4. Malformed payload burst:
   - add temporary edge filtering for offending client build.

### Verify recovery

1. denied ratio drops below 1% sustained for 15m.
2. room-level denied outliers clear.
3. reconnect/open-close churn does not re-spike.

## 5. Escalation and rollback

1. Escalate to runtime owner if no stabilization in 15 minutes.
2. Escalate to auth owner if reason class is issuer/key/capability and persists after first remediation.
3. Roll back to last known-good auth/profile config if two mitigation attempts fail.

## 6. Rehearsal record (simulated drills)

Window:
1. 2026-05-12, local row-19 stream rehearsal

Steps executed:
1. Verified telemetry emission via runtime unit/integration slices.
2. Verified trace correlation appears in both websocket and auth metric paths.
3. Verified pack-relay correlation counter increments when pack message path executes.
4. Executed reconnect-storm simulation asserting room-scoped closed/inbound metric surges.
5. Executed auth-spike simulation asserting denied-reason metric breakdown (`expired`, `capability mismatch`).

Validation commands and outcomes:
1. dotnet test nodalmerge-host/tests/ActiveSync.DotNetHost.Tests/ActiveSync.DotNetHost.Tests.csproj --filter "FullyQualifiedName~RuntimeWebSocketLoopRunnerTests|FullyQualifiedName~RuntimeTokenValidationServiceTests"
   - Result: 31 passed, 0 failed.
2. dotnet test nodalmerge-host/tests/ActiveSync.DotNetHost.Tests/ActiveSync.DotNetHost.Tests.csproj --filter "FullyQualifiedName~ProviderProfileTokenEndpointIntegrationTests|FullyQualifiedName~RuntimeTokenValidationServiceTests|FullyQualifiedName~RuntimeWebSocketLoopRunnerTests|FullyQualifiedName~ProviderDurabilityTests"
   - Result: 48 passed, 0 failed.

Rehearsal readiness result:
1. Detectability: pass for target telemetry slice.
2. Triageability: pass for room + trace correlation path.
3. Mitigation clarity: pass for reconnect-storm and auth-spike runbook workflows under simulated drill inputs.

## 7. Remaining gap

1. Optional hardening follow-up: execute one live reconnect-storm and one live auth-spike drill in staging with retained dashboard artifacts (screenshots, alert payloads, timeline).
