# Integration guide

How to embed NodalMerge sync runtime in your product. One worked example
per deployment shape. All examples wire real, currently-shipping
crates.

> For day-2 operations see
> [operator.md](./operator.md). For the frontend API see
> [sdk.md](./sdk.md). For delegated-storage asset lifecycle cleanup,
> see [delegated-storage-gc.md](./delegated-storage-gc.md).

---

## Shape 1 — Self-host with `DirPersistence`

Smallest possible config. One binary, one data directory, SQLite for
nodes, file-per-blob for bytes. Ideal for single-tenant or small-team
deployments.

```rust
// bin/my-server.rs
use std::sync::Arc;
use nodalmerge_server::{
    keypair,
    room::{self, Rooms},
    store::{DirPersistence, SharedPersistence},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server_key = keypair::load_or_generate();
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open("/var/lib/nodalmerge")?);

    let rooms = Rooms::new(
        server_key,
        persistence,
        /* broadcast_capacity */ 512,
        /* peer_rate_nodes    */ 200,
        /* peer_rate_bytes    */ 4 * 1024 * 1024,
    );

    // ... serve WS via axum / your router of choice ...
    Ok(())
}
```

Or just run the prebuilt binary:

```sh
nodalmerge-server --store /var/lib/nodalmerge
```

---

## Shape 2 — SpeechSlate-shape: Mongo + S3

Nodes in the customer's existing Mongo; blobs in S3 (or R2/MinIO).
Auth is *delegated* — the sync server never holds S3 keys; it asks
the app's API for presigned URLs.

```toml
# Cargo.toml
[dependencies]
nodalmerge-server         = { path = "..." }
nodalmerge-mongo-store    = { path = "..." }
nodalmerge-s3-blobs       = { path = "..." }
tokio = { version = "1", features = ["full"] }
```

```rust
use std::sync::Arc;
use std::time::Duration;
use nodalmerge_server::{keypair, room::Rooms, store::Composite};
use nodalmerge_mongo_store::{MongoNodeStore, MongoNodeStoreConfig};
use nodalmerge_s3_blobs::{S3Auth, S3BlobStore, S3BlobStoreConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server_key = keypair::load_or_generate();

    let nodes = MongoNodeStore::connect(MongoNodeStoreConfig::new(
        std::env::var("MONGO_URI")?,
        "speechslate".to_string(),
    ))?;

    let blobs = S3BlobStore::new(S3BlobStoreConfig {
        bucket: "speechslate-blobs".into(),
        region: "auto".into(),                 // R2
        endpoint: Some("https://<accountid>.r2.cloudflarestorage.com".into()),
        path_prefix: "nodalmerge/".into(),
        presign_get_ttl: Duration::from_secs(3600),
        presign_put_ttl: Duration::from_secs(900),
        direct_upload_threshold: 1 * 1024 * 1024,
        require_https: true,
        auth: S3Auth::delegate(
            "https://api.speechslate.app/blobs/presign",
            Some(format!("Bearer {}", std::env::var("SS_SHARED_SECRET")?)),
        ),
    })?;

    let rooms = Rooms::new(
        server_key,
        Arc::new(Composite::new(nodes, blobs)),
        512,
        200,
        4 * 1024 * 1024,
    );

    // ... serve WS ...
    Ok(())
}
```

**What the app's `/blobs/presign` endpoint must do.** Accept JSON of
the shape `{ op: "get" | "put", room, hash, algorithm, size, ttl_seconds,
content_type, namespace }` (delegate protocol v1 — see
[docs/BLOB_STORAGE_LAYOUT.md](BLOB_STORAGE_LAYOUT.md#7-delegated-presign-protocol-v1))
and respond with `{ "url": "<presigned url>" }`. The caller computes
expiry locally from the `ttl_seconds` it sent — the response carries no
expiry field. HTTP 4xx/5xx is treated as "no URL, fall back to WS".

**Why Delegate mode.** SpeechSlate already owns S3 credentials and
rotates them on a known schedule. The sync server becomes a dumb URL
minter proxied through the app's existing auth surface. Credential
blast radius stays inside the app.

---

## Shape 3 — Postgres + S3

Same blob story; Postgres holds nodes instead of Mongo. Schema is
created on first run via the crate's embedded `sqlx::migrate!()`.

```toml
[dependencies]
nodalmerge-server           = { path = "..." }
nodalmerge-postgres-store   = { path = "..." }
nodalmerge-s3-blobs         = { path = "..." }
```

```rust
use std::sync::Arc;
use nodalmerge_server::{keypair, room::Rooms, store::Composite};
use nodalmerge_postgres_store::{PostgresNodeStore, PostgresNodeStoreConfig};
use nodalmerge_s3_blobs::{S3Auth, S3BlobStore, S3BlobStoreConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server_key = keypair::load_or_generate();

    // connect_and_migrate = connect + MIGRATOR.run in one call.
    let nodes = PostgresNodeStore::connect_and_migrate(
        PostgresNodeStoreConfig::new(std::env::var("PG_URI")?),
    )?;

    let blobs = S3BlobStore::new(S3BlobStoreConfig {
        bucket: std::env::var("S3_BUCKET")?,
        region: std::env::var("S3_REGION")?,
        auth: S3Auth::direct_from_env(),       // IAM role / env vars
        ..Default::default()
    })?;

    let rooms = Rooms::new(
        server_key,
        Arc::new(Composite::new(nodes, blobs)),
        512, 200, 4 * 1024 * 1024,
    );

    // ... serve WS ...
    Ok(())
}
```

**Direct vs. Delegate.** `S3Auth::direct_from_env` reads the AWS
default credential chain — environment variables, shared config,
instance metadata. Right for deployments where the sync server runs
under an IAM role that already has `s3:GetObject` / `s3:PutObject`
on the bucket.

---

## Shape 4 — JWT-bridge for trusted-issuer auth (F5)

Plug an existing identity provider (Clerk, Supabase, Auth0, a
homegrown JWT issuer) in front. The `nodalmerge-jwt-bridge` crate
verifies a JWT and mints a `RoomToken` the sync server accepts. The
bridge is a library; you wrap it in a tiny HTTP service.

See [self-host.md](./self-host.md) for the full ~30-line Axum
example. Key points:

- The bridge holds the per-room Ed25519 signing key.
- The JWT claims must include `{ room, pubkey, exp, caps }` at
  minimum; the bridge supports HS256, RS256, and ES256 signing via
  `jsonwebtoken 9`.
- `BridgeConfig` takes optional `allowed_issuers` and
  `allowed_audiences` allow-lists; passing them makes the bridge
  reject unknown `iss` / `aud`.
- The sync server itself does not change — the bridge's output is a
  `RoomToken` just like any other.

```rust
use nodalmerge_jwt_bridge::{mint_room_token, BridgeConfig, JwtVerifier};

let cfg = BridgeConfig {
    verifier:          JwtVerifier::hs256(std::env::var("JWT_SECRET")?.as_bytes()),
    room_key:          room_signing_key,
    allowed_issuers:   vec!["https://accounts.speechslate.app".into()],
    allowed_audiences: vec!["nodalmerge".into()],
};

// In your HTTP handler:
let token = mint_room_token(&cfg, &incoming_jwt)?;
```

---

## Frontend — `createDoc`

The SDK is transport- and backend-agnostic. The same frontend code
works against every shape above.

```js
import { createDoc } from './sdk.js';

const doc = await createDoc({
  serverUrl: 'wss://sync.speechslate.app',
  room:      'boards/abc-123',
  roomSeed,                           // for locked rooms; 32-byte Uint8Array
  onMetric: (ev) => metrics.emit(ev), // G8 hook
  onConflict: (ev) => toast(ev),      // G9 hook
});

const buttons = doc.list('boards/abc-123/buttons');   // F8 List CRDT
const items   = doc.map('buttons');

const id = buttons.push({ label: 'Hello' });
items.set(id, { label: 'Hola' });
```

See [sdk.md](./sdk.md) for the full API.

---

## What to wire into your metrics pipeline

The server exports Prometheus metrics via `--metrics-addr` (see
[operator.md](./operator.md)). The SDK emits point events via
`createDoc({ onMetric })` and conflicts via
`createDoc({ onConflict })`. Both surfaces are zero-cost when the
hooks are unset.

Correlate server-side `nodalmerge_merge_batch_seconds` with
client-side `pack_applied` + `op_apply_latency` events (G8) to
diagnose latency regressions end-to-end.
