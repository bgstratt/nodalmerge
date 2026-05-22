# Authorization Control-Plane Capabilities

Status: Draft (P1 contract)
Owner: Core + host runtime streams
Last Updated: 2026-05-21
Companion Tracker: [AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md](AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md)

## 1. Purpose

Define canonical capability keys for control-plane runtime commands so Rust host, DotNetHost, and future hosts enforce the same semantics.

## 2. Canonical Capability Keys

1. `policy.admin`
   - Grants permission to submit `set-policy`.
2. `room.admin`
   - Grants permission to submit `set-room-key`.
3. `tick.admin`
   - Grants permission to submit `start-tick` and `stop-tick` where those commands exist.

## 3. Command Mapping

1. `set-policy` -> requires `policy.admin`
2. `set-room-key` -> requires `room.admin`
3. `start-tick` -> requires `tick.admin`
4. `stop-tick` -> requires `tick.admin`

## 4. Denial Semantics

1. Deterministic deny reason prefix is `reject.control_plane_forbidden`.
2. Current wire behavior uses message-string errors, for example:
   - `reject.control_plane_forbidden: command=set-policy requires=policy.admin`
   - `reject.control_plane_forbidden: command=set-room-key requires=room.admin`
3. Structured code fields may be introduced in a future phase without changing required capability mapping.

## 5. Host Scope Notes

1. Rust host currently supports all four mapped commands.
2. DotNetHost enforces mapping for `set-policy`, `set-room-key`, `start-tick`, and `stop-tick` with canonical capability checks (`policy.admin`, `room.admin`, `tick.admin`).

## 6. Cardinality and Metrics Constraints

1. Metrics labels for command/capability/reason must be fixed-enum values from this contract and rejection taxonomy.
2. Do not emit free-form dynamic labels from user payload values.
