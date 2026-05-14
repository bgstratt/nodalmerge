# SpeechSlate Host Operational Parity Matrix

Status: Draft baseline
Owner: Host runtime stream
Last Updated: 2026-05-13

## 1. Usage

1. Execute each scenario in Sidecar and Embedded profiles.
2. Mark status: `Pending`, `Pass`, `Fail`, or `Blocked`.
3. Attach evidence link or test/log reference for each status change.
4. Row is complete only when both profiles pass (unless explicitly N/A with rationale).

## 2. Profiles

1. Sidecar: ActiveSync host runs as separate service.
2. Embedded: ActiveSync host integrated in-process with SpeechSlate API.

## 3. Status Legend

1. Pending: not executed
2. Pass: expected behavior validated with evidence
3. Fail: executed and did not meet expectation
4. Blocked: cannot execute due to external dependency/known blocker

## 4. Scenario Matrix

| ID | Scenario Group | Scenario | Sidecar | Embedded | Evidence | Notes |
|---|---|---|---|---|---|---|
| SH-001 | Session | hello -> welcome | Pending | Pending |  |  |
| SH-002 | Session | open-session / client-hello parity | Pending | Pending |  |  |
| SH-003 | Session | close-session semantics | Pending | Pending |  |  |
| SH-004 | Session | reconnect after transient disconnect | Pending | Pending |  |  |
| SH-005 | Data-plane | map-set/get/delete/all flow parity | Pending | Pending |  |  |
| SH-006 | Data-plane | text-insert/delete/get flow parity | Pending | Pending |  |  |
| SH-007 | Data-plane | list push/insert/update/delete/get parity | Pending | Pending |  |  |
| SH-008 | Data-plane | pack/request/mst request/done parity | Pending | Pending |  |  |
| SH-009 | Data-plane | recent-conflicts parity | Pending | Pending |  |  |
| SH-010 | Signaling | webrtc-offer relay via runtime WS | Pending | Pending |  |  |
| SH-011 | Signaling | webrtc-answer relay via runtime WS | Pending | Pending |  |  |
| SH-012 | Signaling | webrtc-ice relay via runtime WS | Pending | Pending |  |  |
| SH-013 | Presence | presence set/get/sweep parity | Pending | Pending |  |  |
| SH-014 | Subscription | subscribe ack + filtered delivery parity | Pending | Pending |  |  |
| SH-015 | Auth | token mint/validate parity | Pending | Pending |  |  |
| SH-016 | Blob | blob-set/get/get-many parity | Pending | Pending |  |  |
| SH-017 | Blob | request-upload grant/deny parity | Pending | Pending |  |  |
| SH-018 | Blob | blob-request redirect/pack parity | Pending | Pending |  |  |
| SH-019 | Resilience | malformed JSON frame recovery | Pending | Pending |  |  |
| SH-020 | Resilience | invalid frame type recovery | Pending | Pending |  |  |
| SH-021 | Resilience | oversized message handling | Pending | Pending |  |  |
| SH-022 | Resilience | parallel connection isolation | Pending | Pending |  |  |
| SH-023 | Ops | startup readiness checks parity | Pending | Pending |  |  |
| SH-024 | Ops | restart durability expectations | Pending | Pending |  |  |
| SH-025 | Ops | observability (logs/metrics/trace id) parity | Pending | Pending |  |  |
| SH-026 | Security | approved public endpoint surface only | Pending | Pending |  |  |
| SH-027 | Security | raw ffi/debug/demo endpoints internal-only | Pending | Pending |  |  |
| SH-028 | Security | token/blob ownership remains API boundary | Pending | Pending |  |  |

## 5. Gate Summary

Release recommendation conditions:

1. All required rows are `Pass` for Sidecar.
2. All required rows are `Pass` for Embedded.
3. No unresolved `Fail` rows for security/exposure scenarios SH-026, SH-027, SH-028.
4. Any `Blocked` rows have approved exception notes and owners.

## 6. Execution Log

| Date | Profile | Scope | Result | Reference |
|---|---|---|---|---|
| 2026-05-13 | Sidecar | Matrix initialized | Pending | Plan bootstrap |
| 2026-05-13 | Embedded | Matrix initialized | Pending | Plan bootstrap |
