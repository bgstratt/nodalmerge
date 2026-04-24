//! F4 — server-side persistence.
//!
//! The `ServerPersistence` trait is a narrow write-through hook that sits
//! *above* the `NodeStore`/`BlobStore` traits in `activesync-core`: every time
//! a room accepts a node or a blob we append it to durable storage; every
//! time a room is created we hydrate it from durable storage first.
//!
//! Keeping persistence outside the core stores avoids making `Room` /
//! `StateGraph` generic over the backend — the write-through stays local to
//! the server crate, while the in-memory `MemoryNodeStore` / `MemoryBlobStore`
//! continue to satisfy `&SyncNode` borrowing.
//!
//! Two implementations ship:
//!
//! * [`NoPersistence`] — the default. In-memory only; matches pre-F4 behavior.
//! * [`DirPersistence`] — SQLite for nodes (`<root>/activesync.db`) plus a
//!   content-addressed blob dir (`<root>/blobs/<room>/<hash>`). Hydrates on
//!   room creation; write-through on every accepted node/blob.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use activesync_core::{unpack_nodes, pack_nodes, Hash, SyncNode};
use rusqlite::{params, Connection};

/// Write-through persistence backing the server's in-memory rooms.
pub trait ServerPersistence: Send + Sync + std::fmt::Debug {
    /// Return every previously-persisted node for `room_id`, in insertion order.
    fn load_room_nodes(&self, room_id: &str) -> Vec<SyncNode>;
    /// Persist a single accepted node.
    fn persist_node(&self, room_id: &str, node: &SyncNode);
    /// Persist many accepted nodes in a single batch. Default impl loops
    /// [`persist_node`]; backends with transactional semantics should override
    /// to amortize fsync / commit cost across the whole batch.
    fn persist_nodes(&self, room_id: &str, nodes: &[&SyncNode]) {
        for n in nodes { self.persist_node(room_id, n); }
    }
    /// Return every previously-persisted blob for `room_id`.
    fn load_room_blobs(&self, room_id: &str) -> Vec<(Hash, Vec<u8>)>;
    /// Persist a single blob.
    fn persist_blob(&self, room_id: &str, hash: &Hash, bytes: &[u8]);
    /// `true` if this backend survives process restarts. The in-memory
    /// default returns `false`; durable backends (SQLite + files) return
    /// `true`. The idle-eviction sweeper refuses to evict rooms when this
    /// is `false`, because dropping an in-memory room would be pure data
    /// loss.
    fn is_durable(&self) -> bool { true }
}

// ─── NoPersistence ──────────────────────────────────────────────────────────

/// In-memory only. The default — matches pre-F4 behavior.
#[derive(Debug, Default)]
pub struct NoPersistence;

impl ServerPersistence for NoPersistence {
    fn load_room_nodes(&self, _room_id: &str) -> Vec<SyncNode> { Vec::new() }
    fn persist_node(&self, _room_id: &str, _node: &SyncNode) {}
    fn load_room_blobs(&self, _room_id: &str) -> Vec<(Hash, Vec<u8>)> { Vec::new() }
    fn persist_blob(&self, _room_id: &str, _hash: &Hash, _bytes: &[u8]) {}
    fn is_durable(&self) -> bool { false }
}

// ─── DirPersistence ─────────────────────────────────────────────────────────

/// SQLite (nodes) + filesystem (blobs), all rooted at a single directory.
///
/// Layout:
///
/// ```text
/// <root>/
///   activesync.db                      ← SQLite: one row per (room, node)
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
    /// Open (or create) the store rooted at `root`. Creates the directory,
    /// opens the SQLite file, and ensures the schema.
    pub fn open(root: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        std::fs::create_dir_all(root.join("blobs"))?;
        let db_path = root.join("activesync.db");
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
        ).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        Ok(Self { root, conn: Mutex::new(conn) })
    }

    fn blobs_dir_for(&self, room_id: &str) -> PathBuf {
        self.root.join("blobs").join(sanitize(room_id))
    }
}

impl ServerPersistence for DirPersistence {
    fn load_room_nodes(&self, room_id: &str) -> Vec<SyncNode> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT bytes FROM nodes WHERE room_id = ?1 ORDER BY seq ASC",
        ) {
            Ok(s) => s,
            Err(e) => { tracing::warn!(?e, "prepare load_room_nodes failed"); return Vec::new(); }
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
        let bytes = pack_nodes(&[node]);
        let conn = self.conn.lock().unwrap();
        if let Err(e) = conn.execute(
            "INSERT OR IGNORE INTO nodes (room_id, node_id, bytes) VALUES (?1, ?2, ?3)",
            params![room_id, &node.id.as_bytes()[..], bytes],
        ) {
            tracing::warn!(?e, "persist_node failed");
        }
    }

    fn persist_nodes(&self, room_id: &str, nodes: &[&SyncNode]) {
        if nodes.is_empty() { return; }
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
    }

    fn load_room_blobs(&self, room_id: &str) -> Vec<(Hash, Vec<u8>)> {
        let dir = self.blobs_dir_for(room_id);
        let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new(); };
        let mut out = Vec::new();
        for entry in rd.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else { continue };
            let Some(hash) = hash_from_hex(name) else { continue };
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
        let dir = self.blobs_dir_for(room_id);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(?e, "persist_blob: create_dir_all failed");
            return;
        }
        let path = dir.join(hash.to_hex());
        if path.exists() { return; }
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
    if s.len() != 64 { return None; }
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

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use activesync_core::{Op, MapOp, StateGraph};
    use ed25519_dalek::SigningKey;

    fn tmpdir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("activesync-test-{nanos}"));
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
        let id = g.apply_local(&sk, 0, vec![Op::Map(MapOp::Set{ key: "k".into(), value: b"v".to_vec()})]).unwrap();
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
        let id = g.apply_local(&sk, 0, vec![Op::Map(MapOp::Set{ key: "k".into(), value: b"v".to_vec()})]).unwrap();
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
