# nodalmerge-jwt-bridge

Trusted-issuer JWT -> NodalMerge `RoomToken` minting bridge.

This crate lets you keep identity in your existing auth system (Clerk, Auth0, Supabase, custom issuer, etc.) while still using native NodalMerge room authorization.

## What It Does

- Verifies an incoming JWT from a trusted issuer.
- Extracts NodalMerge-specific claims (`room`, `pubkey`, `exp`, optional `caps`).
- Mints an NodalMerge `RoomToken` signed with the room's Ed25519 signing key.
- Returns a token the client can send in the normal NodalMerge `hello` handshake.

Result: the NodalMerge server does not need to understand your identity provider. It only verifies `RoomToken` as usual.

## Why This Exists

Without this bridge, clients usually need to sign `RoomToken` locally (or your server needs custom auth logic in the NodalMerge layer).

With this bridge:

- JWT answers: "Who is this user/session?"
- `RoomToken` answers: "What can this peer do in this room?"

That separation keeps auth architecture clean and easier to evolve.

## How It Works

1. Your auth layer issues a JWT with claims this crate expects.
2. Your bridge endpoint receives that JWT.
3. `mint_room_token` verifies JWT signature and standard JWT checks.
4. It signs a `RoomToken` using your room Ed25519 signing key.
5. Client includes the minted token fields in NodalMerge `hello`.

## Claims Contract

Required claims:

- `room` (string): NodalMerge room id.
- `pubkey` (string): peer Ed25519 public key as 64 hex chars (32 bytes).
- `exp` (number): Unix epoch seconds.

Optional claims:

- `caps` (string array): capability strings, for example `read:world/**`.
- Standard JWT fields such as `iss`, `aud`, `nbf` are supported and validated by `jsonwebtoken` based on configured validation.

Notes:

- `exp` is used both to validate the JWT and to set `RoomToken.expiry_secs`.
- Unknown extra claims are ignored.

## Supported Verifier Modes

`JwtVerifier` supports:

- `hs256(secret)`: shared secret verification.
- `rs256_from_pem(public_key_pem)`: RSA public key verification.
- `es256_from_pem(public_key_pem)`: ECDSA P-256 public key verification.

Use `allowed_issuers` and `allowed_audiences` in `BridgeConfig` to enforce `iss`/`aud` allow-lists.

## Basic Usage

```rust
use nodalmerge_jwt_bridge::{mint_room_token, BridgeConfig, JwtVerifier};
use ed25519_dalek::SigningKey;

// JWT verification side (from your auth issuer)
let verifier = JwtVerifier::hs256(b"super-secret-issuer-key");

// Room signing key (the key that authorizes RoomToken for the room)
let room_key = SigningKey::from_bytes(&[0x42u8; 32]);

let cfg = BridgeConfig {
    verifier,
    room_key,
    allowed_issuers: vec!["https://auth.example.com".into()],
    allowed_audiences: vec!["nodalmerge-clients".into()],
};

let jwt = "<jwt-from-client>";
let token = mint_room_token(&cfg, jwt)?;

// Return token fields to client; client uses them in NodalMerge hello.
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Typical Deployment Pattern

Run this crate behind a tiny HTTP endpoint near your `nodalmerge-server`:

- `POST /bridge/mint-room-token`
- Input: bearer JWT (or JSON field containing JWT)
- Output: serialized `RoomToken` fields for client handshake

That endpoint should:

- Authenticate transport (TLS, proper origin policies, optional service auth).
- Validate JWT using a configured verifier.
- Mint and return `RoomToken` only after successful validation.

## Error Model

`mint_room_token` returns `BridgeError`:

- `BadJwt`: signature/claims validation failed.
- `BadClaim`: claim shape is invalid (for example malformed `pubkey`).
- `RoomToken`: token signing/validation layer error.

This makes it straightforward to map to HTTP responses (for example `401/403/400`).

## Security Notes

- Keep your room signing key private and server-side only.
- Restrict accepted algorithms to what you explicitly configure.
- Prefer strict `iss` and `aud` allow-lists in production.
- Keep JWT lifetimes short; token expiry directly controls NodalMerge access window.
- Never trust client-provided capabilities unless they originate from verified JWT claims you control.

## Testing

The crate includes unit tests for:

- happy-path minting and verification
- expired JWT rejection
- wrong secret rejection
- malformed/missing claim handling
- issuer allow-list enforcement
- default empty capabilities

Run tests from repo root:

```bash
cargo test -p nodalmerge-jwt-bridge
```
