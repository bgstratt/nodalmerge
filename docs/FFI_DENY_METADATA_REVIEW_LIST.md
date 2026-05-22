# FFI Deny Metadata Review List

Status: Proposed (review before implementation)
Owner: Host FFI + DotNetHost runtime maintainers
Last Updated: 2026-05-21
Companion Tracker: [AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md](AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md)

## 1. Goal

Enable FFI ingress to emit concrete control-plane deny metric labels (`command`, `required_capability`) with the same fixed-enum policy used by runtime websocket ingress.

## 2. Recommended Implementation Strategy

1. Keep existing ABI functions unchanged (`as_host_submit_command`, `as_host_submit_command_json`).
2. Introduce new opt-in ABI functions with metadata output:
   - `as_host_submit_command_ex`
   - `as_host_submit_command_json_ex`
3. Return deny metadata only for authorization/policy denial cases; otherwise metadata is empty.
4. DotNetHost FFI loop uses `_ex` functions when available and falls back to current behavior if unavailable.

## 3. Recommended Metadata Shape

Use fixed enums only to avoid cardinality drift.

### 3.1 Deny reason class

1. `reject.control_plane_forbidden`
2. `reject.policy_violation`
3. `unknown`

### 3.2 Command label

1. `set-policy`
2. `set-room-key`
3. `start-tick`
4. `stop-tick`
5. `unknown`

### 3.3 Required capability label

1. `policy.admin`
2. `room.admin`
3. `tick.admin`
4. `unknown`

### 3.4 Data contract fields

1. `reason_class`
2. `command`
3. `required_capability`
4. `deny_message` (optional string for debugging only; never used as metric label)

## 4. ABI/Compatibility Checklist

1. Do not change existing struct sizes or signatures used by current clients.
2. Version-gate `_ex` usage in DotNetHost.
3. Keep `as_status` semantics unchanged.
4. Ensure metadata buffer ownership/free rules match existing `as_bytes_owned_free` behavior.

## 5. DotNetHost Integration Checklist

1. Add bridge surface that can return optional deny metadata with status.
2. In FFI websocket loop, emit `runtime_control_plane_denied_total` with:
   - `host=dotnet-host`
   - `command` from metadata enum
   - `required_capability` from metadata enum
   - `reason_class` from metadata enum
3. If metadata missing, preserve current fallback (`command=ffi`, `required_capability=unknown`).

## 6. Test Plan Checklist

1. host-ffi ABI tests verify `_ex` function success/deny metadata payload shape.
2. DotNetHost unit tests verify metric labels from metadata for at least:
   - `set-policy` denied
   - `set-room-key` denied
3. DotNetHost fallback test verifies non-metadata path still emits `ffi`/`unknown` labels.
4. Regression test confirms no behavior changes for existing non-`_ex` callers.

## 7. Recommended Phased Rollout

1. Phase A: add `_ex` ABI + tests in host-ffi.
2. Phase B: add DotNetHost bridge support + metric mapping + tests.
3. Phase C: switch DotNetHost runtime to `_ex` by default, keep fallback.
4. Phase D: update tracker evidence and conformance notes.

## 8. Review Decisions Needed

1. Confirm enum sets in sections 3.1-3.3.
2. Confirm whether `deny_message` should be included for diagnostics.
3. Confirm whether metadata should be JSON bytes or postcard bytes in ABI.
4. Confirm rollout scope is DotNetHost first, other hosts later.

## 9. Locked Decisions (2026-05-21)

1. Enum sets in sections 3.1-3.3 are approved.
2. `deny_message` is included as an optional diagnostics field.
3. Metadata encoding for `_ex` ABI is JSON bytes.
4. Rollout scope is DotNetHost first, then other hosts.
