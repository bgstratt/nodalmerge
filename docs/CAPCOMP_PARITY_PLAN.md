# CAPCOMP Parity Plan — capability-composition vectors across runtimes

Status: **implemented 2026-07-06** (all 6 phases). `nodalmerge-capability-profile`
crate lives at `server/capability-profile/`; jwt-bridge's fork was replaced
with a thin delegating wrapper; `.NET` CapabilityProfileExpander got the
ASCII-fold fix, UTF-8 payload byte counting, and an additive
`TryExpand(..., out errorClass)` overload; 31 vectors in
`engine/commands/capcomp-vectors.v1.json`; harnesses in both runtimes;
CI workflow extended. Verified: full workspace + solution builds and tests
green, mutation check confirmed both harnesses catch drift (Rust: a flipped
default depth limit broke 3 vectors; .NET: reverting the ASCII-fold change
broke the Kelvin-sign vector), and a live end-to-end mint on both the .NET
demo host (`RoomTokenEmbedded` auth provider) and Rust jwt-bridge both
expanded `["room.admin"]` to `["policy.admin","room.admin","tick.admin"]`
via the benchmark profile. The one follow-up found during implementation
(Rust's `limits` struct requiring all 5 sub-fields vs. .NET defaulting
missing ones independently) was closed the same day: each
`CapabilityProfileLimits` field now has its own `#[serde(default = "...")]`
wired to the existing `DEFAULT_*` constants, matching .NET's per-property
defaults exactly. Two vectors (`limits-partial-specification-*`) lock this
in — one proving a single-field override still applies, one proving the
remaining fields still default — and a mutation check (removing the new
`#[serde(default)]` on `max_edges_per_node`) confirmed the ok-path vector
catches the regression (`missing field` parse error). 33 vectors total.

Prerequisite reading: `docs/RESTRUCTURE_AND_PARITY_PLAN.md` §8 (gap table),
the S1 command-registry pattern (`engine/commands/`), and the S5 schema
contract (`docs/PERSISTENCE_SCHEMA.md`) — this plan reuses both patterns.

## 1. Problem

Capability composition (CAPCOMP) expands role-like capabilities
(`room.admin`) through an inheritance DAG defined in a profile JSON file,
at token-mint and token-validate time. The expansion decides what
capabilities land in a signed RoomToken — it is an authorization surface.

It is implemented **three times by hand** with **zero shared coverage**:

| # | Implementation | Lines | `limits` support | Used by |
|---|---|---|---|---|
| A | `server/server/src/capability_profile.rs` | 404 | **yes** | `ws_handler.rs` token-import path (`maybe_expand_profile_capabilities`, ~line 2414); profile loaded from `NODALMERGE_CAPABILITY_PROFILE_PATH` behind a `OnceLock` cache (~line 2318) |
| B | `server/jwt-bridge/src/capability_profile.rs` | 287 | **no** (parses profile, silently ignores `limits`) | jwt-bridge mint path (`lib.rs:182`), same env var, own `OnceLock` cache |
| C | `hosts/dotnet/src/NodalMerge.Host.Composition/CapabilityProfileExpander.cs` | 315 | **yes** | `RuntimeProtocolMapper` (WS client-hello/token path, ~line 3096) and `RoomTokenEmbeddedAuth` (mint); configured via `NodalMerge:Auth:CapabilityComposition` |

B is a stale fork of A. No production consumer enables CAPCOMP today
(neither Studio nor the docs demo host configures it), and the only profile
JSON in the repo is `benchmarks/profiles/capability-profile.v1.json`, whose
`limits` block restates the defaults. **This is the window to converge
before there are compatibility constraints.**

### Verified divergences (read side-by-side 2026-07-05)

1. **`limits` block**: honored by A and C, ignored by B. A profile raising
   `max_dag_depth` works on the server and .NET host but not on jwt-bridge
   minting.
2. **Payload metric**: A and B count UTF-8 bytes
   (`Σ len(cap) + (n-1)` separators); C counts UTF-16 chars via
   `string.Join(",", expanded).Length`. Equal today only because the token
   charset is ASCII — coincidence, not contract.
3. **Case folding**: A and B use `to_ascii_lowercase` (non-ASCII letters
   pass through and then fail the charset check); C uses `ToLowerInvariant`,
   which maps some non-ASCII chars *into* ASCII (Kelvin sign U+212A → `k`),
   so C accepts tokens A/B reject. Accept/reject divergence on an authz
   surface.
4. **Error strings** differ across all three (e.g. "capability profile
   version mismatch" vs "unknown capability_profile_version 'x'"). Nothing
   should ever match on messages; the parity contract must be error
   *classes*.
5. **Validation ordering** is close but unspecified; multi-violation inputs
   can produce different error classes per runtime.

## 2. Decisions (already made — do not re-litigate)

- **Bring `limits` to all runtimes** (it stays in the profile format; B
  gains it via consolidation with A). Additive: no existing profile uses
  non-default limits.
- **Payload metric is UTF-8 bytes** of the sorted, comma-joined flattened
  list. C changes to byte counting (`Encoding.UTF8.GetByteCount`); no
  behavior change for valid ASCII-only tokens, but the spec is bytes.
- **Case folding is ASCII-only** on all runtimes. C changes so that
  non-ASCII input characters are never folded into the accepted charset
  (i.e. fold only `A–Z`→`a–z`, then validate charset). Kelvin-sign tokens
  become rejects on .NET, matching Rust.
- **Trim is Unicode whitespace trim** on all runtimes (Rust `str::trim`,
  .NET `String.Trim()` — already equivalent; keep).
- **Vectors assert error classes, not messages.** Messages may keep their
  current per-runtime wording.
- **Consumer freeze line respected**: no changes to package IDs,
  `NodalMerge.DotNetHost.Ffi`/`Runtime` namespaces, the three extension
  methods, provider interfaces, or WS wire shapes. `TryExpand`'s existing
  signature stays; additions must be additive.

## 3. The canonical algorithm (spec to encode in vectors)

Normalization (`canonicalize`): Unicode-trim → ASCII-lowercase (A–Z only) →
reject if empty, longer than `max_capability_length`, first char not
`[a-z0-9]`, or any char outside `[a-z0-9._-]`.

Expansion, given profile + assigned caps + claimed profile version:
1. **Version gate**: claimed version (trimmed) must be non-empty and equal
   `profile_version` or one of `supported_profile_versions` (each trimmed,
   ordinal compare). Missing/blank → `missing_profile_version`; unknown →
   `profile_version_mismatch`.
2. **Graph build** (profile node order): canonicalize node + parents;
   parent count > `max_edges_per_node` → `too_many_edges`; duplicate
   canonical node → `duplicate_capability`.
3. **Reference check**: every parent must exist as a node →
   `unknown_capability`.
4. **DFS expansion** (assigned order; parents before self; results into an
   ordinal-sorted set): unknown assigned cap → `unknown_capability`;
   revisiting a node on the current path → `cycle`; edge depth >
   `max_dag_depth` (source is depth 0) → `depth_exceeded`. Already-expanded
   nodes short-circuit.
5. **Limits on the result**: size > `max_capability_count` →
   `count_exceeded`; UTF-8 bytes of comma-joined sorted list >
   `max_flattened_payload_bytes` → `payload_exceeded`.
6. Output: lexicographically (ordinal) sorted, deduplicated list.

Limits default to `{count:128, length:128, payload:8192, depth:16,
edges:64}` and are overridable per-profile via the `limits` object.

**Passthrough**: when composition is not configured (no env var / options
disabled), assigned caps are returned **unchanged and unnormalized**. Pin
this with vectors too.

Canonical error classes (exact strings used in the vectors file):
`invalid_profile`, `missing_profile_version`, `profile_version_mismatch`,
`invalid_token`, `duplicate_capability`, `unknown_capability`,
`too_many_edges`, `cycle`, `depth_exceeded`, `count_exceeded`,
`payload_exceeded`.

## 4. Implementation phases

### Phase 1 — Rust consolidation (delete the fork)

1. New leaf crate `server/capability-profile` →
   `nodalmerge-capability-profile` (deps: `serde`, `serde_json`,
   `thiserror` optional). Move implementation A
   (`server/server/src/capability_profile.rs`) into it verbatim; keep the
   public API (`load_capability_profile_from_path/json/value`,
   `flatten_capabilities`, `profile_supports_version`,
   `CapabilityProfile`, `CapabilityProfileLimits`,
   `CapabilityProfileError`). Add
   `CapabilityProfileError::class(&self) -> &'static str` returning the
   canonical class strings above.
2. `nodalmerge-server` re-exports/uses the crate (`ws_handler.rs` imports
   unchanged apart from the `use` path; the env-var `OnceLock` cache stays
   in `ws_handler.rs`).
3. `nodalmerge-jwt-bridge`: delete
   `server/jwt-bridge/src/capability_profile.rs` internals; keep the thin
   `maybe_expand_minted_capabilities(assigned, claimed_version)` wrapper
   (env read + `OnceLock` cache + version gate) delegating to the shared
   crate. jwt-bridge thereby gains `limits` support.
4. Move A's existing unit tests + B's compatibility-window tests into the
   new crate. Add the new crate to workspace `members` and to
   `pack-local-artifacts.ps1` `$crateDirs`/`$crateIds`.

### Phase 2 — .NET alignment

In `CapabilityProfileExpander.cs` (all internal behavior; public signature
additions only):
1. ASCII-only case fold in `NormalizeToken` (fold `A–Z`, then validate;
   do not use `ToLowerInvariant` on the whole string).
2. Payload = `Encoding.UTF8.GetByteCount(string.Join(",", expanded))`.
3. Error classes: introduce
   `public bool TryExpand(..., out IReadOnlyList<string> expanded, out string? error, out string? errorClass)`
   as an overload (or a small result record) — additive; the existing
   overload keeps working and delegates. Every failure path sets one of the
   canonical class strings.
4. Profile-load failures (bad JSON, empty version, no nodes) map to
   `invalid_profile`.

### Phase 3 — Shared vectors file

`engine/commands/capcomp-vectors.v1.json` (same home as `registry.json`;
same "one canonical data file" pattern). Schema:

```json
{
  "schema_version": 1,
  "vectors": [
    {
      "id": "diamond-dedupe",
      "description": "diamond inheritance deduplicates and sorts",
      "mode": "expand",            // "expand" | "passthrough"
      "profile": { "profile_version": "v1", "nodes": [ ... ], "limits": { ... } },
      "assigned": ["room.admin"],
      "claimed_profile_version": "v1",
      "expect": { "ok": ["policy.admin", "room.admin", "tick.admin"] }
      // or: "expect": { "error": "cycle" }
    }
  ]
}
```

`mode: "passthrough"` vectors have no profile and assert
assigned-in == assigned-out (unnormalized).

Vector inventory (~30; single-violation each unless testing precedence):

- **Happy path**: single node; linear chain; diamond dedupe; output sort
  order (input deliberately unsorted); whitespace + uppercase normalization
  in both profile and assigned; duplicate assigned collapses; empty
  assigned list → empty output.
- **Version gate**: exact match; supported-window match; unsupported →
  `profile_version_mismatch`; missing → `missing_profile_version`;
  whitespace-only claimed → `missing_profile_version`; padded claimed
  version trims to match.
- **Token validation**: empty token; token > `max_capability_length`;
  leading `.`; interior illegal char (`:`); **Kelvin sign U+212A** →
  `invalid_token` (the ASCII-fold regression trap); uppercase-with-trim
  accepted (normalization, not rejection).
- **Graph errors**: unknown assigned; unknown inherit reference; duplicate
  node (two spellings normalizing to the same token); self-cycle; 2-node
  cycle; > `max_edges_per_node` edges (use custom limit 2 to keep the
  vector small).
- **Limits** (each via a custom `limits` block so vectors stay small —
  these also prove `limits` parsing on every runtime): depth exceeded
  (chain of 4, `max_dag_depth: 2`); depth at exactly the limit passes;
  count exceeded (`max_capability_count: 2`, expansion of 3); payload
  exceeded (`max_flattened_payload_bytes: 10`); length limit via
  `max_capability_length: 4`.
- **Passthrough**: composition disabled returns caps verbatim, including
  ones that would fail normalization (e.g. `" Room.ADMIN "`).

### Phase 4 — Harnesses

- **Rust**: `server/capability-profile/tests/capcomp_vectors.rs` — loads
  `../../engine/commands/capcomp-vectors.v1.json` (path constant like the
  existing registry tests), runs the version gate + `flatten_capabilities`,
  maps `CapabilityProfileError::class()` to expected classes. One
  additional smoke test in jwt-bridge asserting its wrapper produces the
  same classes for a mint-shaped call (guards the wrapper's own
  version-gate branch).
- **.NET**: `hosts/dotnet/tests/NodalMerge.DotNetHost.Tests/CapcompParityTests.cs`
  — link the vectors file into the test project as content
  (`capcomp-vectors.v1.json`, exactly how `command-registry.json` is linked;
  see the csproj), write each vector's profile to a temp file, construct
  `CapabilityProfileExpander` with it, assert expansion/class via the new
  overload. Passthrough vectors use `Enabled: false`.

### Phase 5 — CI

Extend `.github/workflows/control-plane-capability-parity.yml`:
- Add path filters: `server/capability-profile/**`,
  `server/jwt-bridge/src/**`,
  `hosts/dotnet/src/NodalMerge.Host.Composition/CapabilityProfileExpander.cs`,
  `engine/commands/capcomp-vectors.v1.json`, and the two harness files.
- Rust job: `cargo test -p nodalmerge-capability-profile` (+ existing
  steps). .NET job already runs the test project; no step change needed
  beyond the filters.

### Phase 6 — Verification (definition of done)

1. `cargo build --workspace` and `cargo test -p
   nodalmerge-capability-profile -p nodalmerge-jwt-bridge -p
   nodalmerge-server` green.
2. `dotnet test hosts/dotnet/NodalMerge.DotNetHost.slnx` green (403+ tests
   plus the new harness).
3. **Mutation check** (manual, once): flip one default limit in the Rust
   crate → Rust harness fails; flip the .NET payload counting back to
   chars and add a vector with a 3-byte UTF-8 char near the boundary →
   .NET harness fails. Revert. This proves the vectors bite.
4. Grep: `server/jwt-bridge/src/capability_profile.rs` no longer exists;
   no remaining duplicate flatten implementations
   (`grep -rn "fn flatten" server/ | wc -l` == 1).
5. End-to-end smoke: run the demo host with
   `NodalMerge__Auth__CapabilityComposition__Enabled=true` +
   `ProfilePath=benchmarks/profiles/capability-profile.v1.json`, mint via
   `/sync/token` with `caps=["room.admin"]`, assert the minted token's caps
   contain the flattened set. Mirror on Rust:
   `NODALMERGE_CAPABILITY_PROFILE_PATH` + jwt-bridge mint test.

## 5. Non-goals

- No changes to the RoomToken wire format or WS message shapes.
- No renaming of the .NET options section or env vars.
- No attempt to share *code* across Rust/.NET — parity is enforced by
  vectors, same as the command registry.
- SDK/wasm untouched (CAPCOMP never runs client-side).

## 6. Risk notes for the implementer

- `RuntimeProtocolMapper` constructs a disabled expander in two
  convenience ctors — don't break those (`Enabled: false` must stay a
  cheap no-op that never touches the filesystem).
- The Rust server's `archive_profile_002_object_manifest_parity_reports_p50_p95`
  test is a known pre-existing flake under full `--lib` runs (passes in
  isolation); don't chase it.
- Windows/WSL share `target/`: after any WSL cargo build, run
  `cargo clean --release` before a Windows release build (or you'll hit
  E0514 "incompatible version of rustc").
- The vectors file is versioned (`.v1.json`); breaking schema changes mean
  a new file, not an edit — same convention as the benchmark matrix.
