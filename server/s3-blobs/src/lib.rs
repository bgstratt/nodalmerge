//! F6 — S3-compatible blob backend for `nodalmerge-server`.
//!
//! Implements [`BlobPersistence`] against any S3 API (AWS, Cloudflare R2,
//! MinIO, Google Cloud Storage's S3-compat layer, Azure via S3 gateway, …).
//! The point of this crate is *direct blob I/O*: instead of paying to ship
//! every megabyte of audio over the WebSocket, the sync server hands the
//! client a presigned URL and lets the SDK PUT/GET against object storage
//! directly.
//!
//! ## Auth modes
//!
//! Two auth strategies, selected via [`S3Auth`]:
//!
//! * **Direct**: the sync server holds IAM credentials and mints presigned
//!   URLs locally. Simplest to operate. Best when the sync server is
//!   trusted with bucket access (typical self-host).
//!
//! * **Delegate**: the sync server POSTs to the application's own API to
//!   ask for a presigned URL. The sync server never sees S3 keys.
//!   Designed for the SpeechSlate / multi-tenant case where each app has
//!   its own bucket and rotates its own keys.
//!
//! ## Composition
//!
//! `S3BlobStore` implements only `BlobPersistence`; pair it with any
//! `NodePersistence` via [`nodalmerge_server::store::Composite`]:
//!
//! ```ignore
//! use nodalmerge_server::store::{Composite, DirPersistence};
//! use nodalmerge_s3_blobs::{S3BlobStore, S3BlobStoreConfig, S3Auth};
//! use std::sync::Arc;
//!
//! let nodes = DirPersistence::open("/var/lib/nodalmerge")?;
//! let blobs = S3BlobStore::new(S3BlobStoreConfig {
//!     bucket: "my-app-blobs".into(),
//!     region: "us-east-1".into(),
//!     auth: S3Auth::direct_from_env(),
//!     ..Default::default()
//! })?;
//! let rooms = nodalmerge_server::Rooms::with_persistence(
//!     Arc::new(Composite::new(nodes, blobs))
//! );
//! ```

use std::sync::Arc;
use std::time::Duration;

use nodalmerge_core::Hash;
use nodalmerge_server::store::{
    parse_blob_entry_name, BlobPersistence, HydrateError, PersistBlobError, PresignedUrl,
};
use bytes::Bytes;
use futures_util::StreamExt;
use object_store::{
    aws::{AmazonS3, AmazonS3Builder},
    path::Path as ObjectPath,
    signer::Signer,
    ObjectStore,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

// ─── Sync→async bridge (slice 6.1, finding #11) ─────────────────────────────
//
// `BlobPersistence` is a sync trait, but every backend op here is async and
// the caller may already be *on* a tokio worker — so we can neither
// `block_on` in place (panics) nor start a runtime on the current thread.
// Pre-6.1 each site spawned a fresh OS thread with a fresh multi-thread
// `Runtime` and blocked on a timeout-less `mpsc::recv()`: nine
// runtime-constructions per nine ops, and a hung bucket or delegate endpoint
// pinned the calling worker forever.
//
// Now there is exactly one bridge: a process-wide runtime, lazily built on
// first use and never dropped. Process-wide (`OnceLock`) rather than owned
// by `S3BlobStore` deliberately — dropping a `Runtime` from inside an async
// context panics, and `S3BlobStore` is built/dropped freely (server-s3
// builds a second instance for `S3BlobObjectStore`; tests build dozens), so
// an owned runtime would turn every drop site into a panic hazard. A static
// never drops, which sidesteps the question entirely.

static BRIDGE_RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

/// How many times [`BRIDGE_RUNTIME`] has ever been constructed. `OnceLock`
/// already guarantees ≤ 1; this makes the guarantee *observable* so
/// `bridge_reuses_one_shared_runtime_across_ops` can pin "N ops → 1 runtime"
/// against regression back to per-op construction.
static BRIDGE_RUNTIMES_BUILT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Margin the blocking receive waits *beyond* the op's own timeout, so the
/// inner, better-labelled timeout (reqwest / `tokio::time::timeout`) always
/// fires first and the recv timeout is purely a backstop against a lost
/// bridge task.
const BRIDGE_RECV_MARGIN: Duration = Duration::from_secs(15);

fn bridge_handle() -> &'static tokio::runtime::Handle {
    BRIDGE_RUNTIME
        .get_or_init(|| {
            BRIDGE_RUNTIMES_BUILT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::runtime::Builder::new_multi_thread()
                // S3/HTTP I/O only — two workers is plenty, and keeps this
                // pool from competing with the server's own runtime.
                .worker_threads(2)
                .thread_name("nm-s3-bridge")
                .enable_all()
                .build()
                .expect("build shared s3 bridge runtime")
        })
        .handle()
}

/// The one sync→async bridge every op in this crate goes through (name per
/// blob-cas-remediation.md 7.3, which absorbs this helper).
///
/// Spawns `fut` onto the shared runtime under a `tokio::time::timeout` of
/// `inner_timeout`, then blocks on `recv_timeout(inner_timeout + margin)`.
/// Both bounds classify as [`S3BlobError::Timeout`] — a *backend* error.
/// Callers must never translate it into an absence/`Missing` answer: GC
/// distinguishes "blob absent" from "backend failed" (slice 2.2), and a hung
/// bucket misread as missing data is the exact failure this crate must not
/// produce.
fn block_on_shared_runtime<T: Send + 'static>(
    op: &'static str,
    inner_timeout: Duration,
    fut: impl std::future::Future<Output = T> + Send + 'static,
) -> Result<T, S3BlobError> {
    let (tx, rx) = std::sync::mpsc::channel();
    bridge_handle().spawn(async move {
        let _ = tx.send(tokio::time::timeout(inner_timeout, fut).await);
    });
    match rx.recv_timeout(inner_timeout + BRIDGE_RECV_MARGIN) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(_elapsed)) => Err(S3BlobError::Timeout { op, waited: inner_timeout }),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(S3BlobError::Timeout {
            op,
            waited: inner_timeout + BRIDGE_RECV_MARGIN,
        }),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(S3BlobError::Bridge {
            op,
            detail: "bridge task dropped its result channel before completing".to_string(),
        }),
    }
}

// ─── GC tombstones ──────────────────────────────────────────────────────────

/// Key infix for the GC tombstone objects
/// [`S3BlobStore::blob_gc_sweep`] writes: full key is
/// `<path_prefix>.tombstones/blake3/<hex>.<unix_millis>`. Mirrors
/// `DirPersistence`'s `blobs/.tombstones/blake3/` directory. A sibling of
/// `<path_prefix>blake3/`, so the blob listing never sees tombstones.
const TOMBSTONE_INFIX: &str = ".tombstones/blake3";

/// Slice 6.3 — how many sweep PUT/DELETE actions run concurrently inside
/// [`S3BlobStore::blob_gc_sweep`]'s single bridge call. 16 keeps the sweep
/// an order of magnitude faster than the old one-await-per-object shape
/// while staying far below S3's per-prefix request limits (≥3500 write
/// req/s) and below anything that would starve the 2-worker bridge runtime;
/// per-object step ordering (tombstone-before-delete) is preserved inside
/// each buffered action, so concurrency never touches the two-phase
/// protocol.
const SWEEP_IO_CONCURRENCY: usize = 16;

fn now_unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Parse a tombstone object's filename (`<64-hex>.<unix_millis>`) into its
/// bare hash hex and the millisecond stamp recorded in the key. `None` for
/// anything else — a foreign object under the tombstone prefix is left
/// alone, same rule as `parse_blob_entry_name`'s (BLOB_STORAGE_LAYOUT.md
/// §3).
fn parse_tombstone_name(filename: &str) -> Option<(String, u128)> {
    let (hex, stamp) = filename.split_once('.')?;
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        return None;
    }
    let ms: u128 = stamp.parse().ok()?;
    Some((hex.to_string(), ms))
}

// ─── Public types ───────────────────────────────────────────────────────────

/// All knobs for an `S3BlobStore`.
#[derive(Debug, Clone)]
pub struct S3BlobStoreConfig {
    /// Bucket name.
    pub bucket: String,
    /// Region (e.g. `"us-east-1"`, `"auto"` for R2).
    pub region: String,
    /// Optional custom endpoint URL. Set for non-AWS providers (R2 / MinIO /
    /// GCS-S3). `None` uses AWS S3.
    pub endpoint: Option<String>,
    /// Prefix prepended to every key. Defaults to `"blobs/"`.
    pub path_prefix: String,
    /// How long presigned **GET** URLs stay valid. Default 1 h.
    pub presign_get_ttl: Duration,
    /// How long presigned **PUT** URLs stay valid. Default 15 min.
    pub presign_put_ttl: Duration,
    /// Skip presigning for blobs *smaller* than this — the WS round-trip is
    /// cheaper than mint+redirect. Default 1 MiB.
    pub direct_upload_threshold: u64,
    /// `true` ⇒ require HTTPS for the endpoint. Default `true`. Test
    /// helpers (MinIO over HTTP) can flip this off.
    pub require_https: bool,
    /// Upper bound on a single backend operation: one delegate presign
    /// roundtrip, one HEAD/GET/PUT/DELETE. Applied three deep — as the
    /// per-request timeout on both HTTP clients *and* as the
    /// `tokio::time::timeout` the bridge wraps each op future in — so no
    /// retry loop inside `object_store` can stretch an op past it. A
    /// timeout is always a **backend** error, never "missing" (slice 6.1).
    /// Default 30 s.
    pub op_timeout: Duration,
    /// TCP connect timeout for both HTTP clients. Default 10 s.
    pub connect_timeout: Duration,
    /// Upper bound on one whole [`BlobPersistence::blob_gc_sweep`] pass —
    /// a batch of LISTs/PUTs/DELETEs, so `op_timeout` would be far too
    /// tight; each request inside it is still individually bounded by
    /// `op_timeout`. Aborting mid-sweep is safe: identical to a crash
    /// mid-sweep, which the two-phase tombstone protocol already tolerates
    /// (slice 1.3). Default 15 min.
    pub sweep_timeout: Duration,
    /// Auth strategy.
    pub auth: S3Auth,
}

impl Default for S3BlobStoreConfig {
    fn default() -> Self {
        Self {
            bucket: String::new(),
            region: "us-east-1".into(),
            endpoint: None,
            path_prefix: "blobs/".into(),
            presign_get_ttl: Duration::from_secs(60 * 60),
            presign_put_ttl: Duration::from_secs(15 * 60),
            direct_upload_threshold: 1 * 1024 * 1024,
            require_https: true,
            op_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            sweep_timeout: Duration::from_secs(15 * 60),
            auth: S3Auth::Direct {
                access_key_id: None,
                secret_access_key: None,
                session_token: None,
            },
        }
    }
}

/// How `S3BlobStore` obtains URLs.
#[derive(Debug, Clone)]
pub enum S3Auth {
    /// Sync server holds IAM creds and mints presigned URLs locally.
    ///
    /// Any field set to `None` falls through to the default AWS credential
    /// chain (env / instance profile / config file). The intended way to
    /// pick up env-var creds is to leave all three `None`.
    Direct {
        access_key_id: Option<String>,
        secret_access_key: Option<String>,
        session_token: Option<String>,
    },

    /// Sync server POSTs to the app's own API to ask for URLs. The sync
    /// server never sees S3 keys.
    ///
    /// The endpoint must accept the delegate presign protocol v1
    /// (see `docs/BLOB_STORAGE_LAYOUT.md` §7):
    /// ```json
    /// { "op": "get" | "put",
    ///   "room": "...",
    ///   "hash": "...",
    ///   "algorithm": "blake3",
    ///   "size": 12345,
    ///   "ttl_seconds": 3600,
    ///   "content_type": "..." }
    /// ```
    /// and respond with
    /// ```json
    /// { "url": "https://..." }
    /// ```
    /// or HTTP 4xx/5xx (treated as "no URL — use WS fallback"). `room` is
    /// metadata only — it MUST NOT influence the app's key derivation
    /// (`<app-prefix>blake3/<hash>`).
    Delegate {
        /// Full URL of the app's presign endpoint.
        presign_endpoint: String,
        /// Optional `Authorization` header value (e.g. shared secret).
        auth_header: Option<String>,
    },
}

impl S3Auth {
    /// Pick credentials from the AWS default chain.
    pub fn direct_from_env() -> Self {
        S3Auth::Direct {
            access_key_id: None,
            secret_access_key: None,
            session_token: None,
        }
    }

    /// Hard-coded creds. Avoid in production; useful for tests.
    pub fn direct_explicit(access_key_id: impl Into<String>, secret: impl Into<String>) -> Self {
        S3Auth::Direct {
            access_key_id: Some(access_key_id.into()),
            secret_access_key: Some(secret.into()),
            session_token: None,
        }
    }

    /// Configure delegate mode.
    pub fn delegate(presign_endpoint: impl Into<String>, auth_header: Option<String>) -> Self {
        S3Auth::Delegate {
            presign_endpoint: presign_endpoint.into(),
            auth_header,
        }
    }
}

#[derive(Debug, Error)]
pub enum S3BlobError {
    #[error("config: {0}")]
    Config(String),
    #[error("object_store: {0}")]
    ObjectStore(#[from] object_store::Error),
    #[error("delegate http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),
    /// The op's bounded wait expired — backend unresponsive. Explicitly
    /// **not** evidence the object is absent; callers whose signature can't
    /// carry an error must pick the conservative value, never "missing".
    #[error("s3 op {op:?} timed out after {waited:?} — backend unresponsive, not evidence of a missing object")]
    Timeout { op: &'static str, waited: Duration },
    /// The bridge task vanished without delivering a result (panic inside
    /// the spawned future). Backend-class, like [`Self::Timeout`].
    #[error("s3 bridge failed for op {op:?}: {detail}")]
    Bridge { op: &'static str, detail: String },
}

// ─── The store ──────────────────────────────────────────────────────────────

/// S3-compatible `BlobPersistence` backend.
pub struct S3BlobStore {
    cfg: S3BlobStoreConfig,
    /// Direct mode only — used to mint presigned URLs and run uploads.
    /// `None` for Delegate mode.
    s3: Option<Arc<AmazonS3>>,
    /// Delegate mode only — async HTTP client. Built once in [`Self::new`]
    /// (connection pool reused across calls) with explicit request/connect
    /// timeouts; `clone()` at call sites is a cheap `Arc` bump sharing the
    /// same pool.
    http: reqwest::Client,
    // No runtime here — all ops go through the process-wide bridge
    // (`block_on_shared_runtime`); see the rationale at its definition.
}

impl std::fmt::Debug for S3BlobStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3BlobStore")
            .field("bucket", &self.cfg.bucket)
            .field("region", &self.cfg.region)
            .field("endpoint", &self.cfg.endpoint)
            .field("auth", &match &self.cfg.auth {
                S3Auth::Direct { .. } => "Direct",
                S3Auth::Delegate { .. } => "Delegate",
            })
            .finish()
    }
}

impl S3BlobStore {
    /// Build a new store from config.
    pub fn new(cfg: S3BlobStoreConfig) -> Result<Self, S3BlobError> {
        if cfg.bucket.is_empty() {
            return Err(S3BlobError::Config("bucket is required".into()));
        }
        if let Some(ep) = &cfg.endpoint {
            if cfg.require_https && !ep.starts_with("https://") {
                return Err(S3BlobError::Config(format!(
                    "endpoint {ep:?} is not https; set require_https=false to allow"
                )));
            }
        }

        // Direct mode needs an `AmazonS3` for both presigning and the
        // (rarely-used) `persist_blob` fallback. Delegate mode skips it.
        let s3 = match &cfg.auth {
            S3Auth::Direct {
                access_key_id,
                secret_access_key,
                session_token,
            } => {
                // Per-request bounds at the client layer, in addition to the
                // bridge's per-op bound: inside `blob_gc_sweep` (many
                // requests under one bridge call) this is the only thing
                // keeping a single hung request from eating the whole
                // sweep budget. Set BEFORE the endpoint block —
                // `with_allow_http` also writes into the builder's client
                // options, and a later `with_client_options` would clobber
                // it.
                let mut b = AmazonS3Builder::new()
                    .with_bucket_name(&cfg.bucket)
                    .with_region(&cfg.region)
                    .with_client_options(
                        object_store::ClientOptions::new()
                            .with_timeout(cfg.op_timeout)
                            .with_connect_timeout(cfg.connect_timeout),
                    )
                    // Slice 6.3 (closing 6.1's filed follow-up): object_store's
                    // default RetryConfig is max_retries=10 / retry_timeout=180s
                    // *per request*. Single ops never felt it — the bridge
                    // aborts them at `op_timeout` regardless — but inside
                    // `blob_gc_sweep` (one bridge call bounded only by
                    // `sweep_timeout`) each internal request could legally
                    // retry toward that 180s, multiplying a flaky bucket's
                    // sweep toward the 15-minute ceiling. Bound the retry
                    // budget near `op_timeout`, with a couple of quick
                    // retries for genuinely transient blips.
                    //
                    // The +5s margin is load-bearing, not slack: the client
                    // layer must never give up *before* the bridge's own
                    // `op_timeout` bound, or a hung bucket's single op
                    // surfaces through the `Ok(Err(_))` arm as a generic
                    // backend error instead of the bridge's `Timeout` —
                    // reclassifying persist_blob's 503-Unavailable ("write
                    // unconfirmed, retry") as 500-Backend ("origin broken").
                    // `blob_put_durability.rs` pins exactly that against a
                    // stalled bucket. So: single ops still cut off by the
                    // bridge at `op_timeout` (classification unchanged),
                    // while sweep-internal requests — which have no bridge
                    // bound of their own — are capped at `op_timeout + 5s`
                    // instead of 3 minutes.
                    .with_retry(object_store::RetryConfig {
                        max_retries: 2,
                        retry_timeout: cfg.op_timeout + Duration::from_secs(5),
                        ..Default::default()
                    });
                if let Some(ep) = &cfg.endpoint {
                    b = b.with_endpoint(ep);
                    if !cfg.require_https {
                        b = b.with_allow_http(true);
                    }
                }
                if let Some(k) = access_key_id {
                    b = b.with_access_key_id(k);
                }
                if let Some(k) = secret_access_key {
                    b = b.with_secret_access_key(k);
                }
                if let Some(t) = session_token {
                    b = b.with_token(t);
                }
                Some(Arc::new(b.build().map_err(S3BlobError::ObjectStore)?))
            }
            S3Auth::Delegate { .. } => None,
        };

        // One client for the store's lifetime, with explicit bounds — a
        // hung delegate endpoint must fail the op, never pin the caller
        // (finding #11). `reqwest::Client::new()` has NO request timeout.
        let http = reqwest::Client::builder()
            .timeout(cfg.op_timeout)
            .connect_timeout(cfg.connect_timeout)
            .build()
            .map_err(S3BlobError::Http)?;

        Ok(Self { cfg, s3, http })
    }

    /// Canonical relative layout: `<path_prefix>blake3/<hex>` — no room
    /// segment, no sharding. See `docs/BLOB_STORAGE_LAYOUT.md`.
    fn key_for(&self, hash: &Hash) -> String {
        format!("{}blake3/{}", self.cfg.path_prefix, hash.to_hex())
    }

    /// Presign a GET via object_store (Direct mode).
    fn direct_presign_get(&self, hash: &Hash) -> Result<String, S3BlobError> {
        let s3 = self.s3.as_ref().expect("direct_presign_get without s3 client");
        let path = ObjectPath::from(self.key_for(hash));
        let ttl = self.cfg.presign_get_ttl;
        let s3 = s3.clone();
        let url: Url = block_on_shared_runtime("presign_get", self.cfg.op_timeout, async move {
            s3.signed_url(reqwest::Method::GET, &path, ttl).await
        })?
        .map_err(S3BlobError::ObjectStore)?;
        Ok(url.to_string())
    }

    /// Presign a PUT via object_store (Direct mode).
    fn direct_presign_put(&self, hash: &Hash) -> Result<String, S3BlobError> {
        let s3 = self.s3.as_ref().expect("direct_presign_put without s3 client");
        let path = ObjectPath::from(self.key_for(hash));
        let ttl = self.cfg.presign_put_ttl;
        let s3 = s3.clone();
        let url: Url = block_on_shared_runtime("presign_put", self.cfg.op_timeout, async move {
            s3.signed_url(reqwest::Method::PUT, &path, ttl).await
        })?
        .map_err(S3BlobError::ObjectStore)?;
        Ok(url.to_string())
    }

    /// HEAD an object — used by `verify_uploaded` (Direct mode only).
    fn direct_head(&self, hash: &Hash) -> Result<bool, S3BlobError> {
        let s3 = self.s3.as_ref().expect("direct_head without s3 client");
        let path = ObjectPath::from(self.key_for(hash));
        let s3 = s3.clone();
        // A timeout propagates as `Err` (via `?`), never as `Ok(false)` —
        // "the backend didn't answer" and "the object is absent" must stay
        // distinguishable for every caller of this existence probe.
        let res = block_on_shared_runtime("head", self.cfg.op_timeout, async move {
            s3.head(&path).await
        })?;
        match res {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(S3BlobError::ObjectStore(e)),
        }
    }

    /// S5.3 — HEAD an arbitrary bucket key (not derived from a `Hash`),
    /// Direct mode only. Generalizes `direct_head` for
    /// [`S3BlobObjectStore`], which the GC coordinator's hard-sweep drives
    /// off inventory rows keyed by object key, not by hash.
    pub fn head_key(&self, key: &str) -> Result<bool, S3BlobError> {
        let Some(s3) = self.s3.clone() else {
            return Err(S3BlobError::Config(
                "head_key requires Direct-mode credentials (Delegate mode has no bucket access)".to_string(),
            ));
        };
        let path = ObjectPath::from(key.to_string());
        // GC-liveness-relevant: a timeout here must reach the coordinator
        // as `Err` (→ `GcError::Backend`), never as `Ok(false)` — "I don't
        // know" must never become "not live".
        let res = block_on_shared_runtime("head_key", self.cfg.op_timeout, async move {
            s3.head(&path).await
        })?;
        match res {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(S3BlobError::ObjectStore(e)),
        }
    }

    /// S5.3 — DELETE an arbitrary bucket key. See [`Self::head_key`]. A
    /// missing object is treated as success (already gone is the goal
    /// state, not an error) — mirrors `object_store::ObjectStore::delete`'s
    /// own idempotent-on-missing behavior for most backends, made explicit
    /// here rather than relying on it.
    pub fn delete_key(&self, key: &str) -> Result<(), S3BlobError> {
        let Some(s3) = self.s3.clone() else {
            return Err(S3BlobError::Config(
                "delete_key requires Direct-mode credentials (Delegate mode has no bucket access)".to_string(),
            ));
        };
        let path = ObjectPath::from(key.to_string());
        // NotFound → success (already gone is the goal state), but a
        // timeout stays an error: "I couldn't confirm the delete" and "it
        // was already gone" are different facts to the GC run ledger.
        let res = block_on_shared_runtime("delete_key", self.cfg.op_timeout, async move {
            s3.delete(&path).await
        })?;
        match res {
            Ok(()) => Ok(()),
            Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(S3BlobError::ObjectStore(e)),
        }
    }

    /// Delegate-mode HTTP roundtrip.
    fn delegate_request(
        &self,
        op: &'static str,
        room_id: &str,
        hash: &Hash,
        size: Option<u64>,
        ttl: Duration,
        content_type: Option<String>,
    ) -> Result<Option<String>, S3BlobError> {
        let (endpoint, auth_header) = match &self.cfg.auth {
            S3Auth::Delegate { presign_endpoint, auth_header } => {
                (presign_endpoint.clone(), auth_header.clone())
            }
            _ => return Ok(None),
        };
        // Delegate presign protocol v1 — see docs/BLOB_STORAGE_LAYOUT.md §7.
        // `room` is metadata only; it must never influence the app's key
        // derivation.
        #[derive(Serialize)]
        struct Req {
            op: &'static str,
            room: String,
            hash: String,
            size: Option<u64>,
            ttl_seconds: u64,
            algorithm: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            content_type: Option<String>,
        }
        #[derive(Deserialize)]
        struct Resp {
            url: String,
        }
        let body = Req {
            op,
            room: room_id.to_string(),
            hash: hash.to_hex(),
            size,
            ttl_seconds: ttl.as_secs(),
            algorithm: "blake3".to_string(),
            content_type,
        };
        tracing::debug!(%op, room = %room_id, hash = %hash.to_hex(), size = ?size, %body.algorithm, endpoint = %endpoint, "delegate_request: sending presign request to app");
        let http = self.http.clone();
        // The client's own request timeout (set in `new`) is the inner
        // bound and carries the better error; the bridge bound backstops it.
        // Both are `Err`, never a `None` "declined" — a wedged endpoint and
        // an endpoint that said no are different operator problems.
        block_on_shared_runtime("delegate_presign", self.cfg.op_timeout, async move {
            let mut req = http.post(&endpoint).json(&body);
            if let Some(h) = auth_header {
                req = req.header("Authorization", h);
            }
            let resp: reqwest::Response = req.send().await?;
            if !resp.status().is_success() {
                tracing::warn!(status = %resp.status(), %op, "delegate presign declined");
                return Ok(None);
            }
            let parsed: Resp = resp.json().await?;
            Ok::<Option<String>, S3BlobError>(Some(parsed.url))
        })?
    }
}

// ─── BlobPersistence impl ───────────────────────────────────────────────────

impl BlobPersistence for S3BlobStore {
    /// `S3BlobStore` does *not* hydrate blob bytes into the server process —
    /// that would defeat the purpose of offloading them. Always `None`; the
    /// SDK pulls blobs lazily via `resolve_get_url`.
    fn get_blob(&self, _hash: &Hash) -> Option<Vec<u8>> {
        None
    }

    /// Declares the policy above as a fact the trait can act on rather than
    /// only as prose — see [`BlobPersistence::get_blob_hydrates`]. This is
    /// what stops the default [`BlobPersistence::hydrate_blob`] from reporting
    /// every object this backend holds as `Missing`.
    fn get_blob_hydrates(&self) -> bool {
        false
    }

    /// blob-cas-remediation.md slice 2.2 (finding #10) — the resolution path
    /// for objects the **server itself** must parse (today: tree objects, via
    /// [`nodalmerge_server::tree_walk::walk_tree`]).
    ///
    /// **This does not contradict `get_blob` above.** That contract keeps
    /// large *file* payloads out of the server process — the whole reason
    /// this crate exists. `hydrate_blob`'s only caller fetches tree objects:
    /// a few hundred bytes of JSON that the server cannot compute a GC live
    /// set without reading (v1 entries and v2 `"f"` entries are terminal and
    /// are never fetched; only `"d"` entries are). Reading those is the
    /// server reading its own index. File bytes still belong on
    /// `resolve_get_url`, and `get_blob` still returns `None` for everything,
    /// forever — asserted directly in `minio_round_trip.rs`'s
    /// `s3_direct_tree_walk_resolves_tree_objects_from_the_real_bucket`.
    ///
    /// * **Direct**: a real bucket `GET` on the same key `has_blob`/`verify_uploaded`
    ///   HEAD (`key_for`) and `resolve_get_url` presign — one key derivation
    ///   for every path, so hydration cannot drift from existence.
    ///   `NotFound` → `Missing`; anything else → `Backend` (retryable), never
    ///   silently `Missing`.
    /// * **Delegate**: `Unhydratable`, always. There are no bucket credentials
    ///   (`self.s3` is `None` — see `delegate_skips_s3_client`) and delegate
    ///   presign protocol v1 has only `get`/`put` **URL-minting** ops, no
    ///   bytes op — so this is a structural fact about the mode, not a
    ///   failure to try. Gated by `delegate_mode_hydrate_blob_is_unhydratable_not_missing`.
    fn hydrate_blob(&self, hash: &Hash) -> Result<Vec<u8>, HydrateError> {
        let Some(s3) = self.s3.clone() else {
            return Err(HydrateError::Unhydratable {
                backend: "S3BlobStore (Delegate auth mode)".to_string(),
                detail: "delegate mode holds no bucket credentials and the delegate presign \
                         protocol v1 has no bytes-fetch op (only `get`/`put` URL minting), so \
                         the server can never read tree objects itself. Studio GC cannot run \
                         in this mode: configure S3 Direct-mode credentials, or have the app \
                         run GC on its own side"
                    .to_string(),
            });
        };
        let path = ObjectPath::from(self.key_for(hash));
        let res: Result<Bytes, object_store::Error> =
            match block_on_shared_runtime("hydrate_get", self.cfg.op_timeout, async move {
                let got = s3.get(&path).await?;
                got.bytes().await
            }) {
                Ok(r) => r,
                // A bridge/op timeout is `Backend` — retryable, "says
                // nothing about whether the object exists" — by the same
                // rule as the 503 case below. `Missing` here would let a
                // hung bucket read as lost data to the GC tree walk.
                Err(e) => return Err(HydrateError::Backend(e.to_string())),
            };
        match res {
            Ok(b) => Ok(b.to_vec()),
            Err(object_store::Error::NotFound { .. }) => Err(HydrateError::Missing),
            // A transient/permission failure must never masquerade as
            // Missing: GC treats Missing as "walk this snapshot's tree is
            // impossible", and quietly turning a 503 into that is how a
            // recoverable blip becomes an apparent lost object.
            Err(e) => Err(HydrateError::Backend(format!("{e}"))),
        }
    }

    /// S4.2 — a real bucket existence check, used by the blob HTTP origin's
    /// `HEAD /blobs/{hash}` (`blob_http.rs`) instead of the trait's default
    /// (`get_blob(hash).is_some()`, which would always be `false` here since
    /// `get_blob` above never hydrates bytes for this backend).
    ///
    /// Direct mode: a real S3 `HEAD` (reuses `direct_head`, already used by
    /// `verify_uploaded`); any error — including a 6.1 timeout — is treated
    /// as "not found" rather than panicking or propagating, matching the
    /// rest of this impl's fall-back-to-WS-on-error posture. This `bool`
    /// signature cannot carry an error, and `false` is the conservative
    /// value *for these callers*: the blob HTTP origin's `HEAD /blobs`
    /// 404 and `PUT /blobs` dedupe short-circuit, where a spurious "absent"
    /// costs a WS fallback or an idempotent re-PUT, while a spurious
    /// "present" would 200 a HEAD for bytes that can't be served. GC
    /// liveness never reads this method — it goes through `hydrate_blob` /
    /// `head_key` / `blob_gc_sweep`, all of which carry errors. Delegate
    /// mode has no bucket credentials to `HEAD` with, so it always reports
    /// `false` — a consumer needing existence in that mode should use `GET
    /// /blobs/{hash}/url` (S4.2) or its own app-side check instead.
    fn has_blob(&self, hash: &Hash) -> bool {
        match &self.cfg.auth {
            S3Auth::Direct { .. } => match self.direct_head(hash) {
                Ok(exists) => exists,
                Err(e) => {
                    tracing::warn!(%e, "has_blob: HEAD failed (backend error, NOT confirmed absence); reporting not-found to the HTTP existence probe");
                    false
                }
            },
            S3Auth::Delegate { .. } => false,
        }
    }

    /// Fallback path: when a small blob arrives over the WS, push it up
    /// to S3 so a later peer's `resolve_get_url` finds something.
    /// Direct mode only — Delegate mode has no creds and treats this as
    /// a **documented structural no-op** (`Ok`, not an error — the client
    /// should be using `request-upload` instead; see
    /// `PersistBlobError`'s doc for why "never writes through this path,
    /// by design" is not a failed write).
    ///
    /// Error classification (4.2): a bridge/op timeout is
    /// [`PersistBlobError::Unavailable`] — the bucket never answered, the
    /// write is *unconfirmed*, retry is the right move (503 at the HTTP
    /// origin). A bucket that answered with an error is
    /// [`PersistBlobError::Backend`] (500). One honest edge: the
    /// object_store client layer carries its own `op_timeout` (6.1's
    /// 3-deep bounds), so a client-layer timeout that beats the bridge's
    /// identical bound surfaces through the `Ok(Err(_))` arm and reports
    /// as `Backend` — both are genuine failure; the split only tunes the
    /// status code, and the message still names the timeout.
    fn persist_blob(&self, hash: &Hash, bytes: &[u8]) -> Result<(), PersistBlobError> {
        let Some(s3) = self.s3.clone() else {
            tracing::trace!(
                "persist_blob skipped in Delegate mode; client should request-upload instead"
            );
            return Ok(());
        };
        let path = ObjectPath::from(self.key_for(hash));
        let payload = Bytes::copy_from_slice(bytes);
        let res = block_on_shared_runtime("persist_put", self.cfg.op_timeout, async move {
            s3.put(&path, payload.into()).await
        });
        match res {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => {
                tracing::warn!(?e, "S3 persist_blob failed");
                Err(PersistBlobError::Backend(format!("s3 put: {e}")))
            }
            Err(e) => {
                tracing::warn!(%e, "S3 persist_blob failed (bridge/timeout)");
                Err(PersistBlobError::Unavailable(e.to_string()))
            }
        }
    }

    /// Two-phase GC sweep against the bucket — the S3 mirror of
    /// [`nodalmerge_server::store::DirPersistence::blob_gc_sweep`]'s
    /// semantics, and of the contract on
    /// [`BlobPersistence::blob_gc_sweep`]: an object not in `live` is
    /// **tombstoned** on the first sweep that sees it and **deleted** only
    /// on a later sweep, once its tombstone is at least `grace` old. An
    /// object that reappears in `live` has its tombstone cleared.
    ///
    /// ## Why this is explicit rather than leaning on bucket versioning
    ///
    /// This sweep used to be single-pass — bind `grace`, never read it,
    /// delete immediately — on the documented premise that "S3 object
    /// versions act as their own grace period, and operators who want
    /// hard-delete can disable versioning." That premise is exactly what
    /// made the behavior unsafe, and blob-cas-remediation.md slice 1.3
    /// replaces it rather than preserving it:
    ///
    /// * It holds only while versioning is enabled — an optional, per-bucket
    ///   feature that is **off** by default, that this crate never checks,
    ///   never sets, and cannot enforce. Every default bucket had no grace
    ///   period whatsoever.
    /// * The same comment actively invited operators to disable versioning
    ///   for hard-delete, i.e. to silently opt out of the only thing standing
    ///   in for grace, with no indication that it was load-bearing.
    /// * A noncurrent version is not a grace window in the sense the caller
    ///   means: nothing re-links it when the blob becomes live again, and
    ///   restoring it is an out-of-band operator action, not something the
    ///   server does on the next sweep.
    /// * It silently no-op'd [`Rooms::sweep_blobs`]'s `MIN_PHYSICAL_GRACE`
    ///   floor (slice 1.4, bug 2), so `--blob-gc-grace 0` meant "delete on
    ///   first sighting" on this backend no matter what the caller passed.
    ///
    /// Versioning remains a perfectly good *backstop* against operator
    /// error; it is simply not this method's grace window.
    ///
    /// ## Tombstone layout
    ///
    /// `<path_prefix>.tombstones/blake3/<hex>.<unix_millis>` — an empty
    /// object. Mirrors `DirPersistence`'s `blobs/.tombstones/blake3/<hex>`
    /// (keyed by bare hex regardless of encoding, so one tombstone covers
    /// both `<hex>` and `<hex>.zst`) with one deliberate difference: the
    /// tombstone *time* lives in the key rather than in the object's
    /// mtime/body.
    ///
    /// * **Not the object's `last_modified`:** that is the *bucket's* clock,
    ///   while `grace` is measured against this process's clock. Skew in the
    ///   unsafe direction (bucket clock behind) would delete early — the one
    ///   failure mode this whole slice exists to prevent. The timestamp in
    ///   the key is written by, and compared against, the same clock.
    /// * **Not the object's body:** that would cost a `GET` per candidate
    ///   per sweep. In the key, a single `LIST` of the tombstone prefix
    ///   yields every hash *and* its tombstone time.
    ///
    /// Since the tombstone prefix is a sibling of `<path_prefix>blake3/`,
    /// tombstones are never enumerated by (nor mistaken for) the blob
    /// listing below.
    ///
    /// Entries that don't parse as a canonical blob/zstd name are foreign
    /// per BLOB_STORAGE_LAYOUT.md §3 and must never be touched, exactly like
    /// the file store's `parse_blob_entry_name`-gated sweep — otherwise an
    /// unrelated object an operator placed under the same prefix (or a
    /// future encoding suffix a listener doesn't understand yet) would be
    /// silently deleted.
    ///
    /// Gated end-to-end against a real MinIO bucket by
    /// `server/s3-blobs/tests/minio_round_trip.rs`
    /// (`s3_blob_gc_two_phase_honors_grace_and_tombstones_first`,
    /// `s3_blob_gc_clears_tombstone_when_object_becomes_live_again`,
    /// `s3_min_physical_grace_floor_is_effective_end_to_end`).
    fn blob_gc_sweep(&self, live: &std::collections::HashSet<Hash>, grace: Duration) -> usize {
        let Some(s3) = self.s3.clone() else { return 0; };
        let prefix = ObjectPath::from(format!("{}blake3", self.cfg.path_prefix));
        let tombs_prefix = format!("{}{}", self.cfg.path_prefix, TOMBSTONE_INFIX);
        let live_set: std::collections::HashSet<String> =
            live.iter().map(|h| h.to_hex()).collect();
        // One bridge call for the whole pass, bounded by `sweep_timeout`
        // (each request inside is separately bounded by `op_timeout` via
        // the client options set in `new`). An aborted sweep = a crashed
        // sweep, which the two-phase tombstone protocol tolerates: nothing
        // is deleted without a previously persisted, aged tombstone.
        let res = block_on_shared_runtime("gc_sweep", self.cfg.sweep_timeout, async move {
                let now_ms = now_unix_millis();
                let grace_ms = grace.as_millis();

                // One LIST of the tombstone prefix up front: bare hex →
                // every tombstone key for it, with its recorded time.
                // Normally one entry per hash; two concurrent sweeps could
                // briefly leave more, so this keeps them all and ages by the
                // *newest* (smallest age ⇒ fail closed, never delete early).
                let mut tombs: std::collections::HashMap<String, Vec<(ObjectPath, u128)>> =
                    std::collections::HashMap::new();
                let tomb_root = ObjectPath::from(tombs_prefix.clone());
                let mut tomb_stream = s3.list(Some(&tomb_root));
                while let Some(meta) = tomb_stream.next().await {
                    let Ok(meta) = meta else { continue };
                    let key = meta.location.as_ref().to_string();
                    let Some(filename) = key.rsplit('/').next() else { continue };
                    let Some((hex, stamped_ms)) = parse_tombstone_name(filename) else {
                        // Foreign object under the tombstone prefix — same
                        // rule as §3: never touched, never fatal.
                        continue;
                    };
                    tombs.entry(hex).or_default().push((meta.location.clone(), stamped_ms));
                }

                // ── Slice 6.3: plan-then-execute instead of one awaited
                // delete per object (the old N+1). The LIST pass below only
                // *classifies* each object into a per-object action; the
                // actions then run [`SWEEP_IO_CONCURRENCY`] at a time via
                // `buffer_unordered`. Two-phase semantics are untouched
                // because every ordering that matters is *within* one
                // object's action — tombstone-PUT-before-delete on the
                // zero-grace branch, blob-delete-before-tombstone-cleanup on
                // the aged branch — and each action keeps its own steps
                // strictly sequential. Nothing orders one object against
                // another today either: the old loop's cross-object ordering
                // was an accident of iteration, not a protocol requirement.
                //
                // `buffer_unordered` over per-object action chains was
                // chosen over `ObjectStore::delete_stream` deliberately:
                // the AWS impl's bulk `DeleteObjects` would fold blob keys
                // and tombstone keys into shared 1000-key batches, erasing
                // exactly those per-object orderings — and a bulk batch's
                // partial-failure reporting would have to be re-mapped back
                // onto "which blob may I now count as deleted / whose
                // tombstone may I clear". Sixteen in-flight ops is far under
                // any S3 per-prefix request limit and turns the N sequential
                // round-trips into ~N/16.
                enum SweepAction {
                    /// Live again — clear its leftover tombstone key(s).
                    ClearTombstones(Vec<ObjectPath>),
                    /// Tombstone aged past grace — delete the blob, then its
                    /// tombstone key(s).
                    DeleteAged { blob: ObjectPath, tombs: Vec<ObjectPath> },
                    /// First sighting — write a tombstone; if (and only if)
                    /// the caller asked for zero grace, delete same-call
                    /// (matching `DirPersistence`'s branch exactly —
                    /// `Rooms::sweep_blobs` never passes a literal zero, see
                    /// `MIN_PHYSICAL_GRACE`, slice 1.4).
                    Tombstone { tomb_key: ObjectPath, blob: ObjectPath, delete_now: bool },
                }

                let mut actions: Vec<SweepAction> = Vec::new();
                let mut stream = s3.list(Some(&prefix));
                while let Some(meta) = stream.next().await {
                    let Ok(meta) = meta else { continue };
                    let key = meta.location.as_ref();
                    let Some(filename) = key.rsplit('/').next() else { continue };
                    let Some((hash, _encoding)) = parse_blob_entry_name(filename) else {
                        // Foreign entry — never delete, never error (§3).
                        continue;
                    };
                    let hex = hash.to_hex();

                    if live_set.contains(&hex) {
                        // Referenced: clear any leftover tombstone so a brief
                        // unreference-then-rereference (e.g. a concurrent
                        // SetBlob arriving between sweeps) doesn't doom the
                        // object next round.
                        if let Some(existing) = tombs.get(&hex) {
                            actions.push(SweepAction::ClearTombstones(
                                existing.iter().map(|(p, _)| p.clone()).collect(),
                            ));
                        }
                        continue;
                    }

                    // Not live — consult the tombstone.
                    match tombs.get(&hex).and_then(|v| v.iter().map(|(_, ms)| *ms).max()) {
                        Some(newest_ms) => {
                            // `saturating_sub`: a tombstone stamped in the
                            // future (clock stepped back between sweeps)
                            // reads as age 0 — not aged, so we wait rather
                            // than delete. Fail closed.
                            let age_ms = now_ms.saturating_sub(newest_ms);
                            if age_ms >= grace_ms {
                                actions.push(SweepAction::DeleteAged {
                                    blob: meta.location.clone(),
                                    tombs: tombs
                                        .get(&hex)
                                        .into_iter()
                                        .flatten()
                                        .map(|(p, _)| p.clone())
                                        .collect(),
                                });
                            }
                        }
                        None => {
                            let tomb_key =
                                ObjectPath::from(format!("{tombs_prefix}/{hex}.{now_ms}"));
                            actions.push(SweepAction::Tombstone {
                                tomb_key,
                                blob: meta.location.clone(),
                                delete_now: grace.is_zero(),
                            });
                        }
                    }
                }

                futures_util::stream::iter(actions.into_iter().map(|action| {
                    let s3 = s3.clone();
                    async move {
                        match action {
                            SweepAction::ClearTombstones(paths) => {
                                for path in &paths {
                                    if let Err(e) = s3.delete(path).await {
                                        tracing::warn!(?e, key = %path, "blob_gc_sweep: clear tombstone failed");
                                    }
                                }
                                0usize
                            }
                            SweepAction::DeleteAged { blob, tombs } => {
                                // Blob first; a failed blob delete keeps the
                                // tombstone so a later sweep retries — never
                                // clear a tombstone for bytes still present.
                                if let Err(e) = s3.delete(&blob).await {
                                    tracing::warn!(?e, key = %blob, "blob_gc_sweep: delete failed");
                                    return 0;
                                }
                                for path in &tombs {
                                    let _ = s3.delete(path).await;
                                }
                                1
                            }
                            SweepAction::Tombstone { tomb_key, blob, delete_now } => {
                                // Tombstone before any delete — the write
                                // that makes an aborted/crashed sweep safe.
                                if let Err(e) = s3.put(&tomb_key, Bytes::new().into()).await {
                                    tracing::warn!(?e, key = %tomb_key, "blob_gc_sweep: create tombstone failed");
                                    return 0;
                                }
                                if !delete_now {
                                    return 0;
                                }
                                if let Err(e) = s3.delete(&blob).await {
                                    tracing::warn!(?e, key = %blob, "blob_gc_sweep: immediate delete failed");
                                    return 0;
                                }
                                let _ = s3.delete(&tomb_key).await;
                                1
                            }
                        }
                    }
                }))
                .buffer_unordered(SWEEP_IO_CONCURRENCY)
                .fold(0usize, |acc, n| async move { acc + n })
                .await
        });
        match res {
            Ok(deleted) => deleted,
            // Timed out / bridge lost: report zero deletions — the
            // conservative sweep outcome — and say loudly why, because a
            // sweep that "reclaims nothing" every tick against a hung
            // bucket would otherwise look like a healthy no-op.
            Err(e) => {
                tracing::error!(%e, "blob_gc_sweep aborted by timeout/bridge failure; no deletions reported");
                0
            }
        }
    }

    fn resolve_get_url(
        &self,
        room_id: &str,
        hash: &Hash,
        _size_hint: Option<u64>,
    ) -> Option<PresignedUrl> {
        let ttl = self.cfg.presign_get_ttl;
        let url = match &self.cfg.auth {
            S3Auth::Direct { .. } => match self.direct_presign_get(hash) {
                Ok(u) => u,
                Err(e) => {
                    tracing::warn!(?e, "presign GET failed; falling back to WS");
                    return None;
                }
            },
            S3Auth::Delegate { .. } => match self.delegate_request(
                "get",
                room_id,
                hash,
                None,
                ttl,
                None,
            ) {
                Ok(Some(u)) => u,
                Ok(None) => return None,
                Err(e) => {
                    tracing::warn!(?e, "delegate GET failed; falling back to WS");
                    return None;
                }
            },
        };
        Some(PresignedUrl::with_ttl(url, ttl))
    }

    fn resolve_put_url(
        &self,
        room_id: &str,
        hash: &Hash,
        size: u64,
        content_type: Option<&str>,
    ) -> Option<PresignedUrl> {
        if size < self.cfg.direct_upload_threshold {
            tracing::debug!(room = %room_id, hash = %hash.to_hex(), size = size, threshold = self.cfg.direct_upload_threshold, "resolve_put_url: size below threshold, skipping presign");
            return None;
        }
        let ttl = self.cfg.presign_put_ttl;
        let url = match &self.cfg.auth {
            S3Auth::Direct { .. } => match self.direct_presign_put(hash) {
                Ok(u) => u,
                Err(e) => {
                    tracing::warn!(?e, "presign PUT failed; falling back to WS");
                    return None;
                }
            },
            S3Auth::Delegate { .. } => match self.delegate_request(
                "put",
                room_id,
                hash,
                Some(size),
                ttl,
                content_type.map(|s| s.to_string()),
            ) {
                Ok(Some(u)) => u,
                Ok(None) => return None,
                Err(e) => {
                    tracing::warn!(?e, "delegate PUT failed; falling back to WS");
                    return None;
                }
            },
        };
        Some(PresignedUrl::with_ttl(url, ttl))
    }

    fn verify_uploaded(&self, _room_id: &str, hash: &Hash) -> Result<(), String> {
        match &self.cfg.auth {
            S3Auth::Direct { .. } => match self.direct_head(hash) {
                Ok(true) => Ok(()),
                Ok(false) => Err("object missing after upload".into()),
                Err(e) => Err(format!("HEAD failed: {e}")),
            },
            // Delegate mode can't HEAD without creds — trust the client.
            // The app's presign endpoint can do its own verification if
            // it wants, by the time the room asks for the same hash on
            // the next read.
            S3Auth::Delegate { .. } => Ok(()),
        }
    }

    fn blobs_durable(&self) -> bool { true }

    /// S4.2 — this backend is always presign-capable (in both auth modes),
    /// distinguishing it from `NoPersistence`/`DirPersistence`'s default
    /// `false`. See `BlobPersistence::supports_presigned_urls`'s doc for why
    /// this can't just be inferred from `verify_uploaded`'s return value:
    /// `Delegate` mode's `Ok(())` there means "trust the client" (a real
    /// backend decision), not "no backend at all".
    fn supports_presigned_urls(&self) -> bool {
        true
    }
}

// ─── S5.3: nodalmerge_gc::contracts::BlobObjectStore ────────────────────────

/// S5.3 — the GC coordinator's hard-sweep HEAD/DELETE surface, backed by an
/// S3-compatible bucket. Direct-mode only (Delegate mode never holds bucket
/// credentials to HEAD/DELETE with — a hard sweep against a Delegate-mode
/// deployment must use a different `BlobObjectStore`, out of this slice's
/// scope). Owns its own [`S3BlobStore`] instance (a second, independent
/// client built from the same [`S3BlobStoreConfig`] the persistence layer
/// uses) rather than sharing the one `nodalmerge_server::store::Composite`
/// owns by value — `S3BlobStore::new` is cheap (just builds an HTTP/S3
/// client, no heavyweight state), and this keeps the GC adapter decoupled
/// from whatever owns the persistence-facing instance.
pub struct S3BlobObjectStore {
    inner: S3BlobStore,
}

impl S3BlobObjectStore {
    pub fn new(cfg: S3BlobStoreConfig) -> Result<Self, S3BlobError> {
        Ok(Self { inner: S3BlobStore::new(cfg)? })
    }
}

impl nodalmerge_gc::contracts::BlobObjectStore for S3BlobObjectStore {
    fn head(&self, bucket: &str, key: &str) -> nodalmerge_gc::GcResult<bool> {
        if bucket != self.inner.cfg.bucket {
            return Err(nodalmerge_gc::GcError::Backend(format!(
                "S3BlobObjectStore is bound to bucket {:?}, got {bucket:?}",
                self.inner.cfg.bucket
            )));
        }
        self.inner
            .head_key(key)
            .map_err(|e| nodalmerge_gc::GcError::Backend(e.to_string()))
    }

    fn delete(&self, bucket: &str, key: &str) -> nodalmerge_gc::GcResult<()> {
        if bucket != self.inner.cfg.bucket {
            return Err(nodalmerge_gc::GcError::Backend(format!(
                "S3BlobObjectStore is bound to bucket {:?}, got {bucket:?}",
                self.inner.cfg.bucket
            )));
        }
        self.inner
            .delete_key(key)
            .map_err(|e| nodalmerge_gc::GcError::Backend(e.to_string()))
    }
}

/// S5.3 — a `gc_store::KeyScheme` that derives `(bucket, object_key)` the
/// same way [`S3BlobStore::key_for`] does (`{path_prefix}blake3/{hex}`), so
/// `AssetRecord` rows the GC ledger writes line up with what
/// [`S3BlobObjectStore`] actually HEADs/DELETEs.
pub fn s3_key_scheme(bucket: String, path_prefix: String) -> nodalmerge_server::gc_store::KeyScheme {
    std::sync::Arc::new(move |hash: &str| (bucket.clone(), format!("{path_prefix}blake3/{hash}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_sane() {
        let c = S3BlobStoreConfig::default();
        assert_eq!(c.region, "us-east-1");
        assert_eq!(c.path_prefix, "blobs/");
        assert_eq!(c.presign_get_ttl, Duration::from_secs(3600));
        assert_eq!(c.presign_put_ttl, Duration::from_secs(900));
        assert_eq!(c.direct_upload_threshold, 1 * 1024 * 1024);
        assert!(c.require_https);
        assert_eq!(c.op_timeout, Duration::from_secs(30));
        assert_eq!(c.connect_timeout, Duration::from_secs(10));
        assert_eq!(c.sweep_timeout, Duration::from_secs(900));
    }

    /// Slice 6.1 (finding #11): N ops → exactly 1 runtime, ever. Pre-6.1
    /// each of the nine bridge sites ran `tokio::runtime::Runtime::new()`
    /// inside a fresh `std::thread::spawn` per call — N ops meant N
    /// runtimes (that half is demonstrated by the old code shape, not by a
    /// test; the counter didn't exist to observe it). Post-6.1 the counter
    /// sits inside `BRIDGE_RUNTIME`'s init closure, so it counts real
    /// constructions: several ops of different shapes (HEAD, GET,
    /// delegate HTTP) through the bridge must leave it at 1. This is the
    /// only test in this binary that touches the bridge, so the assertion
    /// is exact, not `<=`.
    #[test]
    fn bridge_reuses_one_shared_runtime_across_ops() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for conn in listener.incoming() {
                match conn {
                    Ok(s) => held.push(s),
                    Err(_) => break,
                }
            }
        });

        let mut cfg = S3BlobStoreConfig::default();
        cfg.bucket = "b".into();
        cfg.endpoint = Some(format!("http://{addr}"));
        cfg.require_https = false;
        cfg.auth = S3Auth::direct_explicit("ak", "sk");
        cfg.op_timeout = Duration::from_millis(200);
        cfg.connect_timeout = Duration::from_millis(200);
        let direct = S3BlobStore::new(cfg).unwrap();

        let mut cfg = S3BlobStoreConfig::default();
        cfg.bucket = "b".into();
        cfg.auth = S3Auth::delegate(format!("http://{addr}/presign"), None);
        cfg.op_timeout = Duration::from_millis(200);
        cfg.connect_timeout = Duration::from_millis(200);
        let delegate = S3BlobStore::new(cfg).unwrap();

        let h = Hash::of(b"whatever");
        let _ = direct.has_blob(&h);
        let _ = direct.hydrate_blob(&h);
        let _ = direct.verify_uploaded("room", &h);
        let _ = delegate.resolve_get_url("room", &h, None);

        assert_eq!(
            BRIDGE_RUNTIMES_BUILT.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "four ops across two stores and two auth modes must share one runtime"
        );
    }

    #[test]
    fn missing_bucket_rejected() {
        let cfg = S3BlobStoreConfig::default();
        let err = S3BlobStore::new(cfg).unwrap_err();
        assert!(matches!(err, S3BlobError::Config(_)));
    }

    #[test]
    fn http_endpoint_rejected_when_https_required() {
        let mut cfg = S3BlobStoreConfig::default();
        cfg.bucket = "b".into();
        cfg.endpoint = Some("http://localhost:9000".into());
        let err = S3BlobStore::new(cfg).unwrap_err();
        assert!(matches!(err, S3BlobError::Config(_)));
    }

    #[test]
    fn http_endpoint_allowed_when_https_disabled() {
        let mut cfg = S3BlobStoreConfig::default();
        cfg.bucket = "b".into();
        cfg.endpoint = Some("http://localhost:9000".into());
        cfg.require_https = false;
        cfg.auth = S3Auth::direct_explicit("ak", "sk");
        // We don't actually hit the network — just confirm the builder
        // accepts it and we get a store back.
        let _ = S3BlobStore::new(cfg).expect("builder should accept http when allowed");
    }

    /// Asserts `S3BlobStore::key_for` against the canonical vectors
    /// (`engine/commands/blob-layout-vectors.v1.json`). The Rust file-store
    /// and .NET mirrors are `server/server/tests/blob_layout_vectors.rs`
    /// and `hosts/dotnet/tests/NodalMerge.DotNetHost.Tests/BlobLayoutParityTests.cs`.
    #[test]
    fn blob_layout_vectors_match_s3_key_derivation() {
        const VECTORS_JSON: &str =
            include_str!("../../../engine/commands/blob-layout-vectors.v1.json");

        #[derive(serde::Deserialize)]
        struct VectorsFile {
            path_vectors: Vec<PathVector>,
        }

        #[derive(serde::Deserialize)]
        struct PathVector {
            id: String,
            hash: String,
            #[serde(default)]
            prefix: Option<String>,
            #[serde(default)]
            s3_key: Option<String>,
        }

        let file: VectorsFile = serde_json::from_str(VECTORS_JSON)
            .expect("engine/commands/blob-layout-vectors.v1.json must parse");

        let mut failures = Vec::new();
        for vector in &file.path_vectors {
            let (Some(prefix), Some(expected)) = (&vector.prefix, &vector.s3_key) else {
                continue;
            };

            let mut cfg = S3BlobStoreConfig::default();
            cfg.bucket = "b".into();
            cfg.endpoint = Some("https://localhost:9000".into());
            cfg.auth = S3Auth::direct_explicit("ak", "sk");
            cfg.path_prefix = prefix.clone();
            let store = S3BlobStore::new(cfg).unwrap();

            let hash_hex = &vector.hash;
            assert_eq!(hash_hex.len(), 64, "vector hash must be 64 hex chars");
            let mut bytes = [0u8; 32];
            for i in 0..32 {
                let hi = (hash_hex.as_bytes()[2 * i] as char).to_digit(16).unwrap() as u8;
                let lo = (hash_hex.as_bytes()[2 * i + 1] as char).to_digit(16).unwrap() as u8;
                bytes[i] = (hi << 4) | lo;
            }
            let hash = Hash(bytes);

            let actual = store.key_for(&hash);
            if &actual != expected {
                failures.push(format!(
                    "vector `{}`: expected s3_key {expected}, got {actual}",
                    vector.id
                ));
            }
        }

        assert!(
            failures.is_empty(),
            "blob layout vectors drifted from S3 key derivation:\n{}",
            failures.join("\n")
        );
    }

    #[test]
    fn key_layout_is_flat_global_cas_no_room_segment() {
        let mut cfg = S3BlobStoreConfig::default();
        cfg.bucket = "b".into();
        cfg.endpoint = Some("https://localhost:9000".into());
        cfg.auth = S3Auth::direct_explicit("ak", "sk");
        let store = S3BlobStore::new(cfg).unwrap();
        let h = Hash::of(b"hi");
        let key = store.key_for(&h);
        assert_eq!(key, format!("blobs/blake3/{}", h.to_hex()));
    }

    #[test]
    fn key_layout_honors_configured_prefix() {
        let mut cfg = S3BlobStoreConfig::default();
        cfg.bucket = "b".into();
        cfg.endpoint = Some("https://localhost:9000".into());
        cfg.auth = S3Auth::direct_explicit("ak", "sk");
        cfg.path_prefix = "assets/".into();
        let store = S3BlobStore::new(cfg).unwrap();
        let h = Hash::of(b"hi");
        let key = store.key_for(&h);
        assert_eq!(key, format!("assets/blake3/{}", h.to_hex()));
    }

    #[test]
    fn delegate_skips_s3_client() {
        let mut cfg = S3BlobStoreConfig::default();
        cfg.bucket = "b".into();
        cfg.auth = S3Auth::delegate("https://app.example.com/presign", None);
        let store = S3BlobStore::new(cfg).unwrap();
        assert!(store.s3.is_none(), "Delegate mode must not hold S3 creds");
    }

    #[test]
    fn supports_presigned_urls_is_always_true() {
        // Direct mode.
        let mut cfg = S3BlobStoreConfig::default();
        cfg.bucket = "b".into();
        cfg.endpoint = Some("https://localhost:9000".into());
        cfg.auth = S3Auth::direct_explicit("ak", "sk");
        let store = S3BlobStore::new(cfg).unwrap();
        assert!(store.supports_presigned_urls());

        // Delegate mode.
        let mut cfg = S3BlobStoreConfig::default();
        cfg.bucket = "b".into();
        cfg.auth = S3Auth::delegate("https://app.example.com/presign", None);
        let store = S3BlobStore::new(cfg).unwrap();
        assert!(store.supports_presigned_urls());
    }

    #[test]
    fn delegate_mode_has_blob_always_false_no_network() {
        // Delegate mode has no bucket credentials to HEAD with; has_blob
        // must report false without attempting any network call (there's no
        // client to make one with — `store.s3` is `None` in this mode, see
        // `delegate_skips_s3_client`).
        let mut cfg = S3BlobStoreConfig::default();
        cfg.bucket = "b".into();
        cfg.auth = S3Auth::delegate("https://app.example.com/presign", None);
        let store = S3BlobStore::new(cfg).unwrap();
        assert!(!store.has_blob(&Hash::of(b"anything")));
    }

    #[test]
    fn small_upload_skips_presign() {
        let mut cfg = S3BlobStoreConfig::default();
        cfg.bucket = "b".into();
        cfg.endpoint = Some("https://localhost:9000".into());
        cfg.auth = S3Auth::direct_explicit("ak", "sk");
        cfg.direct_upload_threshold = 1024;
        let store = S3BlobStore::new(cfg).unwrap();
        // 100 bytes < 1 KiB threshold → resolve_put_url returns None
        // without ever hitting the (mock) endpoint.
        let url = store.resolve_put_url("room", &Hash::of(b"x"), 100, None);
        assert!(url.is_none(), "small uploads must fall through to WS");
    }

    // ─── blob-cas-remediation.md slice 2.2 (finding #10), Delegate half ──────
    //
    // Direct mode hydrates tree objects with a real bucket GET (gated for
    // real against MinIO in `tests/minio_round_trip.rs`). Delegate mode
    // genuinely *cannot*: it holds no bucket credentials (`store.s3` is
    // `None` — see `delegate_skips_s3_client`), and the delegate presign
    // protocol v1 has no "give me the bytes" op, only `get`/`put` URL
    // minting. So 2.2's "or fails loud" half applies here, and these tests
    // are what make "GC never runs on S3-delegate" impossible to miss.

    /// The whole point: a Delegate-mode server must NOT report a tree object
    /// it cannot hydrate as a generic "missing blob". `MissingBlob` sends an
    /// operator hunting for a lost object; the truth is a *configuration*
    /// fact about this deployment, and it must say so.
    #[test]
    fn delegate_mode_tree_walk_fails_loud_not_as_a_generic_missing_blob() {
        let mut cfg = S3BlobStoreConfig::default();
        cfg.bucket = "b".into();
        cfg.auth = S3Auth::delegate("https://app.example.com/presign", None);
        let store = S3BlobStore::new(cfg).unwrap();

        let tree = Hash::of(b"a tree object living in the app's bucket");
        let err = nodalmerge_server::tree_walk::walk_tree(&store, &tree)
            .expect_err("Delegate mode cannot hydrate; the walk must fail");

        let msg = err.to_string();
        assert!(
            !msg.contains("missing tree/blob object"),
            "finding #10: Delegate mode must not disguise 'this backend has no bucket              credentials to read tree objects with' as a generic missing-blob error —              that is exactly the indistinguishable-from-a-real-bug failure the slice              exists to remove. Got: {msg}"
        );
        // Actionable: name the backend, the mode, and what an operator can do.
        for needle in ["delegate", "hydrate"] {
            assert!(
                msg.to_lowercase().contains(needle),
                "the error must be actionable and name {needle:?}; got: {msg}"
            );
        }
    }

    /// `hydrate_blob` is the seam the walk goes through; assert the backend's
    /// own answer directly, not only through `walk_tree`, so a future caller
    /// (e.g. slice 2.3's archive export) inherits the same loud failure.
    #[test]
    fn delegate_mode_hydrate_blob_is_unhydratable_not_missing() {
        let mut cfg = S3BlobStoreConfig::default();
        cfg.bucket = "b".into();
        cfg.auth = S3Auth::delegate("https://app.example.com/presign", None);
        let store = S3BlobStore::new(cfg).unwrap();

        let err = store
            .hydrate_blob(&Hash::of(b"anything"))
            .expect_err("Delegate mode can never hydrate bytes");
        assert!(
            matches!(err, nodalmerge_server::store::HydrateError::Unhydratable { .. }),
            "must be Unhydratable (a config fact), never Missing (a data fact): {err:?}"
        );
    }
}
