//! F4 — server-side persistence. F6 — trait split.
//!
//! The persistence layer is split along the axis products actually want to
//! mix and match: **nodes** (small, frequent, want indexed SQL/Mongo/Postgres)
//! and **blobs** (large, rare, want S3/R2/filesystem). Each half is its own
//! trait; a `ServerPersistence` supertrait exists so call sites that want
//! "the whole persistence surface" (room hydration, write-through) don't
//! have to name two trait objects.
//!
//! A blanket impl means any tuple `(N, B)` where `N: NodePersistence` and
//! `B: BlobPersistence` automatically implements `ServerPersistence` — so
//! `Arc::new((MongoNodeStore::…, S3BlobStore::…))` works out of the box.
//!
//! Two bundled implementations ship:
//!
//! * [`NoPersistence`] — in-memory only; matches pre-F4 behavior.
//! * [`DirPersistence`] — SQLite for nodes (`<root>/nodalmerge.db`) plus a
//!   content-addressed blob dir (`<root>/blobs/<room>/<hash>`). Hydrates on
//!   room creation; write-through on every accepted node/blob.
//!
//! External backends (e.g. `nodalmerge-s3-blobs::S3BlobStore`) implement
//! just `BlobPersistence` and compose with any `NodePersistence`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use nodalmerge_core::{pack_nodes, unpack_nodes, Hash, SyncNode};
use rusqlite::{params, Connection};

/// F6 — a presigned URL plus its absolute Unix-second expiration.
///
/// The SDK caches URLs and refreshes ~60 s before `expires_at_unix`, so
/// returning an accurate timestamp lets clients minimize round-trips. If
/// the backend genuinely doesn't know the lifetime, return `u64::MAX` for
/// `expires_at_unix` and the SDK will only refresh on 403/expired error.
#[derive(Debug, Clone)]
pub struct PresignedUrl {
    pub url: String,
    pub expires_at_unix: u64,
}

impl PresignedUrl {
    /// Helper: build from a TTL relative to *now*.
    pub fn with_ttl(url: impl Into<String>, ttl: Duration) -> Self {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            url: url.into(),
            expires_at_unix: now.saturating_add(ttl.as_secs()),
        }
    }
}

// ─── Trait split (F6) ───────────────────────────────────────────────────────

/// Node-side persistence. See module docs for rationale.
pub trait NodePersistence: Send + Sync + std::fmt::Debug {
    /// Return every previously-persisted node for `room_id`, in insertion order.
    fn load_room_nodes(&self, room_id: &str) -> Vec<SyncNode>;
    /// Persist a single accepted node.
    fn persist_node(&self, room_id: &str, node: &SyncNode);
    /// Persist many accepted nodes in a single batch. Default impl loops
    /// [`persist_node`]; backends with transactional semantics should
    /// override to amortize fsync / commit cost across the whole batch.
    fn persist_nodes(&self, room_id: &str, nodes: &[&SyncNode]) {
        for n in nodes {
            self.persist_node(room_id, n);
        }
    }
    /// `true` if this backend survives process restarts. See
    /// [`ServerPersistence::is_durable`] for the combined durability used
    /// by the idle-eviction sweeper.
    fn nodes_durable(&self) -> bool {
        true
    }

    /// Every room id with at least one persisted node — including rooms
    /// that are not currently loaded in memory.
    ///
    /// Used by global blob GC ([`BlobPersistence::blob_gc_sweep`]) to
    /// compute the live blob set across *every* room, not just resident
    /// ones — required now that blobs are a single global CAS pool rather
    /// than per-room directories (see `docs/BLOB_STORAGE_LAYOUT.md` §4).
    /// Default: empty, meaning a caller relying on this alone would only
    /// see resident rooms — backends without a room index should override
    /// if they want cold rooms protected from GC.
    ///
    /// ⚠ **An empty result here is ambiguous by itself** — it means both
    /// "this backend genuinely has no other rooms" and "this backend never
    /// implemented enumeration." Callers that need to tell those apart
    /// (i.e. anything that might delete based on the result) MUST also
    /// consult [`Self::can_enumerate_rooms`] rather than trusting an empty
    /// `Vec` as proof of completeness.
    fn known_room_ids(&self) -> Vec<String> {
        Vec::new()
    }

    /// blob-cas-remediation.md slice 1.1 (finding #1) — the capability
    /// signal that makes [`Self::known_room_ids`]'s empty default *safe by
    /// construction* instead of silently authoritative.
    ///
    /// Before this existed, `Composite<N, B>` didn't forward
    /// `known_room_ids` at all, and `PostgresNodeStore`/`MongoNodeStore`
    /// never implemented it — so every one of those (durable, production)
    /// wirings reported zero cold rooms regardless of how many actually had
    /// persisted blobs, and the global blob GC sweep
    /// (`Rooms::sweep_blobs`) read that as "no cold rooms have blobs" and
    /// deleted them.
    ///
    /// `true` means `known_room_ids()` is a genuine, complete enumeration
    /// of every room with persisted nodes — the sweep may trust an empty
    /// result as proof there are no cold rooms to protect. `false` (the
    /// default) means the backend hasn't confirmed that, and the sweep
    /// must fail closed: refuse to run its delete pass rather than risk
    /// treating "I don't know" as "there is nothing to protect."
    ///
    /// Backends that implement real enumeration (`DirPersistence`,
    /// `PostgresNodeStore`, `MongoNodeStore`) override this to `true`
    /// alongside `known_room_ids`. `NoPersistence` doesn't need to — it's
    /// never durable, so [`ServerPersistence::is_durable`] already short-
    /// circuits the sweep before this is consulted.
    fn can_enumerate_rooms(&self) -> bool {
        false
    }
}

/// Why [`BlobPersistence::hydrate_blob`] could not produce a blob's bytes.
///
/// The distinction that matters — and the reason this is an enum rather than
/// an `Option` (`blob-cas-remediation.md` slice 2.2, finding #10) — is
/// [`Missing`](Self::Missing) vs [`Unhydratable`](Self::Unhydratable). Before
/// 2.2 both collapsed into `get_blob` → `None`, so an S3-backed server's GC
/// reported "missing tree object <hex>" on every tick: an operator reads that
/// as data loss and goes hunting for an object that is sitting safely in the
/// bucket, while the real cause — this deployment cannot read tree objects —
/// is invisible. Keeping the two apart is the entire "fail loud" half of the
/// slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HydrateError {
    /// The backend has no such object — or holds it corrupt, which reads the
    /// same way by design (existence is not integrity; see 3.2).
    Missing,
    /// The object may exist, but this backend can never deliver its bytes
    /// into the server process. A **configuration** fact about the
    /// deployment, not a fact about the data. Actionable by an operator.
    Unhydratable {
        /// Which backend refused, for the operator reading the log line.
        backend: String,
        /// Why it cannot hydrate, and — where there is one — what to do.
        detail: String,
    },
    /// A transient read/network failure. Retryable; says nothing about
    /// whether the object exists.
    Backend(String),
}

impl std::fmt::Display for HydrateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HydrateError::Missing => write!(f, "blob not present in this store"),
            HydrateError::Unhydratable { backend, detail } => write!(
                f,
                "backend cannot hydrate blob bytes into the server process ({backend}): {detail}"
            ),
            HydrateError::Backend(e) => write!(f, "backend read failed: {e}"),
        }
    }
}

impl std::error::Error for HydrateError {}

/// Why [`BlobPersistence::persist_blob`] could not make the bytes durable.
///
/// This exists so the blob HTTP origin can stop lying about durability
/// (`blob-cas-remediation.md` slice 4.2, finding #8): before it, the trait
/// method returned `()`, every backend logged-and-swallowed its own write
/// failures, and `PUT /blobs/{hash}` answered `201 Created` whether or not
/// anything was durably stored — the client then dropped its only copy.
///
/// The two variants carry the one distinction a caller can act on — *was the
/// failure reported, or did the backend never answer?* — because that is
/// what picks the HTTP status (500 vs 503) and the client's next move
/// (investigate vs retry). Deliberately NOT a mirror of [`HydrateError`]:
/// there is no `Missing` (persist creates), and no `Unhydratable`-style
/// configuration variant — a backend that structurally never persists
/// (S3 Delegate mode, `NoPersistence`) returns `Ok(())`, because "this
/// deployment does not write through this path, by design and documented"
/// is not a failed write. See each implementor for its own mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistBlobError {
    /// The backend answered, and the answer was failure: a local
    /// filesystem error (mkdir/write/rename — disk full, permissions) or a
    /// bucket that replied to the PUT with an error. Nothing is durably
    /// stored under this hash by this call. HTTP-facing callers map this
    /// to `500` — the origin itself is broken, not merely busy.
    Backend(String),
    /// The backend never answered within its bound: an op timeout, a dead
    /// bridge, an unreachable endpoint. Says nothing certain about whether
    /// the write landed (a PUT can time out after the bucket applied it —
    /// content addressing makes the retry idempotent, so callers must
    /// treat this as "not confirmed durable"). Retryable; HTTP-facing
    /// callers map this to `503 Service Unavailable`.
    Unavailable(String),
}

impl std::fmt::Display for PersistBlobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PersistBlobError::Backend(e) => write!(f, "blob write failed: {e}"),
            PersistBlobError::Unavailable(e) => {
                write!(f, "blob backend unavailable (write not confirmed): {e}")
            }
        }
    }
}

impl std::error::Error for PersistBlobError {}

/// Blob-side persistence. See module docs for rationale.
///
/// Blobs are a single global content-addressed pool — no room scoping, no
/// sharding (see `docs/BLOB_STORAGE_LAYOUT.md`). `room_id` still appears on
/// the presign-related methods below because it flows through as metadata
/// to delegate protocols; it plays no role in where/how a blob's bytes are
/// stored.
pub trait BlobPersistence: Send + Sync + std::fmt::Debug {
    /// Point lookup by hash. `None` if not present, or for backends (e.g.
    /// S3) that never hydrate bytes into the server process — the SDK
    /// pulls those lazily via `resolve_get_url`.
    fn get_blob(&self, _hash: &Hash) -> Option<Vec<u8>> {
        None
    }

    /// Persist a single blob, addressed only by its hash.
    ///
    /// ## Contract (slice 4.2, finding #8 — durability truthfulness)
    ///
    /// * `Ok(())` — the bytes are at rest under `hash` as far as this
    ///   backend can confirm (written+renamed on disk, bucket PUT
    ///   acknowledged), **or** this backend structurally never writes
    ///   through this path and documents that at its impl (S3 Delegate
    ///   mode, `NoPersistence`). Idempotent: already-present is `Ok`.
    /// * `Err` — see [`PersistBlobError`]'s per-variant docs. An error here
    ///   means the caller must NOT tell anyone the blob is stored: no
    ///   `201`, no GC-inventory `Active` row, no `blob-available`
    ///   broadcast. Callers may not discard the result silently — if a
    ///   call site genuinely has nothing better than a log line, the log
    ///   is `error!`-level and the site says why in a comment.
    fn persist_blob(&self, hash: &Hash, bytes: &[u8]) -> Result<(), PersistBlobError>;

    /// Cheap existence check — does this store already hold `hash`?
    ///
    /// Used by the blob HTTP origin (S2.1b) to implement idempotent PUT
    /// (already-present → 200, newly stored → 201) without paying for a
    /// full `get_blob` read+hash-verify when the backend can answer more
    /// cheaply (e.g. a file `exists()` check). Default impl falls back to
    /// `get_blob(...).is_some()`, which is correct for every backend even
    /// if not the cheapest.
    fn has_blob(&self, hash: &Hash) -> bool {
        self.get_blob(hash).is_some()
    }

    /// Does [`get_blob`](Self::get_blob) actually return bytes for blobs this
    /// backend holds?
    ///
    /// **A declaration, not an inference** — and that distinction is the
    /// whole reason this method exists. `get_blob` returning `None` for a
    /// present hash has *two* completely different causes:
    ///
    /// * **Policy:** the backend never hydrates bytes into the server process
    ///   at all (`S3BlobStore` — offloading them is the entire point).
    /// * **Integrity:** the bytes are there but corrupt.
    ///   [`DirPersistence::get_blob`] verifies BLAKE3 on read and returns
    ///   `None` on a mismatch while [`has_blob`](Self::has_blob) — a plain
    ///   `is_file()` — still says `true`.
    ///
    /// So "`get_blob` said `None` but `has_blob` said `true`" **cannot** be
    /// used to detect a non-hydrating backend: on `DirPersistence` that exact
    /// signature means "corrupt blob". [`hydrate_blob`](Self::hydrate_blob)'s
    /// default therefore asks this question outright instead of guessing.
    ///
    /// Default `true`: every backend in this workspace except `S3BlobStore`
    /// hydrates. **If you write a backend whose `get_blob` returns `None` as
    /// policy, override this to `false`** — that is what makes the default
    /// `hydrate_blob` fail *loudly* instead of silently reporting every blob
    /// you hold as missing (see `blob-cas-remediation.md` finding #10, and
    /// the `naive_default_only_backend_reports_everything_absent` pin in
    /// `nodalmerge-blobstore-conformance`).
    fn get_blob_hydrates(&self) -> bool {
        true
    }

    /// Fetch a blob's bytes into the server process, distinguishing "not
    /// there" from "this backend structurally cannot give you bytes".
    ///
    /// ## This is not a hole in the non-hydrating policy
    ///
    /// [`get_blob`](Self::get_blob)'s "S3 never hydrates" contract exists to
    /// keep **large file payloads** out of the server process — that is the
    /// entire point of offloading them to object storage, and slice 2.2 did
    /// **not** change it. `hydrate_blob` is for the narrow class of objects
    /// the server must *parse* to do its own job, which are small metadata by
    /// construction. Its first caller, [`crate::tree_walk::walk_tree`], only
    /// ever fetches **tree objects** (a few hundred bytes of JSON): v1 entries
    /// and v2 `"f"` entries are terminal — their hashes go straight into the
    /// live set and their bytes are never read — so only `"d"` entries are
    /// ever fetched. Hydrating those is not "offloading, but worse"; it is the
    /// server reading its own index. Callers that want *file* bytes on an S3
    /// backend still have no business here: they should mint a URL via
    /// [`resolve_get_url`](Self::resolve_get_url) and let the client pull.
    ///
    /// ## Contract
    ///
    /// * `Ok(bytes)` — verified bytes (backends that verify on read keep doing so).
    /// * [`HydrateError::Missing`] — the backend genuinely has no such object,
    ///   *or* holds it corrupt (same answer `get_blob` gives; existence is not
    ///   integrity — see `blob-cas-remediation.md` 3.2).
    /// * [`HydrateError::Unhydratable`] — the object may well exist, but this
    ///   backend can never deliver its bytes here (S3 **Delegate** mode: no
    ///   bucket credentials, and the delegate presign protocol v1 has no
    ///   "give me the bytes" op). This is a **configuration** fact, and callers
    ///   must surface it as such rather than as a lost object.
    /// * [`HydrateError::Backend`] — a transient read/network failure. Retryable.
    ///
    /// The default is correct-by-construction for every hydrating backend and
    /// **loud** for a backend that declares [`get_blob_hydrates`](Self::get_blob_hydrates)
    /// `false` without overriding this — never silently `Missing`.
    fn hydrate_blob(&self, hash: &Hash) -> Result<Vec<u8>, HydrateError> {
        if !self.get_blob_hydrates() {
            return Err(HydrateError::Unhydratable {
                backend: format!("{self:?}"),
                detail: "backend declares get_blob_hydrates() = false and does not override \
                         hydrate_blob(), so it has no way to read bytes into this process"
                    .to_string(),
            });
        }
        self.get_blob(hash).ok_or(HydrateError::Missing)
    }

    /// G4 — two-phase blob GC sweep across the whole store.
    ///
    /// A blob is *live* when its hash is in `live`. Non-live blobs are
    /// tombstoned on the first sweep that sees them; they are deleted on a
    /// subsequent sweep once the tombstone's age exceeds `grace`. A blob
    /// that reappears in `live` after tombstoning has its tombstone
    /// cleared.
    ///
    /// `live` must be the union of every room's referenced blob hashes —
    /// resident and non-resident alike (see
    /// [`NodePersistence::known_room_ids`]) — since a single global pool
    /// has no per-room boundary to protect a cold room's blobs from an
    /// incomplete live set.
    ///
    /// `grace = Duration::ZERO` collapses the two phases. Returns the
    /// number of blobs actually deleted in this call. Default impl is a
    /// no-op (non-durable backends have nothing to GC).
    fn blob_gc_sweep(
        &self,
        _live: &std::collections::HashSet<Hash>,
        _grace: Duration,
    ) -> usize {
        0
    }

    /// F6 — redirect target for blob *downloads*.
    ///
    /// Return `Some(url)` to tell the SDK to fetch the blob directly from
    /// `url` (typically a presigned S3 GET). The server emits a
    /// `blob-redirect` wire message instead of sending the bytes.
    /// Returning `None` falls through to the existing bytes-over-WS path.
    ///
    /// `size_hint` lets backends skip redirect for below-threshold blobs
    /// (the round-trip to mint a URL isn't worth it for a 1 KB icon). If
    /// the backend doesn't know the size, pass `None`; backends that care
    /// about size can still call `load_room_blobs` or consult their own
    /// metadata.
    fn resolve_get_url(
        &self,
        _room_id: &str,
        _hash: &Hash,
        _size_hint: Option<u64>,
    ) -> Option<PresignedUrl> {
        None
    }

    /// F6 — direct-upload target for blob *uploads*.
    ///
    /// Return `Some(url)` to tell the SDK to PUT bytes straight to `url`
    /// (typically a presigned S3 PUT). The SDK then sends `blob-uploaded`
    /// when the PUT completes. Returning `None` falls through to
    /// bytes-over-WS.
    ///
    /// Backends should typically gate this on `size` (e.g. only redirect
    /// for blobs >= 1 MiB) since the presign round-trip costs one request.
    fn resolve_put_url(
        &self,
        _room_id: &str,
        _hash: &Hash,
        _size: u64,
        _content_type: Option<&str>,
    ) -> Option<PresignedUrl> {
        None
    }

    /// F6 — verify a presigned upload completed. Called when the SDK
    /// sends `blob-uploaded`. Backends with server-side visibility (S3
    /// HEAD object) should verify the object exists and matches the
    /// claimed hash/size; backends without should return `Ok(())`.
    ///
    /// Returning `Err` causes the server to reject the `blob-uploaded`
    /// message and ignore the blob; the client falls back to WS.
    fn verify_uploaded(&self, _room_id: &str, _hash: &Hash) -> Result<(), String> {
        Ok(())
    }

    /// `true` if this backend survives process restarts.
    fn blobs_durable(&self) -> bool {
        true
    }

    /// S4.2 — whether this backend is genuinely presign-capable, as opposed
    /// to merely inheriting `resolve_get_url`/`resolve_put_url`/
    /// `verify_uploaded`'s trait defaults (`None`/`None`/`Ok(())`).
    ///
    /// This exists because `verify_uploaded`'s default is `Ok(())` for two
    /// very different reasons that the blob HTTP origin's `POST
    /// /blobs/{hash}/uploaded` (`blob_http.rs`) must tell apart:
    /// * a backend with **no** presign capability at all (`NoPersistence`,
    ///   `DirPersistence`) — the endpoint must answer **501**.
    /// * `nodalmerge-s3-blobs`'s `S3Auth::Delegate` mode, which *is* a real
    ///   presign-capable backend but has no bucket credentials to verify an
    ///   upload with, so it deliberately trusts the client (**200**) per
    ///   `docs/BLOB_HTTP_SURFACE.md`'s "Blob URL resolution" section.
    ///
    /// `resolve_get_url`/`resolve_put_url` don't need an equivalent flag:
    /// their own `None` already means "no URL, fall back" regardless of
    /// backend, so `GET /blobs/{hash}/url` can key off that directly.
    /// Default: `false`.
    fn supports_presigned_urls(&self) -> bool {
        false
    }

    /// S3.1b — return this backend's own stored bytes for `hash` when it
    /// holds them under an *alternate* at-rest encoding (currently only
    /// zstd; see `docs/BLOB_STORAGE_LAYOUT.md` §8), alongside the
    /// `Content-Encoding` token to serve. Used by the blob HTTP origin
    /// (`blob_http.rs`) to serve stored `.zst` bytes as-is — no
    /// decode+recompress round trip — when the client sent `Accept-Encoding:
    /// zstd`. `None` means "no alternate encoding on hand"; the caller falls
    /// back to `get_blob` (identity). Default: no backend supports this.
    fn get_blob_encoded(&self, _hash: &Hash) -> Option<(Vec<u8>, &'static str)> {
        None
    }
}

/// The whole-persistence surface — everything `Rooms` needs. Existing call
/// sites that hold `Arc<dyn ServerPersistence>` are unchanged.
pub trait ServerPersistence: NodePersistence + BlobPersistence {
    /// Combined durability: a room is safe to evict only when both halves
    /// survive a restart. The idle-eviction sweeper reads this; call sites
    /// that care only about one axis can call `nodes_durable()` /
    /// `blobs_durable()` directly.
    fn is_durable(&self) -> bool {
        self.nodes_durable() && self.blobs_durable()
    }

    /// On-disk store root for topology sidecars (promotion proposals). `None`
    /// when the server is fully in-memory.
    fn topology_store_root(&self) -> Option<PathBuf> {
        None
    }
}

impl ServerPersistence for DirPersistence {
    fn topology_store_root(&self) -> Option<PathBuf> {
        Some(self.root().to_path_buf())
    }
}

impl ServerPersistence for NoPersistence {}

impl<N: NodePersistence, B: BlobPersistence> ServerPersistence for Composite<N, B> {}

// ─── Composite<N, B> ─────────────────────────────────────────────────────────

/// Compose a `NodePersistence` and a `BlobPersistence` into one
/// `ServerPersistence`. Lets you wire mixed backends without writing a
/// bespoke struct:
///
/// ```ignore
/// let store = Composite::new(MongoNodeStore::connect(uri)?, S3BlobStore::new(cfg)?);
/// let rooms = Rooms::with_persistence(Arc::new(store));
/// ```
#[derive(Debug)]
pub struct Composite<N, B> {
    pub nodes: N,
    pub blobs: B,
}

impl<N, B> Composite<N, B> {
    pub fn new(nodes: N, blobs: B) -> Self {
        Self { nodes, blobs }
    }
}

impl<N: NodePersistence, B: BlobPersistence> NodePersistence for Composite<N, B> {
    fn load_room_nodes(&self, room_id: &str) -> Vec<SyncNode> {
        self.nodes.load_room_nodes(room_id)
    }
    fn persist_node(&self, room_id: &str, node: &SyncNode) {
        self.nodes.persist_node(room_id, node)
    }
    fn persist_nodes(&self, room_id: &str, nodes: &[&SyncNode]) {
        self.nodes.persist_nodes(room_id, nodes)
    }
    fn nodes_durable(&self) -> bool {
        self.nodes.nodes_durable()
    }
    fn known_room_ids(&self) -> Vec<String> {
        self.nodes.known_room_ids()
    }
    fn can_enumerate_rooms(&self) -> bool {
        self.nodes.can_enumerate_rooms()
    }
}

impl<N: NodePersistence, B: BlobPersistence> BlobPersistence for Composite<N, B> {
    fn get_blob(&self, hash: &Hash) -> Option<Vec<u8>> {
        self.blobs.get_blob(hash)
    }
    fn has_blob(&self, hash: &Hash) -> bool {
        self.blobs.has_blob(hash)
    }
    /// Must forward. Inheriting the trait default here would be the
    /// `RemoteBlobLinkAggregator` hole (slice 2.1) all over again: for the
    /// production `Composite<Postgres, S3BlobStore>` wiring the default would
    /// answer `true`, then call `get_blob` (correctly `None` on S3) and report
    /// every tree object **Missing** — silently reinstating finding #10 while
    /// `S3BlobStore`'s own override sat right there, unused and untested.
    /// Pinned by `composite_forwards_hydration_seam_to_the_blob_half`.
    fn get_blob_hydrates(&self) -> bool {
        self.blobs.get_blob_hydrates()
    }
    /// Must forward — see [`Self::get_blob_hydrates`].
    fn hydrate_blob(&self, hash: &Hash) -> Result<Vec<u8>, HydrateError> {
        self.blobs.hydrate_blob(hash)
    }
    fn persist_blob(&self, hash: &Hash, bytes: &[u8]) -> Result<(), PersistBlobError> {
        self.blobs.persist_blob(hash, bytes)
    }
    fn blob_gc_sweep(
        &self,
        live: &std::collections::HashSet<Hash>,
        grace: Duration,
    ) -> usize {
        self.blobs.blob_gc_sweep(live, grace)
    }
    fn resolve_get_url(
        &self,
        room_id: &str,
        hash: &Hash,
        size_hint: Option<u64>,
    ) -> Option<PresignedUrl> {
        self.blobs.resolve_get_url(room_id, hash, size_hint)
    }
    fn resolve_put_url(
        &self,
        room_id: &str,
        hash: &Hash,
        size: u64,
        content_type: Option<&str>,
    ) -> Option<PresignedUrl> {
        self.blobs
            .resolve_put_url(room_id, hash, size, content_type)
    }
    fn verify_uploaded(&self, room_id: &str, hash: &Hash) -> Result<(), String> {
        self.blobs.verify_uploaded(room_id, hash)
    }
    fn blobs_durable(&self) -> bool {
        self.blobs.blobs_durable()
    }
    fn supports_presigned_urls(&self) -> bool {
        self.blobs.supports_presigned_urls()
    }
    fn get_blob_encoded(&self, hash: &Hash) -> Option<(Vec<u8>, &'static str)> {
        self.blobs.get_blob_encoded(hash)
    }
}

// ─── NoPersistence ──────────────────────────────────────────────────────────

/// In-memory only. The default — matches pre-F4 behavior.
#[derive(Debug, Default)]
pub struct NoPersistence;

impl NodePersistence for NoPersistence {
    fn load_room_nodes(&self, _room_id: &str) -> Vec<SyncNode> {
        Vec::new()
    }
    fn persist_node(&self, _room_id: &str, _node: &SyncNode) {}
    fn nodes_durable(&self) -> bool {
        false
    }
}

impl BlobPersistence for NoPersistence {
    /// `Ok` on purpose (4.2): in-memory-only is this backend's documented
    /// deployment shape, not a failed write — `blobs_durable()` below is
    /// already the honest "nothing here survives a restart" signal, and a
    /// PUT against a memory-only server has always meant exactly that.
    fn persist_blob(&self, _hash: &Hash, _bytes: &[u8]) -> Result<(), PersistBlobError> {
        Ok(())
    }
    fn blobs_durable(&self) -> bool {
        false
    }
}

// ─── DirPersistence ─────────────────────────────────────────────────────────

/// S3.1b — at-rest content-encoding config for `DirPersistence`, per
/// `docs/BLOB_STORAGE_LAYOUT.md` §8. Only affects *writes*: readers
/// (`get_blob`, `has_blob`, `blob_gc_sweep`) are always encoding-aware
/// regardless of this config, so a store written with compression on
/// remains fully readable by one opened with it off (and vice versa).
///
/// Default mirrors the doc's peer-local-cache guidance (compression off);
/// callers that want the server-side-durable-store default (on, level 3)
/// construct this explicitly — see `main.rs`'s `--blob-compression` wiring.
#[derive(Debug, Clone, Copy)]
pub struct BlobCompressionConfig {
    /// Compress eligible blobs on write. `false` = always write identity
    /// (byte-for-byte pre-v3 behavior).
    pub enabled: bool,
    /// zstd compression level passed to the encoder.
    pub level: i32,
    /// Blobs smaller than this are never compressed (guidance in §8).
    pub min_bytes: usize,
}

impl Default for BlobCompressionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            level: 3,
            min_bytes: 4096,
        }
    }
}

/// SQLite (nodes) + filesystem (blobs), all rooted at a single directory.
///
/// Layout (v3 — see `docs/BLOB_STORAGE_LAYOUT.md`):
///
/// ```text
/// <root>/
///   nodalmerge.db                      ← SQLite: one row per (room, node)
///   topology-promotions.db             ← SQLite: promotion proposals (Wave 3)
///   blobs/
///     blake3/
///       <hash_hex>                     ← identity encoding, global CAS pool
///       <hash_hex>.zst                 ← optional zstd encoding (§8); at most
///                                          one of the two forms per hash
///     .tombstones/
///       blake3/
///         <hash_hex>                   ← empty marker; mtime = tombstone time
///     .layout-v2                       ← empty marker; presence = migrated
/// ```
///
/// Blobs are content-addressed only — no room id anywhere in the path.
/// `open()` auto-migrates a legacy (pre-v2, per-room-directory) layout the
/// first time it sees one; see [`migrate_legacy_blob_layout`].
#[derive(Debug)]
pub struct DirPersistence {
    root: PathBuf,
    conn: Mutex<Connection>,
    compression: BlobCompressionConfig,
}

impl DirPersistence {
    /// On-disk store root (`nodalmerge.db`, `blobs/`, topology sidecars).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Open (or create) the store rooted at `root`, with at-rest blob
    /// compression disabled (byte-for-byte pre-v3 write behavior). Creates
    /// the directory, opens the SQLite file, ensures the schema, and
    /// migrates a legacy blob layout to v2 if one is found.
    pub fn open(root: impl AsRef<Path>) -> std::io::Result<Self> {
        Self::open_with_compression(root, BlobCompressionConfig::default())
    }

    /// Same as [`Self::open`], but with an explicit
    /// [`BlobCompressionConfig`] governing whether newly written blobs are
    /// zstd-encoded at rest (§8). Reading is unaffected by this config —
    /// both encodings are always recognized on read.
    pub fn open_with_compression(
        root: impl AsRef<Path>,
        compression: BlobCompressionConfig,
    ) -> std::io::Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let blobs_root = root.join("blobs");
        std::fs::create_dir_all(&blobs_root)?;
        std::fs::create_dir_all(blobs_root.join("blake3"))?;
        migrate_legacy_blob_layout(&blobs_root);
        let db_path = root.join("nodalmerge.db");
        let conn = Connection::open(&db_path)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        // WAL keeps writers from blocking readers and survives crashes.
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS nodes (
               room_id TEXT NOT NULL,
               node_id BLOB NOT NULL,
               bytes   BLOB NOT NULL,
               seq     INTEGER PRIMARY KEY AUTOINCREMENT,
               UNIQUE(room_id, node_id)
             );
             CREATE INDEX IF NOT EXISTS idx_nodes_room ON nodes(room_id, seq);",
        )
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        Ok(Self {
            root,
            conn: Mutex::new(conn),
            compression,
        })
    }

    fn blake3_dir(&self) -> PathBuf {
        self.root.join("blobs").join("blake3")
    }

    fn blob_path(&self, hash: &Hash) -> PathBuf {
        self.root.join("blobs").join(blob_relative_path(hash))
    }

    /// S3.1b — path of the zstd-encoded form (§8), independent of whether
    /// it currently exists.
    fn blob_encoded_path(&self, hash: &Hash) -> PathBuf {
        self.root
            .join("blobs")
            .join(blob_relative_encoded_path(hash))
    }

    fn tombstones_dir(&self) -> PathBuf {
        self.root.join("blobs").join(".tombstones").join("blake3")
    }
}

impl NodePersistence for DirPersistence {
    fn load_room_nodes(&self, room_id: &str) -> Vec<SyncNode> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            match conn.prepare("SELECT bytes FROM nodes WHERE room_id = ?1 ORDER BY seq ASC") {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(?e, "prepare load_room_nodes failed");
                    return Vec::new();
                }
            };
        let rows = stmt.query_map(params![room_id], |r| r.get::<_, Vec<u8>>(0));
        let mut out = Vec::new();
        if let Ok(iter) = rows {
            for row in iter.flatten() {
                match unpack_nodes(&row) {
                    Ok(mut ns) if ns.len() == 1 => out.push(ns.pop().unwrap()),
                    Ok(_) => tracing::warn!("persisted row held != 1 nodes, skipping"),
                    Err(e) => tracing::warn!(?e, "unpack persisted node failed"),
                }
            }
        }
        out
    }

    fn persist_node(&self, room_id: &str, node: &SyncNode) {
        let t0 = Instant::now();
        let bytes = pack_nodes(&[node]);
        let conn = self.conn.lock().unwrap();
        if let Err(e) = conn.execute(
            "INSERT OR IGNORE INTO nodes (room_id, node_id, bytes) VALUES (?1, ?2, ?3)",
            params![room_id, &node.id.as_bytes()[..], bytes],
        ) {
            tracing::warn!(?e, "persist_node failed");
        }
        drop(conn);
        let elapsed = t0.elapsed().as_secs_f64();
        metrics::histogram!("nodalmerge_persistence_write_seconds", "kind" => "node")
            .record(elapsed);
    }

    fn persist_nodes(&self, room_id: &str, nodes: &[&SyncNode]) {
        if nodes.is_empty() {
            return;
        }
        let t0 = Instant::now();
        // Pre-encode outside the lock so we hold the connection mutex for the
        // minimum possible time. Each row is still one postcard pack, same as
        // persist_node — the win is collapsing N autocommits into one txn.
        let encoded: Vec<(Vec<u8>, Vec<u8>)> = nodes
            .iter()
            .map(|n| (n.id.as_bytes().to_vec(), pack_nodes(&[*n])))
            .collect();

        // Try the batched transactional path. If `begin` fails, fall back to
        // per-row autocommit on a fresh lock so the Err's lifetime-tied
        // `Transaction` drops before we reborrow `conn`.
        let txn_begin_failed: bool;
        {
            let mut conn = self.conn.lock().unwrap();
            match conn.transaction() {
                Ok(tx) => {
                    txn_begin_failed = false;
                    {
                        let mut stmt = match tx.prepare_cached(
                            "INSERT OR IGNORE INTO nodes (room_id, node_id, bytes) VALUES (?1, ?2, ?3)",
                        ) {
                            Ok(s) => s,
                            Err(e) => {
                                tracing::warn!(?e, "persist_nodes: prepare failed");
                                return;
                            }
                        };
                        for (node_id, bytes) in &encoded {
                            if let Err(e) = stmt.execute(params![room_id, node_id, bytes]) {
                                tracing::warn!(?e, "persist_nodes: insert failed");
                            }
                        }
                    }
                    if let Err(e) = tx.commit() {
                        tracing::warn!(?e, "persist_nodes: commit failed");
                    }
                }
                Err(e) => {
                    tracing::warn!(%e, "persist_nodes: begin txn failed; falling back per-row");
                    txn_begin_failed = true;
                }
            };
        }
        if txn_begin_failed {
            let conn = self.conn.lock().unwrap();
            for (node_id, bytes) in &encoded {
                if let Err(e) = conn.execute(
                    "INSERT OR IGNORE INTO nodes (room_id, node_id, bytes) VALUES (?1, ?2, ?3)",
                    params![room_id, node_id, bytes],
                ) {
                    tracing::warn!(?e, "persist_nodes fallback: insert failed");
                }
            }
        }
        let elapsed = t0.elapsed().as_secs_f64();
        metrics::histogram!("nodalmerge_persistence_write_seconds", "kind" => "nodes_batch")
            .record(elapsed);
    }

    fn known_room_ids(&self) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare("SELECT DISTINCT room_id FROM nodes") {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(?e, "prepare known_room_ids failed");
                return Vec::new();
            }
        };
        let rows = stmt.query_map([], |r| r.get::<_, String>(0));
        match rows {
            Ok(iter) => iter.flatten().collect(),
            Err(e) => {
                tracing::warn!(?e, "known_room_ids query failed");
                Vec::new()
            }
        }
    }

    fn can_enumerate_rooms(&self) -> bool {
        true
    }
}

impl BlobPersistence for DirPersistence {
    fn get_blob(&self, hash: &Hash) -> Option<Vec<u8>> {
        // Identity path first — existing verify-on-read logic untouched.
        let path = self.blob_path(hash);
        if let Ok(bytes) = std::fs::read(&path) {
            let actual = Hash::of(&bytes);
            return if actual == *hash {
                Some(bytes)
            } else {
                tracing::warn!(?path, "blob file hash mismatch, skipping");
                None
            };
        }

        // S3.1b: identity absent — fall back to the zstd-encoded form (§8).
        // Both files existing is a writer bug we never produce, but if it
        // happens the identity branch above already returned, so this is
        // also where "identity wins" falls out for free.
        let encoded_path = self.blob_encoded_path(hash);
        let compressed = std::fs::read(&encoded_path).ok()?;
        let bytes = match zstd::stream::decode_all(&compressed[..]) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(?e, ?encoded_path, "zstd blob decode failed, skipping");
                return None;
            }
        };
        // Invariant (§8): the hash is always Blake3 of the DECOMPRESSED bytes.
        let actual = Hash::of(&bytes);
        if actual == *hash {
            Some(bytes)
        } else {
            tracing::warn!(?encoded_path, "zstd blob decompressed hash mismatch, skipping");
            None
        }
    }

    fn has_blob(&self, hash: &Hash) -> bool {
        self.blob_path(hash).is_file() || self.blob_encoded_path(hash).is_file()
    }

    fn get_blob_encoded(&self, hash: &Hash) -> Option<(Vec<u8>, &'static str)> {
        // Raw file bytes, no decode — the HTTP layer serves these as-is
        // under `Content-Encoding: zstd`. No existence/verify duplication
        // with `get_blob`: a corrupt frame here just fails to decode on a
        // later identity-less `get_blob` call, which already warns+None's.
        let bytes = std::fs::read(self.blob_encoded_path(hash)).ok()?;
        Some((bytes, "zstd"))
    }

    /// Every filesystem failure here is [`PersistBlobError::Backend`] (4.2):
    /// a local disk that errors is a broken origin, not a busy one — there
    /// is no timeout class on `std::fs`. The zstd branch keeps its 3.1b
    /// fall-back-to-identity behavior (a failed *compression* is not a
    /// failed *persist* while the identity write can still succeed); only
    /// the identity write's own failure is terminal.
    fn persist_blob(&self, hash: &Hash, bytes: &[u8]) -> Result<(), PersistBlobError> {
        let t0 = Instant::now();
        let dir = self.blake3_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(?e, "persist_blob: create_dir_all failed");
            return Err(PersistBlobError::Backend(format!(
                "create_dir_all {dir:?}: {e}"
            )));
        }
        let identity_path = self.blob_path(hash);
        let encoded_path = self.blob_encoded_path(hash);
        // No-op if either encoding already exists (matches the pre-v3
        // idempotent-persist contract, extended to both forms).
        if identity_path.exists() || encoded_path.exists() {
            return Ok(());
        }

        if self.compression.enabled && should_compress_blob(bytes, &self.compression) {
            match zstd::stream::encode_all(bytes, self.compression.level) {
                Ok(compressed) => {
                    let tmp = dir.join(format!("{}.zst.tmp", hash.to_hex()));
                    let wrote = std::fs::write(&tmp, &compressed).is_ok();
                    if wrote && std::fs::rename(&tmp, &encoded_path).is_ok() {
                        let elapsed = t0.elapsed().as_secs_f64();
                        metrics::histogram!("nodalmerge_persistence_write_seconds", "kind" => "blob")
                            .record(elapsed);
                        return Ok(());
                    }
                    // Encode/write/rename failed partway — clean up the temp
                    // file and fall through to an identity write below so
                    // the blob is never silently dropped.
                    let _ = std::fs::remove_file(&tmp);
                    tracing::warn!("persist_blob: zstd write failed, falling back to identity");
                }
                Err(e) => {
                    tracing::warn!(?e, "persist_blob: zstd encode failed, falling back to identity");
                }
            }
        }

        // Write+rename = atomic on POSIX; on Windows it's a best-effort replace.
        let tmp = dir.join(format!("{}.tmp", hash.to_hex()));
        if let Err(e) = std::fs::write(&tmp, bytes) {
            tracing::warn!(?e, "persist_blob: write tmp failed");
            return Err(PersistBlobError::Backend(format!(
                "write tmp for {}: {e}",
                hash.to_hex()
            )));
        }
        if let Err(e) = std::fs::rename(&tmp, &identity_path) {
            tracing::warn!(?e, "persist_blob: rename failed");
            let _ = std::fs::remove_file(&tmp);
            return Err(PersistBlobError::Backend(format!(
                "rename into place for {}: {e}",
                hash.to_hex()
            )));
        }
        let elapsed = t0.elapsed().as_secs_f64();
        metrics::histogram!("nodalmerge_persistence_write_seconds", "kind" => "blob")
            .record(elapsed);
        Ok(())
    }

    fn blob_gc_sweep(&self, live: &std::collections::HashSet<Hash>, grace: Duration) -> usize {
        let blobs_dir = self.blake3_dir();
        let tombs_dir = self.tombstones_dir();
        let Ok(rd) = std::fs::read_dir(&blobs_dir) else {
            return 0;
        };

        let now = SystemTime::now();
        let mut deleted = 0usize;
        for entry in rd.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            // Skip stray `.tmp` writes from a crashed persist_blob (matches
            // both `<hex>.tmp` and `<hex>.zst.tmp`).
            if name.ends_with(".tmp") {
                continue;
            }
            let Some((hash, _encoding)) = parse_blob_entry_name(name) else {
                // Foreign entry under blake3/ (not a recognized identity or
                // v3 `.zst` name) — never touched by GC. See
                // docs/BLOB_STORAGE_LAYOUT.md §3, §8.
                continue;
            };
            // Tombstones are always keyed by bare hex (§8), regardless of
            // which encoding this entry is — one tombstone covers both.
            let tomb_path = tombs_dir.join(hash.to_hex());

            if live.contains(&hash) {
                // Blob is referenced: clear any leftover tombstone so a brief
                // unreference-then-rereference (e.g. a concurrent SetBlob
                // arriving between sweeps) doesn't doom the blob next round.
                if tomb_path.exists() {
                    let _ = std::fs::remove_file(&tomb_path);
                }
                continue;
            }

            // Not live — consult tombstone.
            match std::fs::metadata(&tomb_path) {
                Ok(md) => {
                    let aged = md
                        .modified()
                        .ok()
                        .and_then(|t| now.duration_since(t).ok())
                        .map(|age| age >= grace)
                        .unwrap_or(false);
                    if aged {
                        if let Err(e) = std::fs::remove_file(&path) {
                            tracing::warn!(?e, blob = %name, "blob_gc_sweep: delete blob failed");
                            continue;
                        }
                        let _ = std::fs::remove_file(&tomb_path);
                        deleted += 1;
                    }
                }
                Err(_) => {
                    // No tombstone yet — create one. `grace == ZERO`
                    // immediately re-checks and deletes in the same pass.
                    if let Err(e) = std::fs::create_dir_all(&tombs_dir) {
                        tracing::warn!(?e, "blob_gc_sweep: mkdir tombstones failed");
                        continue;
                    }
                    if let Err(e) = std::fs::File::create(&tomb_path) {
                        tracing::warn!(?e, "blob_gc_sweep: create tombstone failed");
                        continue;
                    }
                    if grace.is_zero() {
                        if let Err(e) = std::fs::remove_file(&path) {
                            tracing::warn!(?e, blob = %name, "blob_gc_sweep: immediate delete failed");
                            continue;
                        }
                        let _ = std::fs::remove_file(&tomb_path);
                        deleted += 1;
                    }
                }
            }
        }
        deleted
    }
}

/// Auto-migrate a legacy (pre-v2, per-room-directory) blob layout to the
/// canonical global layout, guarded by a `.layout-v2` marker so this scan
/// runs at most once. See `docs/BLOB_STORAGE_LAYOUT.md` §6.
///
/// Idempotent and crash-safe: the marker is written last — and, since
/// slice 5.1, only after a fully clean pass. Any entry the scan cannot
/// process (unreadable dir, non-Unicode name, mkdir/copy failure, blocked
/// quarantine) is logged and *deferred*: the marker stays absent, so the
/// next startup (`open()` runs once per process, from `main`) re-runs the
/// whole scan and picks up the remainder. Writing the marker over a
/// partial pass would permanently orphan the leftovers, because readers
/// only consult `blobs/blake3/` once it exists. The re-run is safe over
/// already-migrated entries: a file found at its destination is the
/// cross-room dedup — verified before the legacy source is dropped, so a
/// partial destination left by an interrupted copy is repaired from the
/// source rather than adopted. Anything that fails hash verification or
/// doesn't parse as a legacy blob filename is quarantined into
/// `.migration-skipped/` rather than deleted; a *successful* quarantine
/// is a terminal disposition and does not defer the marker.
fn migrate_legacy_blob_layout(blobs_root: &Path) {
    let marker = blobs_root.join(".layout-v2");
    if marker.exists() {
        return;
    }

    let blake3_dir = blobs_root.join("blake3");
    let skipped_dir = blobs_root.join(".migration-skipped");
    let mut migrated = 0usize;
    let mut skipped = 0usize;
    // Slice 5.1: entries/rooms this pass could not process. Any deferral
    // leaves the marker absent so the next startup retries.
    let mut deferred = 0usize;

    match std::fs::read_dir(blobs_root) {
        Err(e) => {
            deferred += 1;
            tracing::warn!(path = ?blobs_root, error = %e, "migrate_legacy_blob_layout: cannot enumerate blobs root; deferring migration");
        }
        Ok(rd) => {
            for entry in rd {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(e) => {
                        deferred += 1;
                        tracing::warn!(path = ?blobs_root, error = %e, "migrate_legacy_blob_layout: unreadable blobs-root entry; deferring");
                        continue;
                    }
                };
                let path = entry.path();
                let file_type = match entry.file_type() {
                    Ok(t) => t,
                    Err(e) => {
                        deferred += 1;
                        tracing::warn!(?path, error = %e, "migrate_legacy_blob_layout: cannot stat entry; deferring");
                        continue;
                    }
                };
                if !file_type.is_dir() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                    // A non-Unicode dir name can't be processed (or even
                    // encoded into the quarantine naming scheme) — defer
                    // rather than silently orphan the room's blobs.
                    deferred += 1;
                    tracing::warn!(?path, "migrate_legacy_blob_layout: non-Unicode legacy room dir name; deferring");
                    continue;
                };
                // Skip the canonical/reserved subtrees — everything else under
                // blobs/ is presumed to be a legacy sanitized-room-id directory.
                if name == "blake3" || name == ".tombstones" || name == ".migration-skipped" {
                    continue;
                }

                let room_rd = match std::fs::read_dir(&path) {
                    Ok(rd) => rd,
                    Err(e) => {
                        deferred += 1;
                        tracing::warn!(?path, error = %e, "migrate_legacy_blob_layout: cannot enumerate legacy room dir; deferring");
                        continue;
                    }
                };
                for room_entry in room_rd {
                    let room_entry = match room_entry {
                        Ok(room_entry) => room_entry,
                        Err(e) => {
                            deferred += 1;
                            tracing::warn!(?path, error = %e, "migrate_legacy_blob_layout: unreadable room dir entry; deferring");
                            continue;
                        }
                    };
                    let room_path = room_entry.path();
                    if !room_path.is_file() {
                        // Legacy rooms only ever held plain files; a nested
                        // dir/symlink is foreign, and leaving it behind under
                        // a written marker would strand whatever it holds —
                        // defer until an operator clears it.
                        deferred += 1;
                        tracing::warn!(?room_path, "migrate_legacy_blob_layout: unexpected non-file entry in legacy room dir; deferring");
                        continue;
                    }
                    let Some(file_name) = room_path.file_name().and_then(|s| s.to_str()) else {
                        deferred += 1;
                        tracing::warn!(?room_path, "migrate_legacy_blob_layout: non-Unicode blob file name; deferring");
                        continue;
                    };
                    if file_name.ends_with(".tmp") {
                        continue;
                    }

                    // Returns whether the file actually landed in quarantine:
                    // a blocked quarantine (skipped-dir unmakeable, rename
                    // failed) leaves the entry in the legacy dir, and that
                    // must defer the marker like any other skip.
                    let quarantine = |reason: &str| -> bool {
                        tracing::warn!(?room_path, reason, "migrate_legacy_blob_layout: quarantining file");
                        if std::fs::create_dir_all(&skipped_dir).is_err() {
                            return false;
                        }
                        let dest = skipped_dir.join(format!("{name}__{file_name}"));
                        std::fs::rename(&room_path, &dest).is_ok()
                    };
                    let quarantine_or_defer = |reason: &str, skipped: &mut usize, deferred: &mut usize| {
                        if quarantine(reason) {
                            *skipped += 1;
                        } else {
                            *deferred += 1;
                            tracing::warn!(?room_path, "migrate_legacy_blob_layout: quarantine failed; deferring");
                        }
                    };

                    let Some(hash) = hash_from_hex(file_name) else {
                        quarantine_or_defer("filename is not 64 lowercase hex chars", &mut skipped, &mut deferred);
                        continue;
                    };
                    let Ok(bytes) = std::fs::read(&room_path) else {
                        quarantine_or_defer("read failed", &mut skipped, &mut deferred);
                        continue;
                    };
                    if Hash::of(&bytes) != hash {
                        quarantine_or_defer("content hash mismatch", &mut skipped, &mut deferred);
                        continue;
                    }

                    if let Err(e) = std::fs::create_dir_all(&blake3_dir) {
                        deferred += 1;
                        tracing::warn!(path = ?blake3_dir, error = %e, "migrate_legacy_blob_layout: cannot create blake3 dir; deferring");
                        continue;
                    }
                    let dest = blake3_dir.join(hash.to_hex());
                    if dest.exists() {
                        // Already present — this collision IS the cross-room
                        // dedup the v2 layout is for — but verify the dest
                        // really holds these bytes before dropping the
                        // duplicate: an interrupted copy fallback on an
                        // earlier marker-less pass leaves a partial file
                        // here, and deleting the legacy source on the
                        // strength of a partial copy would destroy the only
                        // good copy. (Slice 5.1.)
                        let dest_ok = std::fs::read(&dest)
                            .map(|b| Hash::of(&b) == hash)
                            .unwrap_or(false);
                        if !dest_ok {
                            if std::fs::write(&dest, &bytes).is_err() {
                                deferred += 1;
                                tracing::warn!(?dest, "migrate_legacy_blob_layout: cannot repair partial destination; deferring");
                                // Don't leave a half-repaired dest for the
                                // next pass to mistake for the real thing.
                                let _ = std::fs::remove_file(&dest);
                                continue;
                            }
                            tracing::warn!(?dest, "migrate_legacy_blob_layout: repaired partial/corrupt destination from legacy source");
                        }
                        let _ = std::fs::remove_file(&room_path);
                    } else if std::fs::rename(&room_path, &dest).is_err() {
                        // Cross-device rename can fail; fall back to copy+remove.
                        if std::fs::write(&dest, &bytes).is_ok() {
                            let _ = std::fs::remove_file(&room_path);
                        } else {
                            deferred += 1;
                            tracing::warn!(?room_path, ?dest, "migrate_legacy_blob_layout: copy fallback failed; deferring");
                            // A failed write can leave a partial dest —
                            // remove it so no later pass adopts it.
                            let _ = std::fs::remove_file(&dest);
                            continue;
                        }
                    }
                    migrated += 1;
                }
                // Best-effort cleanup of the now-empty legacy room directory.
                let _ = std::fs::remove_dir(&path);
            }
        }
    }

    // Legacy tombstones lived in a sibling `blob-tombstones/` tree (one
    // level up from `blobs/`, not under it) — discard entirely per the v2
    // contract. Worst case: a blob that should already be tombstoned
    // survives until the next GC pass notices it's unreferenced again.
    if let Some(store_root) = blobs_root.parent() {
        let _ = std::fs::remove_dir_all(store_root.join("blob-tombstones"));
    }

    if migrated > 0 || skipped > 0 || deferred > 0 {
        tracing::info!(migrated, skipped, deferred, "migrated legacy blob layout to v2");
    }
    if deferred > 0 {
        tracing::warn!(deferred, "migrate_legacy_blob_layout: incomplete pass; leaving .layout-v2 absent so the next startup retries");
        return;
    }
    let _ = std::fs::write(&marker, b"");
}

/// Parse a filename back into a [`Hash`] — strictly lowercase, exactly 64
/// hex chars. Uppercase is deliberately rejected (not just normalized):
/// per `docs/BLOB_STORAGE_LAYOUT.md` §3 an uppercase name is *foreign*, to
/// be silently skipped by readers/GC, never adopted. (This used to accept
/// uppercase too — a latent divergence from the .NET side's equivalent
/// check, caught while writing the cross-runtime layout vectors.)
pub fn hash_from_hex(s: &str) -> Option<Hash> {
    if !is_canonical_blob_name(s) {
        return None;
    }
    let mut out = [0u8; 32];
    let bytes = s.as_bytes();
    for i in 0..32 {
        let hi = hex_digit(bytes[2 * i])?;
        let lo = hex_digit(bytes[2 * i + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Some(Hash(out))
}

/// Whether `name` is exactly 64 lowercase hex characters — the canonical
/// on-disk blob/tombstone filename shape. Anything else under `blake3/`
/// is foreign per `docs/BLOB_STORAGE_LAYOUT.md` §3.
pub fn is_canonical_blob_name(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// Canonical relative blob path (`blake3/<hex>`), independent of any store
/// root. Pure/testable formula shared by [`DirPersistence`] and the
/// cross-runtime layout parity vectors — see
/// `docs/BLOB_STORAGE_LAYOUT.md` §2.
pub fn blob_relative_path(hash: &Hash) -> PathBuf {
    Path::new("blake3").join(hash.to_hex())
}

/// Canonical relative tombstone path (`.tombstones/blake3/<hex>`),
/// independent of any store root. See `docs/BLOB_STORAGE_LAYOUT.md` §2.
pub fn tombstone_relative_path(hash: &Hash) -> PathBuf {
    Path::new(".tombstones").join("blake3").join(hash.to_hex())
}

/// S3.1b — canonical relative path of the zstd-encoded form
/// (`blake3/<hex>.zst`), independent of any store root. Pure/testable
/// formula shared by [`DirPersistence`] and the cross-runtime layout
/// parity vectors — see `docs/BLOB_STORAGE_LAYOUT.md` §8.
pub fn blob_relative_encoded_path(hash: &Hash) -> PathBuf {
    Path::new("blake3").join(format!("{}.zst", hash.to_hex()))
}

/// S3.1b — which at-rest encoding a `blake3/` entry name represents. See
/// `docs/BLOB_STORAGE_LAYOUT.md` §8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobEncoding {
    /// Bare `<hex>` — raw bytes, `Blake3(bytes) == hex`.
    Identity,
    /// `<hex>.zst` — single zstd frame, `Blake3(decompressed bytes) == hex`.
    Zstd,
}

/// Parse a `blake3/` entry filename into its hash and encoding. Recognizes
/// exactly two canonical v3 shapes: the bare 64-lowercase-hex identity form
/// and `<64-lowercase-hex>.zst`. Anything else — wrong length, uppercase,
/// non-hex, an unrecognized suffix (`.gz`), or a doubled suffix
/// (`.zst.zst`) — is foreign: `None`, to be left alone by readers and GC
/// alike (§3, §8). This is the v3 sibling of [`is_canonical_blob_name`],
/// used by [`DirPersistence::blob_gc_sweep`] and the layout parity vectors
/// (`name_conformance_vectors_v3`).
pub fn parse_blob_entry_name(name: &str) -> Option<(Hash, BlobEncoding)> {
    if let Some(stem) = name.strip_suffix(".zst") {
        hash_from_hex(stem).map(|h| (h, BlobEncoding::Zstd))
    } else {
        hash_from_hex(name).map(|h| (h, BlobEncoding::Identity))
    }
}

/// S3.1b — whether `name` is canonical under the v3 naming rules (§3
/// amended by §8): either the bare identity form or `<hex>.zst`. Thin
/// wrapper over [`parse_blob_entry_name`] for callers that only care about
/// the yes/no answer (e.g. `name_conformance_vectors_v3`).
pub fn is_canonical_blob_name_v3(name: &str) -> bool {
    parse_blob_entry_name(name).is_some()
}

/// S3.1b — recommended skip-compress heuristic from
/// `docs/BLOB_STORAGE_LAYOUT.md` §8: never compress below `min_bytes`;
/// otherwise sample-compress the first `min(64 KiB, len)` bytes and skip
/// (store raw) if the sampled ratio is > 0.98. Content-type-based skipping
/// (declared compressed media types) is guidance for callers that *have* a
/// content type (the .NET/S3 side) — this seam has none, so it isn't
/// implemented here.
fn should_compress_blob(bytes: &[u8], cfg: &BlobCompressionConfig) -> bool {
    if bytes.len() < cfg.min_bytes {
        return false;
    }
    let sample_len = bytes.len().min(64 * 1024);
    let sample = &bytes[..sample_len];
    match zstd::stream::encode_all(sample, cfg.level) {
        Ok(compressed_sample) => {
            let ratio = compressed_sample.len() as f64 / sample.len() as f64;
            ratio <= 0.98
        }
        // Sample-compression failure — don't gamble on the full blob either.
        Err(_) => false,
    }
}

/// Boxed handle used by `Rooms` — one instance is shared across all rooms.
pub type SharedPersistence = Arc<dyn ServerPersistence>;

/// Topology sidecar root derived from the active persistence backend.
pub fn topology_store_root(persistence: &SharedPersistence) -> Option<PathBuf> {
    persistence.topology_store_root()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use nodalmerge_core::{MapOp, Op, StateGraph};

    fn tmpdir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("nodalmerge-test-{nanos}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn node_roundtrip() {
        let dir = tmpdir();
        let store = DirPersistence::open(&dir).unwrap();
        // Build one signed node via StateGraph::apply_local.
        let sk = SigningKey::from_bytes(&[0x11u8; 32]);
        let mut g = StateGraph::new();
        let id = g
            .apply_local(
                &sk,
                0,
                vec![Op::Map(MapOp::Set {
                    key: "k".into(),
                    value: b"v".to_vec(),
                })],
            )
            .unwrap();
        let node = g.get_nodes(&[id]).into_iter().next().unwrap().clone();
        store.persist_node("room-x", &node);
        let loaded = store.load_room_nodes("room-x");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, node.id);
        // Different room returns nothing.
        assert!(store.load_room_nodes("other").is_empty());
    }

    #[test]
    fn idempotent_persist() {
        let dir = tmpdir();
        let store = DirPersistence::open(&dir).unwrap();
        let sk = SigningKey::from_bytes(&[0x22u8; 32]);
        let mut g = StateGraph::new();
        let id = g
            .apply_local(
                &sk,
                0,
                vec![Op::Map(MapOp::Set {
                    key: "k".into(),
                    value: b"v".to_vec(),
                })],
            )
            .unwrap();
        let node = g.get_nodes(&[id]).into_iter().next().unwrap().clone();
        store.persist_node("room-x", &node);
        store.persist_node("room-x", &node);
        assert_eq!(store.load_room_nodes("room-x").len(), 1);
    }

    #[test]
    fn blob_roundtrip() {
        let dir = tmpdir();
        let store = DirPersistence::open(&dir).unwrap();
        let bytes = b"hello world".to_vec();
        let h = Hash::of(&bytes);
        store.persist_blob(&h, &bytes).unwrap();
        let loaded = store.get_blob(&h);
        assert_eq!(loaded, Some(bytes));
    }

    #[test]
    fn blob_tamper_rejected() {
        let dir = tmpdir();
        let store = DirPersistence::open(&dir).unwrap();
        let bytes = b"truthy".to_vec();
        let h = Hash::of(&bytes);
        store.persist_blob(&h, &bytes).unwrap();
        // Overwrite with bogus content.
        let p = store.blob_path(&h);
        std::fs::write(&p, b"LIES").unwrap();
        assert!(store.get_blob(&h).is_none());
    }

    #[test]
    fn blob_layout_is_flat_global_cas() {
        let dir = tmpdir();
        let store = DirPersistence::open(&dir).unwrap();
        let bytes = b"shared across rooms".to_vec();
        let h = Hash::of(&bytes);
        store.persist_blob(&h, &bytes).unwrap();
        let expected = dir.join("blobs").join("blake3").join(h.to_hex());
        assert!(expected.is_file(), "expected blob at {expected:?}");
    }

    #[test]
    fn known_room_ids_lists_every_persisted_room() {
        let dir = tmpdir();
        let store = DirPersistence::open(&dir).unwrap();
        let sk = SigningKey::from_bytes(&[0x33u8; 32]);
        for room in ["room-a", "room-b"] {
            let mut g = StateGraph::new();
            let id = g
                .apply_local(
                    &sk,
                    0,
                    vec![Op::Map(MapOp::Set {
                        key: "k".into(),
                        value: b"v".to_vec(),
                    })],
                )
                .unwrap();
            let node = g.get_nodes(&[id]).into_iter().next().unwrap().clone();
            store.persist_node(room, &node);
        }
        let mut rooms = store.known_room_ids();
        rooms.sort();
        assert_eq!(rooms, vec!["room-a".to_string(), "room-b".to_string()]);
    }

    #[test]
    fn migrate_legacy_blob_layout_moves_and_dedupes() {
        let dir = tmpdir();
        let blobs_root = dir.join("blobs");
        // Hand-construct a pre-v2 layout: two rooms, one shared blob.
        let bytes = b"shared blob content".to_vec();
        let h = Hash::of(&bytes);
        for room in ["room-a", "room-b"] {
            let room_dir = blobs_root.join(room);
            std::fs::create_dir_all(&room_dir).unwrap();
            std::fs::write(room_dir.join(h.to_hex()), &bytes).unwrap();
        }
        // A tampered/foreign file that must be quarantined, not adopted.
        let bogus_dir = blobs_root.join("room-a");
        std::fs::write(bogus_dir.join("not-a-hash"), b"junk").unwrap();

        let store = DirPersistence::open(&dir).unwrap();

        let migrated_path = blobs_root.join("blake3").join(h.to_hex());
        assert!(migrated_path.is_file());
        assert_eq!(std::fs::read(&migrated_path).unwrap(), bytes);
        assert!(blobs_root.join(".layout-v2").is_file());
        assert!(blobs_root
            .join(".migration-skipped")
            .join("room-a__not-a-hash")
            .is_file());

        // Re-opening must be a no-op (idempotent) and not error.
        drop(store);
        let _store2 = DirPersistence::open(&dir).unwrap();
        assert!(migrated_path.is_file());
    }

    #[test]
    fn migrate_legacy_blob_layout_is_noop_on_fresh_store() {
        let dir = tmpdir();
        let store = DirPersistence::open(&dir).unwrap();
        assert!(dir.join("blobs").join(".layout-v2").is_file());
        assert_eq!(store.get_blob(&Hash::of(b"anything")), None);
    }
}
