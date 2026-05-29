# E2EE Future-State Plan (Deferred Post-Stable)

Owner: Platform/runtime  
Status: Deferred (design frozen; implementation intentionally paused)  
Last updated: 2026-05-29

## Purpose

Capture the agreed end-to-end encryption (E2EE) direction for NodalMerge while holding implementation until after a stable release baseline is shipped.

This plan is intentionally design-only for now so that current effort can prioritize:

1. stable runtime and release packaging
2. docs/playgrounds/tutorials/demos/onboarding
3. downstream AI implementation tracks

## Freeze decision for current release cycle

E2EE implementation work is tabled until after stable release.  
The architecture remains extensible, but no additional E2EE runtime/protocol churn is planned in this cycle.

## Design principles for v1 E2EE

Prioritize:

1. determinism
2. interop simplicity
3. operational survivability

De-prioritize early crypto flexibility/multi-suite agility.

## Agreed v1 recommendations

### 1) Single v1 cipher suite

Launch with exactly one valid suite (`cryptoVersion: 1`, `cipherSuite: NM-E2EE-1`).  
Do not support suite negotiation or multiple suites in v1.

Directional profile:

1. X25519 for ECDH
2. Ed25519 identities/signatures
3. HKDF-SHA256
4. ChaCha20-Poly1305 (or AES-GCM only if enterprise/FIPS pressure requires)
5. deterministic canonical encoding before encrypt/sign

Future suites remain possible via new versions (for example `NM-E2EE-2`) without adding v1 matrix complexity.

### 2) Behavior when room policy is `off`

Reject encrypted ops deterministically.  
Do not accept-and-ignore.

Reasoning:

1. preserves convergence invariants
2. avoids silent security footguns
3. avoids storage/relay/hash ambiguity across peers

Suggested reject class: `ERR_ENCRYPTION_NOT_ALLOWED` (wire taxonomy to be finalized during implementation).

### 3) Rekey overlap default

Use a bounded overlap window.  
Default target: 10 minutes (acceptable range: 5-15 minutes).

Semantics:

1. old key: decrypt-only during overlap
2. new key: encrypt+decrypt immediately
3. old key expires fully after overlap

This is a transitional decrypt grace period, not long-term multi-generation key support.

### 4) Rekey mechanism

Rekey should be protocol-native replicated control-plane state, not config-only.

Conceptual event:

1. `epoch`
2. activation boundary (`activateAt` logical time/sequence)
3. overlap boundary (`overlapUntil`)
4. wrapped key material metadata

Initiation can still be operator-gated (owner/role/automation), but the mechanism should be replayable and convergent.

### 5) Encryption policy transitions

Policy modes:

1. `off`
2. `optional`
3. `required`

Handling:

1. `off` => plaintext only; encrypted ops rejected
2. `optional` => mixed mode allowed
3. `required` => encrypted only; plaintext rejected

Transition guidance:

1. `off -> optional` allowed
2. `optional -> required` allowed
3. `required -> optional` maybe (explicitly controlled)
4. `required -> off` strongly discouraged or forbidden

Prefer treating significant policy transitions as room epoch boundaries.

## Future-proofing fields to include in first implementation slice

Even with a single suite in v1, include:

1. `cryptoVersion`
2. `cipherSuite`
3. `keyEpoch`

## Out of scope for this deferred plan

1. multi-authority federation crypto choreography
2. post-quantum suite work
3. enterprise/fips suite matrix
4. full implementation task breakdown and code changes

## Resume criteria (when to un-table)

Begin implementation only after stable-release objectives are complete and release quality is holding:

1. stable packaging and runtime lanes are green
2. core docs/tutorial/onboarding artifacts are published
3. release cadence can absorb protocol/runtime additions safely

## Proposed first implementation slice (for later)

When resumed, start with a narrow vertical:

1. contract freeze + reject taxonomy
2. policy enforcement (`off|optional|required`) in server/host parity lanes
3. one E2EE end-to-end conformance vector

