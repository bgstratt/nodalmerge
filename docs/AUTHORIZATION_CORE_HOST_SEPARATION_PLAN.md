# Authorization Core-Host Separation Plan

Status: Draft for implementation planning
Owner: Core + host runtime streams
Date: 2026-05-21
Execution Tracker: [AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md](AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md)

## 1. Objective

Define a robust authorization architecture that remains:

1. Optional.
2. Offline-first.
3. Low latency.
4. Deterministic and replay-stable.
5. Transport-agnostic (WS, WebRTC, host relays).
6. Compatible with decentralized and authoritative room modes.
7. Ready for multi-surface packaging (crate, NuGet, npm) and future hosts (Java or others).

This plan explicitly separates core authorization semantics from host identity integration.

## 2. Non-Goals

1. Enterprise RBAC graph semantics in core (groups, nested roles, org hierarchy).
2. Per-op remote auth callouts.
3. Centralized-only operation model.
4. JWT/OIDC provider logic inside core.
5. Product-specific tenancy rules in core.

## 3. Architecture Boundary

| Layer | Responsibility |
| ----- | -------------- |
| Core (activesync-core) | Deterministic authorization semantics and enforcement at op ingress |
| Host (Rust server, DotNetHost, future Java host) | Authentication, token issuance/validation orchestration, policy provisioning |
| SDK (npm and future SDKs) | Ergonomics, local intent APIs, policy convenience helpers |

Rule: core evaluates whether an op is allowed. Host decides how identities become capabilities and policies.

## 4. Guiding Principles

1. Enforce authorization before merge.
2. Unauthorized ops never enter canonical graph state.
3. Policy evaluation must be pure, local, deterministic, and synchronous.
4. Write authorization is convergence-critical and belongs in core.
5. Read projection/privacy is host and SDK policy unless cryptographic read controls are enabled.
6. Authority is optional and composable, not mandatory.
7. Policies are state and must be replayable/versioned.
8. Keep policy evaluator branch-light and allocation-light.

## 5. Current Baseline (as of plan date)

1. Core already includes RoomToken primitives and signature verification.
2. Core already includes Policy, PolicyRule, PolicyDefault.
3. Core already enforces can_write in apply_remote/apply_remote_batch path.
4. Rust host supports room lock token verification and set-policy commands.
5. Rust host supports optional authoritative tick loop.
6. DotNetHost supports pluggable auth providers: Default, JwtBridgeEmbedded, JwtBridgeSidecar.
7. JWT bridge crate exists for issuer JWT -> RoomToken minting.

This means the target model is an extension and hardening of existing direction, not a greenfield rewrite.

## 6. Target Authorization Model

### 6.1 Core model

Core owns deterministic authorization primitives:

1. Op identity fields used for validation and provenance.
2. Signature verification.
3. Policy schema and matcher.
4. can_apply(op, policy) semantics.
5. Ingress enforcement in remote apply pipeline.
6. Policy replication and versioning semantics.
7. Rejection reasons suitable for observability.

### 6.2 Host model

Host owns identity and integration:

1. JWT/OIDC/API-key/custom auth validation.
2. Mapping identity claims -> capabilities.
3. Policy source of truth provisioning for room/session.
4. Admin controls for who may set/replace policy.
5. Session-level admission and token lifecycle.
6. Policy update channel mode selection:
	- `admin_command_only`
	- `replicated_signed_ops_only`
	- `hybrid`
7. Profile-based defaults:
	- auth-enabled hosted authority profile defaults to `admin_command_only`
	- decentralized/distributed profile may default to `hybrid`
8. Capability inheritance/composition (when enabled) is host-side only; core does not expand capabilities.
9. Host expansion model is additive-only with explicit DAG edges.
10. Host fully flattens expanded capabilities before token signing.
11. Tokens must not embed inheritance graphs or expansion logic.
12. Capability profile version is host-issued and explicit to preserve issuance-time semantics.

### 6.3 SDK model

SDK owns developer ergonomics:

1. Intent vs state write API shape.
2. Optional optimistic local prediction.
3. Policy helper methods and generated typed clients.
4. Transport-specific behavior hidden behind common API.

### 6.4 Capability composition contract (post-P4)

1. Phase 1 runtime contract remains unchanged:
	- core evaluates flattened capability string presence + namespace rule matching only
	- no inheritance semantics in core
2. Phase 2 adds host-side expansion only:
	- additive-only grants (no deny semantics)
	- explicit DAG edges only (no wildcard implication)
	- deterministic flattened output before signing
3. Flattened capability grammar is strict and portable:
	- regex: `[a-z0-9][a-z0-9._-]{0,127}`
	- lowercase only
	- separators limited to `.`, `_`, `-`
	- no spaces, no unicode, no wildcard tokens in capability identities
4. Canonicalization before signing is mandatory:
	- trim
	- lowercase normalization
	- validate charset/length
	- deduplicate
	- lexicographic ascending sort
5. Profile/version behavior:
	- host capability graph is immutable per profile version
	- profile changes are versioned, not in-place semantic mutation
	- tokens preserve issuance-time flattened semantics until expiry
	- unknown profile version deterministically rejects (no silent fallback)
6. Runtime token vs audit provenance split:
	- runtime token remains minimal (actor, flattened caps, expiry, signature, profile version)
	- expansion provenance/audit chain is external or optional artifact, not required in every token/op
7. Initial safety limits (host-enforced):

| Limit | Initial value |
| ----- | ------------- |
| capability count | 128 |
| capability string length | 128 chars |
| flattened token capability payload | 8 KB |
| DAG max depth | 16 |
| max inheritance edges per node | 64 |

8. Graph validation requirements:
	- detect and reject cycles
	- reject malformed/ambiguous graph definitions
	- prefer configuration-load-time validation over issuance-time failures

## 7. Operational Semantics

### 7.1 Ingress pipeline (required)

1. Receive op.
2. Verify hash/integrity.
3. Verify signature.
4. Evaluate policy/capability deterministically.
5. Accept or reject.
6. Only accepted op mutates canonical graph.

### 7.2 Intent vs canonical state split

1. Intent namespaces are client writable when policy allows (example: intent/**).
2. Canonical protected namespaces are authority writable only (example: world/**, economy/**).
3. Authority host may transform accepted intents into canonical writes.
4. This allows low latency UX while preserving authoritative correctness.

### 7.3 Namespace classes (minimal core profile)

Phase-1 classes:

1. server-only
2. owner-only
3. shared
4. capability-required

These map to concrete policy rules and remain simpler than full RBAC.

## 8. Policy as Replicated State

1. Policy updates are represented as signed replicated state/events.
2. Policy has a version or monotonic sequence for audit and replay clarity.
3. Replay applies policy timeline and data timeline in deterministic order.
4. Historical replay must honor policy in effect at that replay point.

## 9. Security and Hardening Requirements

1. Restrict policy mutation and authority controls to authorized actors only.
2. Distinguish control-plane capabilities from data-plane capabilities.
3. Enforce token expiry mid-session in locked rooms.
4. Keep deny reasons bounded and normalized for metric labels.
5. Audit rejected ops separately from canonical graph.
6. Ensure policy bypass is impossible through alternate transport paths.

## 10. Packaging and Integration Strategy

### 10.1 Crate (Rust)

1. Keep activesync-core free of JWT/OIDC dependencies.
2. Keep policy/token APIs stable and host-friendly.
3. Expose policy evaluator and rejection reason surface suitable for hosts.

### 10.2 NuGet (.NET)

1. Keep provider model with auth pluggability.
2. Maintain provider-neutral IRoomTokenAuthProvider style contracts.
3. Keep embedded and sidecar auth options as host-level composition.
4. Add policy administration hooks with explicit auth guardrails.

### 10.3 npm (SDK)

1. Expose capability-friendly policy helper APIs.
2. Preserve local-first optimistic write experience for intent paths.
3. Surface deterministic rejection reasons to app code.
4. Keep SDK transport abstraction independent from auth provider internals.

### 10.4 Future Java host pattern

1. Implement host auth adapter interface equivalent to .NET provider contract.
2. Reuse same token wire shape and policy semantics.
3. Reuse same control-plane endpoints/contracts (mint, validate, policy set if enabled).
4. Keep Java host as identity translator and policy provisioner, not policy engine fork.

## 11. Compatibility Contract

A host in any language is conformant if it guarantees:

1. Signature and policy checks happen before op merge.
2. Unauthorized ops do not enter canonical graph.
3. Policy semantics match core rules exactly.
4. Token admission semantics match RoomToken contract.
5. Policy update authority is enforced and auditable.

## 12. Rollout Plan

## P0: Consolidate and document baseline

1. Publish this separation contract as canonical architecture reference.
2. Inventory all ingress points and verify they route through policy enforcement.
3. Define standard rejection reason taxonomy.

Exit criteria:

1. Single source of truth document accepted.
2. Ingress point inventory complete.

## P1: Control-plane hardening

1. Add explicit authorization for set-policy, set-room-key, start-tick, stop-tick.
2. Introduce control-plane capability keys (example: policy.admin, room.admin, tick.admin).
3. Add tests for unauthorized control-plane command rejection.

Exit criteria:

1. Unauthorized control-plane mutation is impossible in tests.
2. Deterministic denial reasons are emitted.

## P2: Policy replication and replay correctness

1. Define policy version timeline semantics.
2. Ensure replay and compaction preserve policy history semantics.
3. Add replay tests proving historical policy behavior is stable.

Exit criteria:

1. Replay determinism holds across policy transitions.

## P3: SDK ergonomics and developer APIs

1. Add high-level policy helper APIs in SDK.
2. Add intent vs canonical helper patterns.
3. Add docs and examples for authority optionality and offline behavior.

Exit criteria:

1. Common scenarios are implementable without low-level policy wiring.

## P4: Multi-host conformance kit

1. Publish host conformance test suite for auth/policy behavior.
2. Run suite against Rust host and DotNetHost.
3. Define Java host adapter checklist and starter template.

Exit criteria:

1. Conformance suite passes on existing hosts.
2. New host implementation path is documented and repeatable.

## 13. Test Matrix

1. Core unit tests:
- policy match semantics
- can_apply deterministic behavior
- unauthorized op rejection
2. Core property tests:
- order-independent convergence under allowed-op sets
3. Host integration tests:
- token admission success/failure paths
- control-plane auth gating
- policy enforcement in live ws sessions
4. Replay tests:
- policy transition timeline correctness
5. SDK tests:
- optimistic intent behavior with authoritative refinement
- rejection propagation behavior

## 14. Open Decisions

1. Resolved: Policy timeline encoding format and migration strategy (see [POLICY_TIMELINE_ENCODING_DECISION.md](POLICY_TIMELINE_ENCODING_DECISION.md)).
2. Control-plane capability naming conventions.
3. Resolved: Policy update channel is host-configurable (`admin_command_only`, `replicated_signed_ops_only`, `hybrid`) with profile-based defaults (hosted auth-enabled authority defaults to `admin_command_only`).
4. Resolved: Minimal actor identity normalization is deterministic and host-enforced (trim + lowercase normalization before deterministic comparison/signing contracts where applicable).
5. Resolved: Capability inheritance/composition remains outside core evaluator complexity and is host-side flattening only.
6. Deferred explicitly: deny semantics, wildcard inheritance, runtime expansion, dynamic policy expressions.

## 15. Immediate Next Steps

1. Track all execution status in [AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md](AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md).
2. Approve this layer boundary and rollout phases.
3. Use P0 artifacts as implementation baseline:
	- [AUTHORIZATION_INGRESS_INVENTORY.md](AUTHORIZATION_INGRESS_INVENTORY.md)
	- [AUTHORIZATION_REJECTION_TAXONOMY.md](AUTHORIZATION_REJECTION_TAXONOMY.md)
4. Add capability composition host-profile spec and graph schema (DAG edges, versioning, normalization rules).
5. Add conformance vectors for capability expansion parity (positive and negative vectors).
6. Add host-side expansion validation harness for cycle/overflow/unknown-profile failures.
7. Keep core evaluator unchanged and benchmark-check auth path regressions after host expansion rollout.
8. Use [AUTHORIZATION_CAPABILITY_PROFILE_SCHEMA.md](AUTHORIZATION_CAPABILITY_PROFILE_SCHEMA.md) as the concrete host profile schema baseline.
