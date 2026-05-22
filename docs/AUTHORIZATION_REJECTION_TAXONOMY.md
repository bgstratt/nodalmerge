# Authorization Rejection Taxonomy

Status: Draft (P0 deliverable)
Owner: Core + host runtime streams
Last Updated: 2026-05-21
Companion Tracker: [AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md](AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md)

## 1. Purpose

Define canonical rejection reasons and label policy for deterministic behavior, observability, and cross-host parity.

## 2. Design Rules

1. Reasons are stable identifiers, not free-form prose.
2. Metrics labels use a bounded allowlist.
3. Error payloads may include human-readable detail, but reason code remains canonical.
4. Unauthorized operations are rejected before merge.

## 3. Canonical Reason Codes

## 3.1 Admission and identity

1. unauthorized.token_missing
2. unauthorized.token_invalid
3. unauthorized.token_expired
4. unauthorized.token_peer_mismatch
5. unauthorized.token_room_mismatch
6. unauthorized.capability_mismatch

## 3.2 Signature and integrity

1. reject.hash_mismatch
2. reject.invalid_signature
3. reject.missing_parent
4. reject.lamport_ceiling
5. reject.wall_clock_skew

## 3.3 Policy and authorization

1. reject.policy_violation
2. reject.control_plane_forbidden
3. reject.path_scope_forbidden
4. reject.authority_required

## 3.4 Protocol and payload shape

1. reject.protocol_invalid_message
2. reject.protocol_invalid_field
3. reject.protocol_unsupported_command
4. reject.protocol_message_too_large

## 3.5 Runtime protection and backpressure

1. reject.rate_limit_nodes
2. reject.rate_limit_bytes
3. reject.broadcast_lagged_resync_required
4. reject.server_overload

## 4. Mapping Guidance

1. Core SyncError::PolicyViolation -> reject.policy_violation.
2. Token verification failures -> unauthorized.* family.
3. Control-plane denied actions -> reject.control_plane_forbidden.
4. Signature/hash failures -> reject.invalid_signature or reject.hash_mismatch.
5. Rate limit close paths -> reject.rate_limit_nodes or reject.rate_limit_bytes.

## 5. Metrics Label Policy

1. Use reason_code label with values from this file only.
2. Do not emit dynamic strings as reason labels.
3. Keep human detail in logs and debug payloads.

Example:
1. auth_denied_total{reason_code="unauthorized.token_expired"}
2. op_rejected_total{reason_code="reject.policy_violation"}

## 6. Protocol Envelope Recommendation

Error envelope shape:
1. code: stable machine code from taxonomy.
2. message: short human-readable summary.
3. detail: optional contextual field for debugging (not for labels).

## 7. Cross-Host Conformance Rule

Rust host, DotNetHost, and future hosts must:
1. map equivalent failures to the same reason code.
2. preserve pre-merge rejection semantics.
3. avoid host-specific reason drift in shared telemetry.

## 8. Next Actions

1. Apply this taxonomy in Rust control-plane deny responses (P1).
2. Apply equivalent mapping in DotNetHost runtime error paths (P1).
3. Add conformance tests asserting reason-code parity across hosts (P4).
