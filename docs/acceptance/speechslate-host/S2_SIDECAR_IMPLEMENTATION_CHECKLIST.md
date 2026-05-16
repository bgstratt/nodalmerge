# S2 Sidecar Implementation Checklist

Status: Complete (ready to transition to Embedded)
Owner: Host runtime stream
Last Updated: 2026-05-14

## 1. Purpose

Define the concrete integration contract for Sidecar mode between SpeechSlate API and ActiveSync .NET host, with explicit security boundaries and parity evidence steps.

## 2. Integration Contract

SpeechSlate API remains control-plane owner:
1. Token mint/validate authority remains SpeechSlate-owned.
2. Blob delegated presign authority remains SpeechSlate-owned.
3. External public API contract remains stable for clients.

ActiveSync sidecar remains runtime data-plane owner:
1. Runtime WebSocket endpoint for sync transport.
2. Protocol/session execution and relay behavior.
3. Durable node/blob adapter integration.

## 3. Required Runtime Endpoints

Publicly reachable endpoints:
1. GET websocket path /ws/runtime
2. Optional compatibility alias /ws/{roomId}

Internal-only or blocked endpoints in production:
1. /ffi/abi-version
2. /ffi/submit
3. /ws/ffi
4. /demo
5. /debug/providers
6. /

## 4. Required Provider Configuration

Use sample profile as baseline:
1. dotnet-host/src/ActiveSync.DotNetHost/appsettings.SpeechSlate.Sidecar.sample.json

Local secrets bootstrap options:
1. Set keys directly:
	- `dotnet user-secrets set --project dotnet-host/src/ActiveSync.DotNetHost/ActiveSync.DotNetHost.csproj <key> <value>`
2. Inspect active keys:
	- `dotnet user-secrets list --project dotnet-host/src/ActiveSync.DotNetHost/ActiveSync.DotNetHost.csproj`

Required provider selections:
1. ActiveSync:Providers:NodeStorage=Mongo
2. ActiveSync:Providers:BlobStorage=S3Delegated
3. ActiveSync:Providers:Auth=JwtBridgeSidecar

Required delegated auth semantics:
1. Sidecar auth client calls POST /mint and POST /validate against the configured JwtBridgeSidecar BaseUrl.
2. S3 delegated resolver calls configured PutPath and GetPath against configured S3Delegated BaseUrl.

## 5. Network and Auth Boundary Checks

1. Sidecar can reach SpeechSlate API delegated endpoints over internal network.
2. Sidecar endpoints are not directly internet exposed except approved WebSocket route(s).
3. Reverse proxy routes external runtime traffic only to /ws/runtime and optional /ws/{roomId}.
4. Internal API key/header for delegated calls is configured and rotated through secrets management.
5. CORS/origin rules permit expected client origins for runtime websocket route.

## 6. Execution Steps

1. Configure sidecar with sample profile values adjusted for environment.
2. Start sidecar and confirm startup readiness.
3. Verify delegated blob-url route by running dotnet-host/verify.ps1 with BaseUrl and DelegateBaseUrl adjusted for environment.
4. Verify runtime websocket hello and noop flow.
5. Execute parity matrix rows SH-001 through SH-028 in sidecar column.

## 7. Evidence Capture

For each S2 run, capture:
1. Runtime startup log excerpt.
2. Delegated blob-url verification output.
3. Runtime websocket verification output.
4. Endpoint exposure inventory showing only approved public routes.
5. Matrix status updates in OPERATIONAL_PARITY_MATRIX.md.

## 8. Exit Criteria for S2

1. Sidecar deployment config and network/auth contract validated in target environment.
2. No regressions in token and delegated blob behavior through SpeechSlate boundary.
3. External endpoint exposure conforms to Section 3.
4. S2 evidence artifacts attached and parity matrix sidecar column updated.

## 9. Completion Note (2026-05-14)

1. Sidecar was exercised with concurrent Chrome + Edge clients connected and mirroring updates.
2. SpeechSlate API token mint/forward flow to sidecar returned successful responses during live activity.
3. Sidecar runtime logs showed sustained relay and persistence activity without the prior BSON null cast failure signature.
4. Team decision: close S2 and proceed to Embedded integration slices.
