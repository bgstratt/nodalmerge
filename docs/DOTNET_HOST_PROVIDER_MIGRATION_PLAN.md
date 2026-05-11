# .NET Hosted Service Provider Migration Plan

Status: Draft for implementation planning
Owner: Hosted runtime stream
Date: 2026-05-11

## 1. Goals

1. Keep the hosted runtime as the transport/runtime owner.
2. Keep deterministic sync semantics in host-core/host-ffi.
3. Add pluggable providers for:
- DAG/node persistence
- blob storage and delivery mode (direct, delegated presign, WS fallback)
- auth/token validation/mint integration
4. Allow SpeechSlate and other deployments to pick provider combinations via config, without forking runtime code.

## 2. Confirmed Current State

1. Tokened room connect is already app/API-owned and compatible with hosted runtime:
- SpeechSlate web client uses SDK tokenProvider backed by API `/sync/token`.
- The hosted runtime receives the same hello token payload through existing runtime mapping.
2. Blob delegate hooks are already present in engine contracts:
- host-core defines `HostBlobUrlResolver` with `resolve_put_url` and `resolve_get_url`.
- HostEngine already has `with_blob_url_resolver(...)`.
3. Node/blob persistence adapters are currently mostly server-crate-centric examples (DirPersistence, Mongo+S3, Postgres+S3).

Implication: your intuition is correct. We should add provider projects around stable contracts, then compose them in the .NET host.

## 3. Target Architecture

## 3.1 Layering

1. ActiveSync runtime core:
- host-core + host-ffi + dotnet runtime mapper/loop
- no storage/vendor-specific dependencies
2. Provider contracts:
- small abstractions package consumed by runtime host
- capability-based registration (what this provider supports)
3. Provider implementations:
- one package/project per storage concern and vendor family
4. Host composition:
- appsettings/env chooses providers
- DI wires selected providers at startup

## 3.2 Provider Families

1. Node/DAG persistence providers:
- InMemory
- Sqlite/File
- Mongo
- Postgres
2. Blob providers:
- WS-only (no direct URLs)
- File/local blob store
- S3 direct credentials
- S3 delegated presign service
3. Auth/token providers:
- pass-through (current behavior)
- JWT-bridge integration client
- custom enterprise auth adapter

## 4. Recommended Solution/Package Layout

## 4.1 Contracts and runtime

1. ActiveSync.DotNetHost
- ASP.NET runtime host, websocket loops, mapping
- depends only on abstractions + selected provider packages
2. ActiveSync.Host.Abstractions
- contracts for node persistence, blob URL resolver, blob read/write, auth hooks
- options and health-check interfaces
3. ActiveSync.Host.Composition
- DI extension methods
- provider binding from appsettings/env

## 4.2 Provider packages (small extenders)

1. ActiveSync.Host.Persistence.InMemory
2. ActiveSync.Host.Persistence.Sqlite
3. ActiveSync.Host.Persistence.Mongo
4. ActiveSync.Host.Persistence.Postgres
5. ActiveSync.Host.Blobs.WsOnly
6. ActiveSync.Host.Blobs.File
7. ActiveSync.Host.Blobs.S3Direct
8. ActiveSync.Host.Blobs.S3Delegated
9. ActiveSync.Host.Auth.JwtBridge

This matches your preference: small optional extenders with stable contracts and config-driven activation.

## 4.3 Why not one giant package

1. Cleaner optional dependency graph (Mongo, AWS SDK, Npgsql, etc. only when used).
2. Faster security patching and upgrades by provider.
3. Easier testing matrix and clearer operational ownership.
4. Keeps runtime host package lightweight.

## 5. Contracts to Freeze First

## 5.1 Node persistence contract

Required capabilities:

1. LoadRoomSnapshot(roomId) on room warm/open
2. PersistAcceptedNodes(roomId, nodes) on import/apply path
3. PersistCompactionSnapshot(roomId, snapshot)
4. Optional metadata:
- watermarks/version
- tenant/account scoping

## 5.2 Blob contract

Required capabilities:

1. PutBlob(hash, bytes, contentType)
2. GetBlob(hash)
3. ResolvePutUrl(roomId, namespace, hash, size, contentType)
4. ResolveGetUrl(roomId, namespace, hash)
5. Fallback policy:
- if no URL resolver or URL failure, use WS fallback path

## 5.3 Auth/token contract

Required capabilities:

1. Validate incoming room token
2. Optional room lock key lookup and rotation support
3. Optional callout to JWT-bridge sidecar/service

## 6. Config Model (appsettings/env)

Single host with provider selection:

1. ActiveSync:Storage:Nodes:Provider = InMemory | Sqlite | Mongo | Postgres
2. ActiveSync:Storage:Blobs:Provider = WsOnly | File | S3Direct | S3Delegated
3. ActiveSync:Auth:Provider = Default | JwtBridge
4. Provider-specific nested sections for connection strings, bucket/region, delegate endpoint, retries, TTLs.

Startup behavior:

1. Validate config at boot.
2. Register selected providers.
3. Emit startup capability banner (selected providers and fallback behavior).

## 7. Phased Migration Plan

## Phase P0: Architecture freeze

1. Freeze provider contracts in ActiveSync.Host.Abstractions.
2. Freeze config schema and provider selection rules.
3. Define compatibility policy for future provider additions.

Exit criteria:

1. Contracts and options reviewed.
2. No provider-specific dependencies in runtime project.

## Phase P1: Runtime composition refactor

1. Introduce ActiveSync.Host.Composition.
2. Move current hardcoded behavior to abstraction-backed services.
3. Keep behavior identical to today (in-memory + existing WS fallback).

Exit criteria:

1. Existing tests still pass.
2. No behavioral regressions in row-19 legacy checks.

## Phase P2: First persistence providers

1. Implement InMemory (baseline/no-op persistence).
2. Implement Sqlite nodes + File blobs provider pair for local durability.
3. Add restart-durability acceptance tests.

Exit criteria:

1. Host restart preserves room state in Sqlite/File mode.
2. Runtime acceptance tests green.

## Phase P3: SpeechSlate target providers

1. Implement Mongo node provider.
2. Implement S3Delegated blob provider.
3. Implement delegate API client for presign get/put with retries and circuit breaker.
4. Validate fallback behavior to WS for presign failures.

Exit criteria:

1. SpeechSlate-shape acceptance scenarios pass.
2. Direct/delegated/fallback evidence captured.

## Phase P4: Optional providers

1. Postgres node provider.
2. S3Direct provider (IAM/direct credentials).
3. JwtBridge auth provider package.

Exit criteria:

1. Integration coverage for each provider.
2. Clear deployment recipes by profile.

## Phase P5: Packaging and operations

1. Publish package matrix and compatibility table.
2. Add health checks per provider.
3. Add migration docs from activesync-server direct deployments.

Exit criteria:

1. Operator runbooks complete.
2. Production readiness review complete.

## 8. Acceptance Matrix Additions

Add provider-specific rows to row-19 evidence stream:

1. Tokened room connect via API mint + hosted runtime.
2. Node durability across restart in selected node provider.
3. Blob upload/download in selected blob provider mode.
4. Delegated presign failure fallback to WS.
5. Subscription/conflict/metrics behavior unchanged across provider modes.

## 9. Risks and Mitigations

1. Risk: provider complexity leaks into runtime loops.
- Mitigation: strict abstractions package and composition layer.
2. Risk: inconsistent behavior across providers.
- Mitigation: shared provider contract tests and profile-based integration tests.
3. Risk: startup misconfiguration.
- Mitigation: fail-fast options validation with explicit diagnostics.

## 10. Recommended First Implementation Slice

1. Create ActiveSync.Host.Abstractions and ActiveSync.Host.Composition.
2. Refactor current runtime to depend on abstractions only.
3. Add InMemory + Sqlite/File providers.
4. Add one profile-driven sample appsettings for each mode.
5. Run row-19 legacy acceptance to confirm no functional drift.

## 11. Decision Summary

1. Yes: token bridge model remains valid for hosted runtime.
2. Yes: persistence/blob concerns should be host/provider-level, not engine-level.
3. Yes: small extender projects/packages are the preferred architecture.
4. Yes: config-driven provider composition is the right operational model.

## 12. JWT Bridge Topology Decision

Question: can JWT bridge behavior live inside the hosted service instead of a separate bridge process?

Answer: yes.

Recommended model:

1. Support both topologies behind one auth provider contract:
- Embedded mode: JWT verification and RoomToken minting run in-process in ActiveSync.DotNetHost.
- Sidecar mode: ActiveSync.DotNetHost calls external JWT bridge HTTP service.
2. Keep sidecar optional, not mandatory.
3. Use the same provider interface and config shape for both to avoid app/runtime drift.

Trade-offs:

1. Embedded mode pros:
- fewer services to deploy
- lower token mint latency
- simpler local development
2. Embedded mode cons:
- auth libraries/keys live in host process
- tighter coupling of auth lifecycle to runtime deployment
3. Sidecar mode pros:
- independent auth scaling/rotation
- cleaner security boundary
4. Sidecar mode cons:
- extra hop and operational surface

Implementation decision for P1:

1. Build provider contract once.
2. Implement embedded provider first.
3. Add sidecar provider as alternate implementation after embedded is stable.

## 13. Phase P0 Backlog (Concrete)

## P0.1 Project scaffolding

1. Create dotnet-host/src/ActiveSync.Host.Abstractions project.
2. Create dotnet-host/src/ActiveSync.Host.Composition project.
3. Reference both from dotnet-host/src/ActiveSync.DotNetHost.
4. Add solution entries and build wiring.

Deliverables:

1. Projects compile in solution.
2. No behavior change in runtime host.

## P0.2 Contracts freeze

1. Add INodeStoreProvider contract:
- LoadRoomSnapshotAsync(roomId)
- PersistAcceptedNodesAsync(roomId, nodes)
- PersistCompactionSnapshotAsync(roomId, snapshot)
2. Add IBlobStoreProvider contract:
- TryGetBlobAsync(hash)
- PutBlobAsync(hash, bytes, contentType)
3. Add IBlobUrlResolverProvider contract:
- ResolvePutUrlAsync(roomId, namespace, hash, sizeBytes, contentType)
- ResolveGetUrlAsync(roomId, namespace, hash)
4. Add IRoomTokenAuthProvider contract:
- ValidateTokenAsync(roomId, tokenPayload)
- Optional MintTokenAsync(request) for embedded/sidecar bridge modes.
5. Add IProviderHealthCheck contract for startup/runtime diagnostics.

Deliverables:

1. Contracts documented in XML comments and markdown appendix.
2. Public versioning policy note for contract evolution.

## P0.3 Config schema freeze

1. Add options classes in abstractions/composition:
- ActiveSyncHostOptions
- NodeStorageOptions
- BlobStorageOptions
- AuthProviderOptions
2. Add provider selection enums/strings with validation.
3. Add fail-fast validator service with clear error messages.
4. Add sample appsettings profile blocks for:
- InMemory + WsOnly
- Sqlite + File
- Mongo + S3Delegated

Deliverables:

1. Options binding tests.
2. Validation tests for invalid provider names and missing required settings.

## P0.4 Baseline tests for freeze

1. Add unit tests for contract DTO serialization/parsing.
2. Add composition tests for provider selection and DI registration.
3. Add snapshot of startup capability banner output.

Exit criteria:

1. P0 solution builds.
2. Contracts/options reviewed and accepted.
3. Runtime project has no new provider-specific package dependency.

## 14. Phase P1 Backlog (Concrete)

## P1.1 Composition refactor in host runtime

1. Add CompositionExtensions.AddActiveSyncHostProviders(IServiceCollection, IConfiguration).
2. Replace direct wiring points in ActiveSync.DotNetHost with abstraction-backed services.
3. Keep existing runtime protocol mapping unchanged.

Deliverables:

1. ActiveSync.DotNetHost starts with default provider profile.
2. Existing runtime endpoints unchanged.

## P1.2 Default providers (no behavior drift)

1. Create ActiveSync.Host.Persistence.InMemory project.
2. Create ActiveSync.Host.Blobs.WsOnly project.
3. Create ActiveSync.Host.Auth.Default project.
4. Register these as defaults when no explicit provider is configured.

Deliverables:

1. Legacy demo behavior remains identical.
2. Existing WS fallback and token pass-through continue to work.

## P1.3 Embedded JWT bridge provider

1. Create ActiveSync.Host.Auth.JwtBridge project.
2. Implement embedded verifier/mint service using configured issuer/audience/key material.
3. Wire as optional auth provider via ActiveSync:Auth:Provider = JwtBridgeEmbedded.
4. Keep API-minted token path supported in parallel for SpeechSlate compatibility.

Deliverables:

1. Embedded provider integration test.
2. Backward compatibility test proving current API token path still works.

## P1.4 Test plan for P1

1. Unit tests:
- DI registration by profile
- auth provider selection behavior
- startup validation
2. Integration tests:
- /ws/runtime hello with valid token across Default and JwtBridgeEmbedded modes
- /ws/runtime hello invalid token rejection behavior
- request-upload/blob-request fallback behavior unchanged in default mode
3. Regression tests:
- dotnet runtime websocket suites still green
- row-19 legacy smoke checks (map/text/list/blob/presence)

Exit criteria:

1. P1 host behavior unchanged under default profile.
2. Embedded JWT bridge mode works under test profile.
3. No regressions in existing runtime endpoint tests.

## 15. First Execution Slice After Planning Approval

1. Implement P0.1 + P0.2 in one PR.
2. Implement P0.3 + P0.4 in second PR.
3. Implement P1.1 + P1.2 in third PR.
4. Implement P1.3 + P1.4 in fourth PR.

Validation command set for each PR:

1. dotnet build dotnet-host/ActiveSync.DotNetHost.slnx
2. dotnet test dotnet-host/ActiveSync.DotNetHost.slnx
3. cargo test -p activesync-host-core
4. cargo test -p activesync-host-ffi
