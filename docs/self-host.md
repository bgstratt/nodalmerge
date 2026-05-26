# Self-host NodalMerge in 5 minutes

This doc shows the shortest path from zero to a production-ish NodalMerge
deployment with your existing auth provider (Clerk, Supabase, Auth0, or a
homegrown JWT issuer) in front.

Compatibility note: legacy `activesync-*` crate/package IDs and selected
logger/config identifiers remain valid during the migration window.

## 1. Build & run the server

```sh
git clone <your-fork-or-mirror> nodalmerge
cd nodalmerge
docker build -t nodalmerge-server .
docker run --rm -p 7878:7878 \
    -v "$PWD/data:/data" \
    -e RUST_LOG=info \
    nodalmerge-server
```

The container listens on `0.0.0.0:7878` and persists rooms + blobs to
`/data` (see [deployment.md](./deployment.md) for the on-disk layout,
backups, and operational notes).

## 2. Decide who signs room tokens

NodalMerge rooms are gated by a per-room Ed25519 key. Capabilities for a
connecting peer are encoded in a short-lived `RoomToken` signed by that
key. You have two options for where the signing happens:

| Mode       | Who holds the room key           | Use when |
| ---------- | -------------------------------- | -------- |
| Self-mint  | Each client (bundled on device)  | Peer-to-peer / trust-on-install |
| Bridge-mint| Your bridge service              | You already have user auth and want server-issued capabilities |

For the bridge flow — which is what this doc covers — your auth provider
signs a **JWT** describing a peer's room + capabilities, and the
[`activesync-jwt-bridge`](../jwt-bridge) crate converts that into a
`RoomToken` the NodalMerge server will accept.

## 3. Stand up the JWT bridge

The bridge is a library, so you wrap it in a tiny HTTP service (10-ish
lines of Axum). Minimum viable:

```rust
use activesync_jwt_bridge::{mint_room_token, BridgeConfig, JwtVerifier};
use axum::{extract::State, routing::post, Json, Router};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
struct Req { jwt: String }

#[derive(Serialize)]
struct Resp {
    peer_pubkey_hex: String,
    expiry_secs:     u64,
    capabilities:    Vec<String>,
    sig_hex:         String,
}

async fn mint(State(cfg): State<Arc<BridgeConfig>>, Json(r): Json<Req>)
    -> Result<Json<Resp>, (axum::http::StatusCode, String)>
{
    let tok = mint_room_token(&cfg, &r.jwt)
        .map_err(|e| (axum::http::StatusCode::UNAUTHORIZED, e.to_string()))?;
    Ok(Json(Resp {
        peer_pubkey_hex: hex(&tok.peer_pubkey),
        expiry_secs:     tok.expiry_secs,
        capabilities:    tok.capabilities,
        sig_hex:         hex(&tok.signature),
    }))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

#[tokio::main]
async fn main() {
    let cfg = Arc::new(BridgeConfig {
        verifier:          JwtVerifier::hs256(b"replace-me-with-your-secret"),
        room_key:          SigningKey::from_bytes(&load_room_key()),
        allowed_issuers:   vec!["https://your-auth.example.com".into()],
        allowed_audiences: vec!["nodalmerge".into()],
    });
    let app = Router::new().route("/mint", post(mint)).with_state(cfg);
    axum::serve(tokio::net::TcpListener::bind("0.0.0.0:8088").await.unwrap(), app)
        .await.unwrap();
}

fn load_room_key() -> [u8; 32] { todo!("read from KMS / file / env") }
```

For RS256 / ES256 (Clerk, Auth0, Supabase), swap
`JwtVerifier::hs256(secret)` for `JwtVerifier::rs256_from_pem(pem)` or
`JwtVerifier::es256_from_pem(pem)`.

## 4. Claim shape your issuer must produce

```json
{
    "iss":    "https://your-auth.example.com",
    "aud":    "nodalmerge",
    "exp":    1999999999,
    "room":   "game-42",
    "pubkey": "<64 hex chars of the peer's Ed25519 public key>",
    "caps":   ["read:world/**", "write:intent/**"]
}
```

`exp` doubles as the JWT expiry *and* the `RoomToken.expiry_secs` — one
clock, one truth. `caps` is optional (omit or `[]` for an unscoped token).

## 5. Wire the client

The client generates its Ed25519 keypair on first launch, POSTs
`{ jwt }` to `/mint`, and feeds the response fields into the NodalMerge
SDK's `hello` message (`tokenCaps` / `roomSeed` / signature fields — see
the SDK README). From this point the NodalMerge server treats the
connection identically to a self-minted token; it has no knowledge of
JWTs.

## 6. Operate

- Rotate the room signing key by redeploying the bridge with a new key
    *and* making the server start a new room id (roll forward: old rooms
    still verify with the old key until their tokens expire).
- Rotate the JWT verifier by deploying a new `JwtVerifier`; no server
    change needed.
- Back up `/data` (see [deployment.md](./deployment.md)) — that's the
    entire durable state.
- Tune logs via `RUST_LOG` (migration window still uses targets like
    `RUST_LOG=activesync_server=debug,info`).

That's it. Five minutes of YAML/Terraform and you have a self-hosted
NodalMerge deployment that plugs into whatever auth you already own.
