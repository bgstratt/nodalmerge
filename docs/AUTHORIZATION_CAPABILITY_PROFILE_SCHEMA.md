# Authorization Capability Profile Schema

Status: Draft v1 (host-side rollout scaffold)
Owner: Host runtime streams
Date: 2026-05-21
Companion plan: [AUTHORIZATION_CORE_HOST_SEPARATION_PLAN.md](AUTHORIZATION_CORE_HOST_SEPARATION_PLAN.md)

## 1. Purpose

Define a concrete host-side schema for capability inheritance/composition profiles used to expand assigned capabilities into deterministic flattened sets before token signing.

Non-goals:

1. Core evaluator changes.
2. Deny semantics.
3. Wildcard inheritance or dynamic expressions.

## 2. Contract Summary

1. Composition is host-side only.
2. Graph model is additive explicit DAG edges only.
3. Runtime token stores flattened capabilities only.
4. Profile version is explicit and immutable-by-version.
5. Unknown profile version rejects deterministically.

## 3. JSON Schema Shape (v1)

Top-level fields:

1. `profile_version` (string, required)
2. `supported_profile_versions` (array<string>, optional)
3. `nodes` (array, required)
4. `limits` (object, optional)

Node fields:

1. `capability` (string, required)
2. `inherits` (array<string>, optional, defaults to `[]`)

Limits fields (optional overrides; defaults are required when omitted):

1. `max_capability_count` (int)
2. `max_capability_length` (int)
3. `max_flattened_payload_bytes` (int)
4. `max_dag_depth` (int)
5. `max_edges_per_node` (int)

Example profile:

```json
{
  "profile_version": "capprof-v1",
  "supported_profile_versions": ["capprof-v0"],
  "limits": {
    "max_capability_count": 128,
    "max_capability_length": 128,
    "max_flattened_payload_bytes": 8192,
    "max_dag_depth": 16,
    "max_edges_per_node": 64
  },
  "nodes": [
    {
      "capability": "room.admin",
      "inherits": ["tick.admin", "policy.admin"]
    },
    {
      "capability": "tick.admin",
      "inherits": []
    },
    {
      "capability": "policy.admin",
      "inherits": []
    }
  ]
}
```

## 4. Capability Token Grammar

Canonical capability token regex:

1. `[a-z0-9][a-z0-9._-]{0,127}`

Rules:

1. Lowercase only.
2. First character must be ASCII lowercase or digit.
3. Remaining characters may be ASCII lowercase, digits, `.`, `_`, `-`.
4. No whitespace, unicode, wildcard tokens, or path globs.

## 5. Deterministic Flattening Algorithm

Input:

1. Assigned capability list from host identity mapping.
2. Profile graph for `capability_profile_version`.

Algorithm:

1. Canonicalize each assigned capability (`trim` + lowercase).
2. Validate assigned capabilities and graph tokens against grammar and length limit.
3. Validate requested `capability_profile_version` against `profile_version` and optional `supported_profile_versions` compatibility window.
4. Validate graph references and constraints (edges/depth/cycles).
5. DFS/BFS expand additive inheritance closure.
6. Deduplicate expanded set.
7. Lexicographically sort ascending.
8. Enforce count and payload limits.
9. Emit flattened list for token signing.

Output invariants:

1. Stable for equivalent inputs across hosts.
2. Same profile version + same assigned list -> same flattened sequence.
3. Token payload contains only flattened capability list, not graph provenance.

## 6. Validation Error Categories

Deterministic failure categories (host-local error details may vary; reason class must remain stable in conformance):

1. `unknown_profile_version`
2. `invalid_profile_schema`
3. `invalid_capability_token`
4. `duplicate_capability_node`
6. `unknown_capability_reference`
7. `cycle_detected`
8. `depth_limit_exceeded`
9. `edge_limit_exceeded`
10. `capability_count_limit_exceeded`
11. `payload_size_limit_exceeded`

Conformance mapping for current rollout:

1. Reject class: `reject.protocol_violation`
2. Command label: `capability-composition`

## 7. Default Limits (v1)

1. `max_capability_count`: 128
2. `max_capability_length`: 128
3. `max_flattened_payload_bytes`: 8192
4. `max_dag_depth`: 16
5. `max_edges_per_node`: 64

## 8. Versioning Rules

1. `profile_version` is immutable once published.
2. Semantic changes require a new `profile_version` value.
3. `supported_profile_versions` (when present) is an explicit bounded compatibility window for rolling upgrades.
4. Existing tokens preserve issuance-time flattened semantics until expiry.
5. Unknown profile versions reject without fallback when not present in the compatibility window.

## 9. Rollout Notes

1. P4 release gate remains unchanged.
2. CAPCOMP vectors run as supplemental profile tests first.
3. Promote CAPCOMP vectors to release-gating only after Rust host + DotNetHost parity is stable.
