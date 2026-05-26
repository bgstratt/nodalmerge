# NodalMerge Open-Core Packaging Strategy

Status: Proposed execution plan
Owner: Runtime + host packaging stream
Date: 2026-05-20

## 1. Product Boundary

Open source (adoption surface):
1. Runtime core (`activesync-core`)
2. Transport adapters and protocol compatibility layers
3. Local persistence adapters and offline queue behavior
4. Replay engine and deterministic canonical hash
5. Developer-facing diagnostics and baseline topology visibility

Paid / hosted (monetization surface):
1. Hosted orchestration and room lifecycle automation
2. Enterprise topology management
3. Deep observability and convergence debugging workflows
4. Scaling layer and managed relay infrastructure
5. Advanced merge tooling and policy workflows
6. Managed auth/security controls and analytics

Decision principle:
1. Keep deterministic state semantics and protocol primitives open.
2. Monetize operational excellence, managed reliability, and enterprise controls.

## 2. Why This Split Works

The compounding moat is operations and distributed-systems expertise:
1. Topology behavior under churn and partitions
2. Convergence diagnostics at scale
3. Failure-mode handling and rollout patterns
4. Security/compliance lifecycle management

Most stacks can sync data. Few can explain and audit convergence in production.

## 3. Sequencing

## Phase 1: Package the Runtime (Adoption)

Ship package-grade artifacts with a dead-simple surface:
1. room
2. sync
3. replay
4. offline
5. CAS
6. topology

Artifacts:
1. npm package wrappers (`nodalmerge-bridge`, `nodalmerge-sdk-js`) as primary identities
2. legacy npm package names (`activesync-bridge`, `activesync-sdk-js`) retained during migration window
3. NuGet managed + native packages (`ActiveSync.Host.*`, `ActiveSync.DotNetHost.Native.*`)
4. NuGet wrapper package identities (`NodalMerge.Host.*`, `NodalMerge.DotNetHost.Native.*`) during migration
5. Rust crates (`activesync-core`, `activesync-host-core`, `activesync-host-ffi`, `activesync-host-axum`, `activesync-bridge`) plus wrapper crates (`nodalmerge-core`, `nodalmerge-gc`, `nodalmerge-host-core`, `nodalmerge-host-axum`, `nodalmerge-host-ffi`, `nodalmerge-server`, `nodalmerge-jwt-bridge`, `nodalmerge-s3-blobs`)

Compatibility note:
1. Legacy `ActiveSync.*` NuGet IDs and `activesync-*` crate/package names remain supported during migration window.

Pre-publish validation rule:
1. Always validate package consumption from a local feed (`artifacts/nuget-local`) before pushing to public/private remote feeds.

Definition of done:
1. Versioned artifacts are consumable from package feeds (no source checkout required).
2. Quickstart docs exist for JavaScript, .NET, and Rust.
3. Tactical showcase can run against package artifacts only.

## Phase 2: Hosted Relay / Sync Service (Monetization)

Managed service capability set:
1. Room coordination and tenancy controls
2. Durable persistence and replay history
3. Asset synchronization and policy enforcement
4. Peer discovery and topology-aware routing
5. Operational visibility, alerts, and analytics

Definition of done:
1. Service has clear free tier vs paid tier limits.
2. SDK/host packages can target self-hosted or managed endpoints with config-only changes.
3. Operational runbooks and SLOs exist for hosted rollout.

## 4. Package Matrix

## 4.1 npm

Package:
1. `nodalmerge-bridge` (primary wrapper package)
2. `activesync-bridge` (legacy compatibility package)

Minimal API profile:
1. initialize runtime/store
2. room/session hello and sync
3. replay timeline access
4. offline queue and flush
5. CAS put/get
6. topology snapshot stream

## 4.2 NuGet

Managed packages:
1. `NodalMerge.Host.Abstractions` (primary wrapper package identity)
2. `NodalMerge.Host.Composition` (primary wrapper package identity)

Native packages:
1. `NodalMerge.DotNetHost.Native.win-x64` (primary wrapper package identity)
2. `NodalMerge.DotNetHost.Native.linux-x64` (primary wrapper package identity)

Compatibility package IDs:
1. `ActiveSync.Host.Abstractions`
2. `ActiveSync.Host.Composition`
3. `ActiveSync.DotNetHost.Native.win-x64`
4. `ActiveSync.DotNetHost.Native.linux-x64`

Notes:
1. Keep package IDs stable and aligned with existing CI workflow.
2. Expand RID coverage after parity stabilization.

## 4.3 Crates

Crates to publish:
1. `activesync-core`
2. `activesync-host-core`
3. `activesync-host-ffi`
4. `activesync-host-axum`
5. `activesync-s3-blobs`
6. `nodalmerge-core` (wrapper)
7. `nodalmerge-gc` (wrapper)
8. `nodalmerge-host-core` (wrapper)
9. `nodalmerge-host-axum` (wrapper)
10. `nodalmerge-host-ffi` (wrapper)
11. `nodalmerge-server` (wrapper)
12. `nodalmerge-jwt-bridge` (wrapper)
13. `nodalmerge-s3-blobs` (wrapper)

Notes:
1. Remove path-only assumptions before publish.
2. Preserve deterministic behavior parity test gates.

## 5. API Shape Guardrails

The package-first API should optimize for first-run success:
1. Fewer concepts on first contact (room/sync/replay/offline/CAS/topology)
2. Strong defaults with explicit advanced hooks
3. Transport and storage pluggability without forcing host-specific internals

Non-goals for Phase 1:
1. Full enterprise control-plane parity
2. Commercial analytics UX
3. Cross-cloud multi-region orchestration features

## 6. Tactical Showcase Pivot

Goal:
1. Showcase consumes packaged managed/native runtime artifacts instead of sibling source coupling.

Required changes:
1. Add package-feed setup doc and lock package IDs/versions used by showcase.
2. Keep source fallback for local contributor workflows only.
3. Add acceptance script that proves package-only startup on Windows and Linux.

Acceptance checks:
1. Host health endpoint reports runtime available with only package artifacts installed.
2. Runtime websocket flow works with no local `../activesync` repo dependency.
3. Demo script remains deterministic across restarts.

## 7. Packaging Release Gates

Gate A: Artifact correctness
1. Version and metadata correctness
2. Native asset layout validation by RID
3. Symbol/source package policy where applicable

Gate B: Runtime parity
1. Baseline host/runtime test suites pass
2. Replay hash parity checks pass
3. Sidecar and embedded smoke checks pass

Gate C: Consumer validation
1. Fresh machine bootstrap from packages only
2. Tactical showcase package-only runbook validated
3. Upgrade/rollback workflow validated across at least two versions

## 8. Immediate Next Actions

1. Freeze package IDs and first public versioning policy.
2. Publish package-consumer quickstarts per language/runtime.
3. Add tactical showcase package-only mode and acceptance checklist.
4. Start hosted service definition with explicit free/paid boundaries and quota model.

Execution runbook:
1. `docs/PACKAGING_PUBLISH_RUNBOOK.md`
