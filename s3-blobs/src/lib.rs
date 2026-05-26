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
//! let nodes = DirPersistence::open("/var/lib/activesync")?;
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
use nodalmerge_server::store::{BlobPersistence, PresignedUrl};
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
// Avoid creating a tokio runtime on the current thread (which may already
// be driving one). Instead we spawn a short-lived thread and run a runtime
// there for the few sync-to-async bridges below.
use url::Url;

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
    /// The endpoint must accept JSON of the shape
    /// ```json
    /// { "op": "get" | "put",
    ///   "room_id": "...",
    ///   "hash": "...",
    ///   "size": 12345,
    ///   "ttl_seconds": 3600 }
    /// ```
    /// and respond with
    /// ```json
    /// { "url": "https://...", "expires_at_unix": 1738454400 }
    /// ```
    /// or HTTP 4xx/5xx (treated as "no URL — use WS fallback").
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
}

// ─── The store ──────────────────────────────────────────────────────────────

/// S3-compatible `BlobPersistence` backend.
pub struct S3BlobStore {
    cfg: S3BlobStoreConfig,
    /// Direct mode only — used to mint presigned URLs and run uploads.
    /// `None` for Delegate mode.
    s3: Option<Arc<AmazonS3>>,
    /// Delegate mode only — async HTTP client.
    http: reqwest::Client,
    // No persistent runtime here — we spawn a short-lived runtime in a
    // dedicated thread for each sync-to-async bridge to avoid starting a
    // runtime on a thread that may already be running one.
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
                let mut b = AmazonS3Builder::new()
                    .with_bucket_name(&cfg.bucket)
                    .with_region(&cfg.region);
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

        Ok(Self { cfg, s3, http: reqwest::Client::new() })
    }

    fn key_for(&self, room_id: &str, hash: &Hash) -> String {
        // Sanitize to the same shape `DirPersistence::sanitize` would
        // produce: any byte outside `[A-Za-z0-9_-]` becomes `_XX`. Keeps
        // arbitrary room ids safe in S3 object keys.
        let mut sanitized = String::with_capacity(room_id.len());
        for b in room_id.as_bytes() {
            let c = *b;
            if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' {
                sanitized.push(c as char);
            } else {
                sanitized.push_str(&format!("_{:02X}", c));
            }
        }
        format!("{}{}/{}", self.cfg.path_prefix, sanitized, hash.to_hex())
    }

    /// Presign a GET via object_store (Direct mode).
    fn direct_presign_get(
        &self,
        room_id: &str,
        hash: &Hash,
    ) -> Result<String, S3BlobError> {
        let s3 = self.s3.as_ref().expect("direct_presign_get without s3 client");
        let path = ObjectPath::from(self.key_for(room_id, hash));
        let ttl = self.cfg.presign_get_ttl;
        let s3 = s3.clone();
        // Run the async call on a fresh runtime inside a spawned thread so
        // we don't attempt to create or block on a runtime on the current
        // thread (which may already be running Tokio).
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt2 = tokio::runtime::Runtime::new().expect("create runtime");
            let res = rt2.block_on(async move { s3.signed_url(reqwest::Method::GET, &path, ttl).await });
            let _ = tx.send(res);
        });
        let url_res: Result<Url, object_store::Error> = rx
            .recv()
            .map_err(|e| S3BlobError::Config(format!("thread error: {e}")))?;
        let url = url_res.map_err(S3BlobError::ObjectStore)?;
        Ok(url.to_string())
    }

    /// Presign a PUT via object_store (Direct mode).
    fn direct_presign_put(
        &self,
        room_id: &str,
        hash: &Hash,
    ) -> Result<String, S3BlobError> {
        let s3 = self.s3.as_ref().expect("direct_presign_put without s3 client");
        let path = ObjectPath::from(self.key_for(room_id, hash));
        let ttl = self.cfg.presign_put_ttl;
        let s3 = s3.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt2 = tokio::runtime::Runtime::new().expect("create runtime");
            let res = rt2.block_on(async move { s3.signed_url(reqwest::Method::PUT, &path, ttl).await });
            let _ = tx.send(res);
        });
        let url_res: Result<Url, object_store::Error> = rx
            .recv()
            .map_err(|e| S3BlobError::Config(format!("thread error: {e}")))?;
        let url = url_res.map_err(S3BlobError::ObjectStore)?;
        Ok(url.to_string())
    }

    /// HEAD an object — used by `verify_uploaded` (Direct mode only).
    fn direct_head(&self, room_id: &str, hash: &Hash) -> Result<bool, S3BlobError> {
        let s3 = self.s3.as_ref().expect("direct_head without s3 client");
        let path = ObjectPath::from(self.key_for(room_id, hash));
        let s3 = s3.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt2 = tokio::runtime::Runtime::new().expect("create runtime");
            let res = rt2.block_on(async move { s3.head(&path).await });
            let _ = tx.send(res);
        });
        let res: Result<_, object_store::Error> = rx
            .recv()
            .map_err(|e| S3BlobError::Config(format!("thread error: {e}")))?;
        let res = res;
        match res {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
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
        #[derive(Serialize)]
        struct Req {
            op: &'static str,
            room_id: String,
            hash: String,
            size: Option<u64>,
            ttl_seconds: u64,
            // Algorithm helps the app choose canonical S3 key layout (e.g. "blake3").
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
            room_id: room_id.to_string(),
            hash: hash.to_hex(),
            size,
            ttl_seconds: ttl.as_secs(),
            algorithm: "blake3".to_string(),
            content_type,
        };
        tracing::debug!(%op, room = %room_id, hash = %hash.to_hex(), size = ?size, %body.algorithm, endpoint = %endpoint, "delegate_request: sending presign request to app");
        let http = self.http.clone();
        // Run delegate HTTP request on a fresh runtime in a spawned thread.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt2 = tokio::runtime::Runtime::new().expect("create runtime");
            let res = rt2.block_on(async move {
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
            });
            let _ = tx.send(res);
        });
        rx.recv().map_err(|e| S3BlobError::Config(format!("thread error: {e}")))?
    }
}

// ─── BlobPersistence impl ───────────────────────────────────────────────────

impl BlobPersistence for S3BlobStore {
    /// `S3BlobStore` does *not* hydrate every blob on room open — that
    /// would defeat the purpose of offloading them. Returns empty; the
    /// SDK pulls blobs lazily via `resolve_get_url`.
    fn load_room_blobs(&self, _room_id: &str) -> Vec<(Hash, Vec<u8>)> {
        Vec::new()
    }

    /// Fallback path: when a small blob arrives over the WS, push it up
    /// to S3 so a later peer's `resolve_get_url` finds something.
    /// Direct mode only — Delegate mode has no creds and treats this as
    /// a no-op (the client should be using `request-upload` instead).
    fn persist_blob(&self, room_id: &str, hash: &Hash, bytes: &[u8]) {
        let Some(s3) = self.s3.clone() else {
            tracing::trace!(
                "persist_blob skipped in Delegate mode; client should request-upload instead"
            );
            return;
        };
        let path = ObjectPath::from(self.key_for(room_id, hash));
        let payload = Bytes::copy_from_slice(bytes);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt2 = tokio::runtime::Runtime::new().expect("create runtime");
            let res = rt2.block_on(async move { s3.put(&path, payload.into()).await });
            let _ = tx.send(res);
        });
        if let Err(e) = rx.recv().map_err(|e| S3BlobError::Config(format!("thread error: {e}"))).and_then(|r| r.map_err(S3BlobError::ObjectStore)) {
            tracing::warn!(?e, "S3 persist_blob failed");
        }
    }

    fn blob_gc_sweep(
        &self,
        room_id: &str,
        live: &std::collections::HashSet<Hash>,
        _grace: Duration,
    ) -> usize {
        // S3 GC: list under prefix, drop any object whose hash isn't
        // live. We collapse the two phases into one — S3 object versions
        // (when enabled) act as their own grace period, and operators
        // who want hard-delete can disable versioning.
        let Some(s3) = self.s3.clone() else { return 0; };
        let prefix = ObjectPath::from(format!(
            "{}{}",
            self.cfg.path_prefix,
            sanitize_room(room_id)
        ));
        let live_set: std::collections::HashSet<String> =
            live.iter().map(|h| h.to_hex()).collect();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt2 = tokio::runtime::Runtime::new().expect("create runtime");
            let res = rt2.block_on(async move {
                let mut deleted = 0usize;
                let mut stream = s3.list(Some(&prefix));
                while let Some(meta) = stream.next().await {
                    let Ok(meta) = meta else { continue };
                    let key = meta.location.as_ref();
                    let Some(filename) = key.rsplit('/').next() else { continue };
                    if !live_set.contains(filename) {
                        if let Err(e) = s3.delete(&meta.location).await {
                            tracing::warn!(?e, key = %meta.location, "blob_gc_sweep: delete failed");
                        } else {
                            deleted += 1;
                        }
                    }
                }
                deleted
            });
            let _ = tx.send(res);
        });
        rx.recv().map_err(|_| 0).unwrap_or(0)
    }

    fn resolve_get_url(
        &self,
        room_id: &str,
        hash: &Hash,
        _size_hint: Option<u64>,
    ) -> Option<PresignedUrl> {
        let ttl = self.cfg.presign_get_ttl;
        let url = match &self.cfg.auth {
            S3Auth::Direct { .. } => match self.direct_presign_get(room_id, hash) {
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
            S3Auth::Direct { .. } => match self.direct_presign_put(room_id, hash) {
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

    fn verify_uploaded(&self, room_id: &str, hash: &Hash) -> Result<(), String> {
        match &self.cfg.auth {
            S3Auth::Direct { .. } => match self.direct_head(room_id, hash) {
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
}

fn sanitize_room(room_id: &str) -> String {
    let mut sanitized = String::with_capacity(room_id.len());
    for b in room_id.as_bytes() {
        let c = *b;
        if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' {
            sanitized.push(c as char);
        } else {
            sanitized.push_str(&format!("_{:02X}", c));
        }
    }
    sanitized
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

    #[test]
    fn key_layout_round_trips_funny_room_ids() {
        let mut cfg = S3BlobStoreConfig::default();
        cfg.bucket = "b".into();
        cfg.endpoint = Some("https://localhost:9000".into());
        cfg.auth = S3Auth::direct_explicit("ak", "sk");
        let store = S3BlobStore::new(cfg).unwrap();
        let h = Hash::of(b"hi");
        let key = store.key_for("rooms/Alice & Bob!", &h);
        assert!(key.starts_with("blobs/rooms_2FAlice_20_26_20Bob_21/"));
        assert!(key.ends_with(&h.to_hex()));
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
}
