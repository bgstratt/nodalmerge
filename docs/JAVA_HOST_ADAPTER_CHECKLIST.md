# Java Host Adapter Checklist (P4 Starter)

Status: Draft starter checklist
Companion tracker: [AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md](AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md)

## Purpose

Define the minimum contract a Java host adapter must satisfy to be authorization-conformant with Rust host and DotNetHost.

## Required Contract Inputs

1. [AUTHORIZATION_CONTROL_PLANE_CAPABILITIES.md](AUTHORIZATION_CONTROL_PLANE_CAPABILITIES.md)
2. [AUTHORIZATION_REJECTION_TAXONOMY.md](AUTHORIZATION_REJECTION_TAXONOMY.md)
3. [AUTHORIZATION_CONFORMANCE_SPEC.md](AUTHORIZATION_CONFORMANCE_SPEC.md)
4. [acceptance/authz-conformance-vectors.json](acceptance/authz-conformance-vectors.json)

## Ingress Requirements

1. All mutation ingress paths must enforce authorization before merge.
2. Control-plane commands map to canonical capability keys:
   - `set-policy` -> `policy.admin`
   - `set-room-key` -> `room.admin`
   - `start-tick` -> `tick.admin`
   - `stop-tick` -> `tick.admin`
3. Rejection reasons must use canonical reason classes and deterministic deny messaging.

## Replay/Compaction Requirements

1. Replay must support policy-at-time timeline semantics.
2. Snapshot compatibility checks must honor policy timeline metadata where present.
3. Timeline cutover behavior at lamport boundaries must match core semantics.

## Deny Surface Requirements

1. When structured metadata is available, emit `reason_class`, `command`, and `required_capability`.
2. Preserve compatibility fallback behavior when metadata is unavailable.
3. Metrics labels for deny counters must use fixed enums.

## Conformance Execution Requirements

1. Implement vector loader for [acceptance/authz-conformance-vectors.json](acceptance/authz-conformance-vectors.json).
2. Emit normalized results by `vector_id` and expected fields.
3. Run same vectors across runtime and bridge/ffi-equivalent ingress paths.
4. Compare results against Rust host and DotNetHost parity baselines.

## Exit Checklist

1. All required vectors pass.
2. No reason-class drift from canonical taxonomy.
3. No capability-name drift from canonical contract.
4. Adapter review signed off by architecture group.
