# Identity Continuity + Rotation v1 Contract

Status: Draft-implemented (Phase C slice 1)
Owner: Host runtime maintainers
Last Updated: 2026-05-22

## 1. Purpose

This contract defines the minimum identity continuity semantics for key rotation without introducing federation or delegated trust.

Goals:
1. Allow a successor signer key during a bounded overlap window.
2. Reject successor admission when overlap has expired.
3. Reject successor admission when predecessor has already been revoked.

## 2. Token Shape (v1)

When present, continuity metadata is carried in `hello.token.continuity`.

```json
{
  "token": {
    "peer_pubkey": "<64-char-hex>",
    "expiry": 1735689600,
    "sig": "<signature-hex>",
    "caps": ["room.admin"],
    "continuity": {
      "predecessor_peer_pubkey": "<64-char-hex>",
      "overlap_not_after": 1735689500,
      "revoked_predecessors": ["<64-char-hex>"]
    }
  }
}
```

Field contract:
1. `predecessor_peer_pubkey` is required when `continuity` is present.
2. `overlap_not_after` is required when `continuity` is present.
3. `revoked_predecessors` is optional and defaults to empty.
4. `predecessor_peer_pubkey` must differ from current `token.peer_pubkey`.

Backward compatibility:
1. If `token.continuity` is absent, v1 continuity checks are not enforced.
2. Existing locked-room token verification behavior remains unchanged.

## 3. Admission Semantics

Given `now_secs` from host clock:
1. If `continuity` is absent, continue with normal token verification.
2. If `predecessor_peer_pubkey` is missing/invalid, reject (`reject.protocol_violation`).
3. If `overlap_not_after` is missing, reject (`reject.protocol_violation`).
4. If predecessor equals current signer key, reject (`reject.protocol_violation`).
5. If predecessor is listed in `revoked_predecessors`, reject (`reject.auth_violation`).
6. If `now_secs > overlap_not_after`, reject (`reject.auth_violation`).
7. Otherwise, allow continuity handoff.

## 4. Conformance Vectors (Phase C)

Identity vectors introduced for v1:
1. `IDENTITY-CONTINUITY-001` allow during overlap window.
2. `IDENTITY-CONTINUITY-002` reject after overlap expiry.
3. `IDENTITY-CONTINUITY-003` reject revoked predecessor key.

These vectors execute in Rust canonical conformance slices in this phase.

## 5. Non-Goals (v1)

1. Federation-wide identity proofs.
2. Cross-authority predecessor attestation.
3. Multi-hop continuity chains beyond predecessor->successor checks.
4. Cryptographic binding of continuity statement payload beyond existing token signature.
