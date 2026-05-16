# S4 Embedded Implementation Checklist

Status: In progress
Owner: Host runtime stream
Last Updated: 2026-05-14

## 1. Purpose

Define the concrete integration contract for Embedded mode where ActiveSync host is integrated in-process with SpeechSlate API while preserving sidecar parity and security constraints.

## 2. Embedded Contract

SpeechSlate API + ActiveSync host run in one process:
1. Runtime WebSocket transport behavior remains parity-equivalent to Sidecar mode.
2. Token/blob control-plane boundaries remain SpeechSlate-owned.
3. Internal HTTP hops may be replaced with in-process service calls where approved.

## 3. Required S4 Outcomes

1. Embedded registration and endpoint mapping can start successfully in Development.
2. Public endpoint surface remains limited to approved runtime websocket route(s).
3. Internal/debug/raw FFI endpoints remain blocked or disabled in production profile.
4. Existing Sidecar mode startup remains functional after embedded wiring changes.

## 4. Execution Steps

1. Add/verify explicit embedded registration path in SpeechSlate API startup.
2. Map runtime websocket endpoint(s) and preserve route compatibility rules.
3. Wire required providers using current sidecar-equivalent configuration values.
4. Validate startup, websocket hello/open-session flow, and multi-client relay.
5. Record sidecar-vs-embedded deltas in OPERATIONAL_PARITY_MATRIX.md.

## 5. Evidence Capture

For each S4 run, capture:
1. Embedded startup log excerpt.
2. Runtime websocket smoke output.
3. Endpoint exposure inventory.
4. Matrix status updates for embedded column rows touched.

## 6. Exit Criteria for S4

1. Embedded host starts and accepts runtime websocket traffic.
2. Runtime behavior matches sidecar for executed parity rows.
3. Endpoint exposure conforms to policy.
4. No sidecar regressions introduced by embedded wiring changes.

## 7. Checkpoint Log

1. 2026-05-14: Embedded kickoff wiring landed in SpeechSlate API with in-process runtime websocket routes (`/ws/runtime`, `/ws/{roomId}`).
2. 2026-05-14: API mint path successfully targeted local `/sync/token` bridge endpoint (no sidecar dependency).
3. 2026-05-14: Embedded runtime accepted websocket room traffic and persisted DAG packs during live client activity.
