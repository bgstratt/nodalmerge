# SpeechSlate Host Packaging and Operational Parity Plan

Status: Execution in progress (Sidecar complete, Embedded started)
Owner: Host runtime stream
Date: 2026-05-13

## 1. Objective

Ship ActiveSync host integration to SpeechSlate in two controlled modes:

1. Sidecar mode first (lowest-risk rollout and parity baseline).
2. Embedded mode second (in-process integration and control-plane optimization).

Primary constraint:
1. Preserve behavioral and operational parity across modes before cutover.

## 2. Scope

In scope:
1. Packaging strategy for managed host and native runtime.
2. Sidecar deployment profile and acceptance.
3. Embedded deployment profile and acceptance.
4. Endpoint exposure policy and hardening.
5. Parity matrix and operational evidence.

Out of scope:
1. CRDT/protocol redesign.
2. New SDK feature semantics unrelated to packaging/integration.
3. Product UX redesign.

## 3. Guiding Decisions

1. Sidecar is the first production-ready profile.
2. Embedded is additive and must prove parity before production preference.
3. Runtime data-plane stays WebSocket (`/ws/runtime`, optional `/ws/{roomId}` compatibility alias).
4. Control-plane endpoints are candidates for in-process calls in embedded mode.
5. Public attack surface is minimized: no public raw FFI/debug/demo endpoints.

## 4. Target Artifacts

## 4.1 Packages

1. Managed host package:
- Runtime and composition services, endpoint mapping helpers, provider wiring.
2. Native runtime package:
- RID-specific native host FFI binaries under `runtimes/<rid>/native`.
3. Optional meta package:
- Pulls managed + selected native package(s) for convenience.

## 4.2 Deployment Profiles

1. Sidecar profile:
- SpeechSlate API and ActiveSync host as separate services.
2. Embedded profile:
- ActiveSync host integrated in-process in SpeechSlate API.

## 5. Endpoint Exposure Policy

Public (required):
1. `GET ws(s)://.../ws/runtime`
2. Optional compatibility: `GET ws(s)://.../ws/{roomId}`

Internal-only or disabled in production:
1. `/ffi/abi-version`
2. `/ffi/submit`
3. `/ws/ffi`
4. `/demo`
5. `/debug/providers`
6. Root metadata endpoint `/` (optional internal-only)

Control-plane hosting policy:
1. Keep token/blob APIs in SpeechSlate API boundary.
2. Embedded mode may route token/blob operations to in-process provider/service calls.

## 6. Rollout Slices

## Slice S0: Plan and Baseline Freeze

Goals:
1. Freeze this plan and parity matrix definitions.
2. Define baseline version tags/commit SHAs for host and SpeechSlate API.

Tasks:
1. Confirm required runtime routes and endpoint policy.
2. Confirm parity scenarios and evidence format.
3. Lock initial sidecar configuration contract.

Exit criteria:
1. Plan accepted.
2. Baseline SHA/version references recorded.

Evidence:
1. This plan document.
2. Parity matrix file created (see Section 8).

## Slice S1: Managed + Native Packaging (No Behavior Change)

Goals:
1. Produce distributable packages without changing runtime semantics.

Tasks:
1. Create managed package metadata and package boundaries.
2. Add native assets packaging with RID layout:
- `runtimes/win-x64/native/activesync_host_ffi.dll`
- `runtimes/linux-x64/native/libactivesync_host_ffi.so`
- `runtimes/osx-x64/native/libactivesync_host_ffi.dylib`
- arm64 RIDs as required by target deployment.
3. Verify local resolution with and without explicit `ACTIVESYNC_HOST_FFI_DLL`.

Locked defaults for initial implementation:
1. Minimum native package targets in first cut: `win-x64` and `linux-x64` only.
2. Initial publish target: GitHub Packages via `.github/workflows/nuget-build-push.yml`.
3. NuGet.org publication is deferred to a later slice after sidecar parity stabilization.

Exit criteria:
1. Package restore + runtime startup succeeds on target OS/RID set.
2. Existing host tests remain green.

Evidence:
1. Package layout manifest.
2. Startup verification logs per OS/RID.

## Slice S2: Sidecar Integration with SpeechSlate API

Goals:
1. Integrate sidecar with minimal SpeechSlate API behavior changes.

Tasks:
1. Add deployment config for host URL/WS routing.
2. Configure network/proxy rules between SpeechSlate API and host.
3. Ensure token/blob contracts remain SpeechSlate-owned externally.
4. Validate CORS/origin and auth boundary behavior.

Exit criteria:
1. SpeechSlate client sync works through sidecar host.
2. No regressions in token mint/validate and blob URL behavior.
3. External endpoint exposure matches Section 5 policy.

Evidence:
1. End-to-end sidecar smoke logs.
2. Endpoint exposure inventory.

## Slice S3: Sidecar Operational Parity Gate

Goals:
1. Prove sidecar mode parity before embedded work.

Tasks:
1. Execute parity matrix in Section 8 for sidecar mode.
2. Record reliability metrics and failure-mode behavior.
3. Validate restart/reconnect semantics and room/session stability.

Exit criteria:
1. All required sidecar parity rows pass.
2. Any exceptions are documented with explicit blocker owners.

Evidence:
1. Completed parity matrix with sidecar results.
2. Incident/failure notes with remediations.

## Slice S4: Embedded Host Package and Integration Surface

Goals:
1. Expose embedded integration API while retaining sidecar compatibility.

Tasks:
1. Add explicit embedded registration and endpoint mapping API.
2. Ensure route mapping is configurable (enable/disable compatibility alias and internal endpoints).
3. Preserve WS runtime data-plane semantics.

Exit criteria:
1. Embedded startup in SpeechSlate API succeeds with hardened endpoint profile.
2. Sidecar mode remains fully functional.

Evidence:
1. Embedded integration sample and startup verification.

## Slice S5: Control-Plane In-Process Call Migration (Embedded)

Goals:
1. Replace internal HTTP hops with direct method calls where appropriate.

Candidate migrations:
1. Token mint: `IRoomTokenAuthProvider.MintAsync`
2. Token validate: `IRoomTokenAuthProvider.ValidateAsync`
3. Blob URL resolve: `IBlobUrlResolverProvider.ResolvePutUrlAsync` / `ResolveGetUrlAsync`

Tasks:
1. Introduce service abstractions in SpeechSlate API for control-plane operations.
2. Switch call sites from HTTP to DI method calls.
3. Keep compatibility endpoints as optional fallback during transition.

Exit criteria:
1. Embedded mode passes control-plane parity checks.
2. No public API contract regressions.

Evidence:
1. Before/after call-path inventory.
2. Functional parity logs for migrated operations.

## Slice S6: Embedded Operational Parity Gate

Goals:
1. Prove embedded mode parity with sidecar baseline.

Tasks:
1. Run full parity matrix for embedded mode.
2. Compare sidecar vs embedded outcomes (functional + operational).
3. Validate security posture and endpoint exposure.

Exit criteria:
1. Embedded parity equals or exceeds sidecar parity.
2. Security exposure is equal or smaller than sidecar production profile.

Evidence:
1. Parity comparison report.
2. Security/exposure checklist sign-off.

## Slice S7: Production Cutover and Runbook Finalization

Goals:
1. Enable safe production choice of sidecar or embedded profile.

Tasks:
1. Finalize runbooks for both profiles.
2. Add rollback and kill-switch procedures.
3. Define default production profile and change policy.

Exit criteria:
1. Production recommendation approved.
2. Operators have complete playbooks for both profiles.

Evidence:
1. Operator runbook updates.
2. Cutover and rollback drill logs.

## 7. Required Test Gates per Slice

Baseline gates (where applicable):
1. `cargo test -p activesync-host-core`
2. `cargo test -p activesync-host-ffi`
3. `dotnet test dotnet-host/ActiveSync.DotNetHost.slnx`

Additional gates:
1. Sidecar smoke verifier (`dotnet-host/verify.ps1`) for sidecar slices.
2. Embedded in-process startup + WS flow smoke for embedded slices.
3. Endpoint exposure assertions for production profile.

## 8. Parity Matrix (Required Scenarios)

The matrix should be captured in a dedicated artifact (recommended path:
`docs/acceptance/speechslate-host/OPERATIONAL_PARITY_MATRIX.md`) and executed for both Sidecar and Embedded columns.

Required scenario groups:
1. Connection/session:
- hello/open-session/client-hello/close-session
- reconnect after transient disconnect
2. Realtime data-plane:
- map/text/list/blob command/event flows
- pack/request/mst/conflict behavior
3. Signaling relay:
- webrtc-offer/webrtc-answer/webrtc-ice message relay behavior
4. Presence/subscription/policy/auth:
- presence set/get/sweep
- subscribe ack and scoped delivery
- token validation outcomes
5. Blob handling:
- inline blob flow
- delegated upload URL resolution and redirect handling
6. Resilience:
- malformed frame recovery
- oversized message handling
- connection isolation under mixed outcomes
7. Operational behavior:
- restart durability expectations
- startup dependency/readiness checks
- observability and diagnostics parity

## 9. Security and Exposure Acceptance Checklist

1. Public runtime endpoints limited to approved WS path(s).
2. Raw FFI/debug/demo endpoints blocked or disabled in production.
3. Token and blob control-plane ownership remains within SpeechSlate API boundary.
4. Auth provider mode documented and validated for deployment profile.
5. CORS/origin and reverse-proxy behavior validated for client routes.

## 10. Risks and Mitigations

Risk: Sidecar and embedded diverge behaviorally.
Mitigation:
1. Single parity matrix executed against both profiles.
2. Block cutover until parity gate passes.

Risk: Embedded profile increases attack surface.
Mitigation:
1. Strict endpoint mapping policy.
2. Internal-only control-plane and debug/raw endpoints.

Risk: Native asset resolution issues across environments.
Mitigation:
1. RID-native package validation per platform.
2. Keep `ACTIVESYNC_HOST_FFI_DLL` override for emergency pathing.

## 11. Progress Tracking

Execution status key:
1. `Not Started`
2. `In Progress`
3. `Blocked`
4. `Complete`

Slice status board:
1. S0 Plan and Baseline Freeze: `Complete`
2. S1 Managed + Native Packaging: `In Progress`
3. S2 Sidecar Integration: `Complete`
4. S3 Sidecar Operational Parity Gate: `In Progress` (checkpoint pass; remaining rows can continue in parallel)
5. S4 Embedded Package/Surface: `In Progress`
6. S5 Control-Plane In-Process Migration: `Not Started`
7. S6 Embedded Operational Parity Gate: `Not Started`
8. S7 Production Cutover and Runbooks: `Not Started`

S2 implementation artifacts:
1. Sidecar sample runtime profile: `dotnet-host/src/ActiveSync.DotNetHost/appsettings.SpeechSlate.Sidecar.sample.json`
2. Sidecar execution checklist: `docs/acceptance/speechslate-host/S2_SIDECAR_IMPLEMENTATION_CHECKLIST.md`
3. Parity tracking matrix: `docs/acceptance/speechslate-host/OPERATIONAL_PARITY_MATRIX.md`

## 12. Definition of Done

1. Sidecar and embedded profiles are both validated end-to-end.
2. Operational parity matrix passes in both profiles.
3. Production endpoint exposure is hardened per policy.
4. Packaging and runbooks allow repeatable deployment in either mode.
5. Production default profile is explicitly selected with rollback path documented.
