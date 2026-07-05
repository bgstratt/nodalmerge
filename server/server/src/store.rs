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
}

/// Blob-side persistence. See module docs for rationale.
pub trait BlobPersistence: Send + Sync + std::fmt::Debug {
    /// Return every previously-persisted blob for `room_id`.
    fn load_room_blobs(&self, room_id: &str) -> Vec<(Hash, Vec<u8>)>;
    /// Persist a single blob.
    fn persist_blob(&self, room_id: &str, hash: &Hash, bytes: &[u8]);

    /// G4 — two-phase blob GC sweep for one room.
    ///
    /// Walk every persisted blob for `room_id`. A blob is *live* when its
    /// hash is in `live`. Non-live blobs are tombstoned on the first
    /// sweep that sees them; they are deleted on a subsequent sweep once
    /// the tombstone's age exceeds `grace`. A blob that reappears in
    /// `live` after tombstoning has its tombstone cleared.
    ///
    /// `grace = Duration::ZERO` collapses the two phases. Returns the
    /// number of blobs actually deleted in this call. Default impl is a
    /// no-op (non-durable backends have nothing to GC).
    fn blob_gc_sweep(
        &self,
        _room_id: &str,
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
}

impl<N: NodePersistence, B: BlobPersistence> BlobPersistence for Composite<N, B> {
    fn load_room_blobs(&self, room_id: &str) -> Vec<(Hash, Vec<u8>)> {
        self.blobs.load_room_blobs(room_id)
    }
    fn persist_blob(&self, room_id: &str, hash: &Hash, bytes: &[u8]) {
        self.blobs.persist_blob(room_id, hash, bytes)
    }
    fn blob_gc_sweep(
        &self,
        room_id: &str,
        live: &std::collections::HashSet<Hash>,
        grace: Duration,
    ) -> usize {
        self.blobs.blob_gc_sweep(room_id, live, grace)
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
    fn load_room_blobs(&self, _room_id: &str) -> Vec<(Hash, Vec<u8>)> {
        Vec::new()
    }
    fn persist_blob(&self, _room_id: &str, _hash: &Hash, _bytes: &[u8]) {}
    fn blobs_durable(&self) -> bool {
        false
    }
}

// ─── DirPersistence ─────────────────────────────────────────────────────────

/// SQLite (nodes) + filesystem (blobs), all rooted at a single directory.
///
/// Layout:
///
/// ```text
/// <root>/
///   nodalmerge.db                      ← SQLite: one row per (room, node)
///   topology-promotions.db             ← SQLite: promotion proposals (Wave 3)
///   blobs/
///     <sanitized_room_id>/
///       <hash_hex>                     ← one file per blob (content-addressed)
/// ```
///
/// Sanitization: any byte outside `[A-Za-z0-9_-]` is escaped as `_XX` (hex)
/// so arbitrary room ids round-trip through the filesystem safely.
#[derive(Debug)]
pub struct DirPersistence {
    root: PathBuf,
    conn: Mutex<Connection>,
}

impl DirPersistence {
    /// On-disk store root (`nodalmerge.db`, `blobs/`, topology sidecars).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Open (or create) the store rooted at `root`. Creates the directory,
    /// opens the SQLite file, and ensures the schema.
    pub fn open(root: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        std::fs::create_dir_all(root.join("blobs"))?;
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
        })
    }

    fn blobs_dir_for(&self, room_id: &str) -> PathBuf {
        self.root.join("blobs").join(sanitize(room_id))
    }

    /// G4: sibling directory tree for tombstones, kept out of
    /// `blobs/<room>/` so `load_room_blobs` never has to skip them.
    fn tombstones_dir_for(&self, room_id: &str) -> PathBuf {
        self.root.join("blob-tombstones").join(sanitize(room_id))
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
}

impl BlobPersistence for DirPersistence {
    fn load_room_blobs(&self, room_id: &str) -> Vec<(Hash, Vec<u8>)> {
        let dir = self.blobs_dir_for(room_id);
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in rd.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(hash) = hash_from_hex(name) else {
                continue;
            };
            match std::fs::read(&path) {
                Ok(bytes) => {
                    // Verify integrity — reject tampered files.
                    let actual = Hash::of(&bytes);
                    if actual == hash {
                        out.push((hash, bytes));
                    } else {
                        tracing::warn!(?path, "blob file hash mismatch, skipping");
                    }
                }
                Err(e) => tracing::warn!(?e, ?path, "read blob file failed"),
            }
        }
        out
    }

    fn persist_blob(&self, room_id: &str, hash: &Hash, bytes: &[u8]) {
        let t0 = Instant::now();
        let dir = self.blobs_dir_for(room_id);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(?e, "persist_blob: create_dir_all failed");
            return;
        }
        let path = dir.join(hash.to_hex());
        if path.exists() {
            return;
        }
        // Write+rename = atomic on POSIX; on Windows it's a best-effort replace.
        let tmp = dir.join(format!("{}.tmp", hash.to_hex()));
        if let Err(e) = std::fs::write(&tmp, bytes) {
            tracing::warn!(?e, "persist_blob: write tmp failed");
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            tracing::warn!(?e, "persist_blob: rename failed");
            let _ = std::fs::remove_file(&tmp);
        }
        let elapsed = t0.elapsed().as_secs_f64();
        metrics::histogram!("nodalmerge_persistence_write_seconds", "kind" => "blob")
            .record(elapsed);
    }

    fn blob_gc_sweep(
        &self,
        room_id: &str,
        live: &std::collections::HashSet<Hash>,
        grace: Duration,
    ) -> usize {
        let blobs_dir = self.blobs_dir_for(room_id);
        let tombs_dir = self.tombstones_dir_for(room_id);
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
            // Skip stray `.tmp` writes from a crashed persist_blob.
            if name.ends_with(".tmp") {
                continue;
            }
            let Some(hash) = hash_from_hex(name) else {
                continue;
            };
            let tomb_path = tombs_dir.join(name);

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
                            tracing::warn!(?e, room = %room_id, blob = %name, "blob_gc_sweep: delete blob failed");
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
                            tracing::warn!(?e, room = %room_id, blob = %name, "blob_gc_sweep: immediate delete failed");
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

/// Escape a room-id into a filesystem-safe name. `[A-Za-z0-9_-]` pass through;
/// every other byte becomes `_HH` (two upper-hex digits).
fn sanitize(room_id: &str) -> String {
    let mut out = String::with_capacity(room_id.len());
    for b in room_id.as_bytes() {
        if b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-' {
            out.push(*b as char);
        } else {
            out.push('_');
            out.push_str(&format!("{:02X}", b));
        }
    }
    out
}

fn hash_from_hex(s: &str) -> Option<Hash> {
    if s.len() != 64 {
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

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
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
        store.persist_blob("room-y", &h, &bytes);
        let loaded = store.load_room_blobs("room-y");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0, h);
        assert_eq!(loaded[0].1, bytes);
    }

    #[test]
    fn blob_tamper_rejected() {
        let dir = tmpdir();
        let store = DirPersistence::open(&dir).unwrap();
        let bytes = b"truthy".to_vec();
        let h = Hash::of(&bytes);
        store.persist_blob("room-z", &h, &bytes);
        // Overwrite with bogus content.
        let p = store.blobs_dir_for("room-z").join(h.to_hex());
        std::fs::write(&p, b"LIES").unwrap();
        assert!(store.load_room_blobs("room-z").is_empty());
    }

    #[test]
    fn sanitize_safe_chars() {
        assert_eq!(sanitize("abc-ROOM_01"), "abc-ROOM_01");
        assert_eq!(sanitize("a/b"), "a_2Fb");
        assert_eq!(sanitize("hi!"), "hi_21");
    }
}
