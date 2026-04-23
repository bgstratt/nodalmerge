use std::collections::HashMap;
use crate::{error::SyncError, hash::Hash, node::{NodeId, SyncNode}};

// ── NodeStore trait ───────────────────────────────────────────────────────────

/// Pluggable storage backend for DAG nodes.
///
/// The default implementation (`MemoryNodeStore`) keeps everything in a
/// `HashMap`. Future adapters: `IndexedDbNodeStore`, `RocksDbNodeStore`, etc.
pub trait NodeStore: Send + Sync {
    /// Persist a node. Must be idempotent (re-inserting the same node is fine).
    fn put(&mut self, node: SyncNode) -> Result<(), SyncError>;
    /// Retrieve a node by its content-addressable ID.
    fn get(&self, id: &NodeId) -> Option<&SyncNode>;
    /// Return the IDs of every node currently held in this store.
    fn all_ids(&self) -> Vec<NodeId>;

    /// Returns `true` if a node with the given ID is already stored.
    fn contains(&self, id: &NodeId) -> bool {
        self.get(id).is_some()
    }

    /// Number of nodes in the store.
    fn len(&self) -> usize {
        self.all_ids().len()
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ── BlobStore trait ───────────────────────────────────────────────────────────

/// Pluggable storage backend for large binary blobs (CAS).
///
/// Blobs are keyed by their Blake3 hash. The default implementation
/// (`MemoryBlobStore`) keeps everything in a `HashMap`. Future adapters:
/// `IndexedDbBlobStore`, `S3BlobStore` (which returns a presigned URL from
/// `resolve_url` and never transmits bytes through the engine), etc.
pub trait BlobStore: Send + Sync {
    /// Store a blob. If already present the store is unchanged. Returns the
    /// content-addressable hash.
    fn put(&mut self, data: Vec<u8>) -> Hash;
    /// Retrieve a blob by hash. Returns `None` if not present.
    fn get(&self, hash: &Hash) -> Option<Vec<u8>>;
    /// Returns `true` if the blob is present locally.
    fn contains(&self, hash: &Hash) -> bool;
    /// Byte length without loading the blob. Used for bandwidth planning,
    /// chunking thresholds, and streaming decisions.
    fn size(&self, hash: &Hash) -> Option<u64>;
    /// Partial fetch. Enables resumable transfers and WebRTC data-channel
    /// chunking without loading the full blob into memory.
    fn get_range(&self, hash: &Hash, range: std::ops::Range<u64>) -> Option<Vec<u8>>;
    /// Return a redirect URL instead of transmitting bytes (S3, R2, CDN, etc.).
    /// When `Some`, the client fetches from the URL directly; the engine never
    /// touches the blob bytes. Default: `None` (serve bytes inline).
    fn resolve_url(&self, hash: &Hash) -> Option<String> {
        let _ = hash;
        None
    }
}

// ── MemoryNodeStore ───────────────────────────────────────────────────────────

/// In-memory `NodeStore`. The default backend — identical to the original
/// embedded `HashMap` inside `StateGraph`.
#[derive(Debug, Default)]
pub struct MemoryNodeStore {
    data: HashMap<NodeId, SyncNode>,
}

impl MemoryNodeStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl NodeStore for MemoryNodeStore {
    fn put(&mut self, node: SyncNode) -> Result<(), SyncError> {
        self.data.insert(node.id, node);
        Ok(())
    }

    fn get(&self, id: &NodeId) -> Option<&SyncNode> {
        self.data.get(id)
    }

    fn all_ids(&self) -> Vec<NodeId> {
        self.data.keys().copied().collect()
    }

    // Override the default for O(1) instead of O(n).
    fn contains(&self, id: &NodeId) -> bool {
        self.data.contains_key(id)
    }

    fn len(&self) -> usize {
        self.data.len()
    }
}

// ── MemoryBlobStore ───────────────────────────────────────────────────────────

/// In-memory `BlobStore`. The default backend — identical to the original
/// `cas::BlobStore` struct.
#[derive(Debug, Default)]
pub struct MemoryBlobStore {
    data: HashMap<Hash, Vec<u8>>,
}

impl MemoryBlobStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Iterate over all hashes currently stored.
    pub fn hashes(&self) -> impl Iterator<Item = &Hash> {
        self.data.keys()
    }

    /// Return the subset of `remote` hashes that are not present locally.
    pub fn missing_from<'a>(&self, remote: &'a [Hash]) -> Vec<&'a Hash> {
        remote.iter().filter(|h| !self.contains(h)).collect()
    }
}

impl BlobStore for MemoryBlobStore {
    fn put(&mut self, data: Vec<u8>) -> Hash {
        let hash = Hash::of(&data);
        self.data.entry(hash).or_insert(data);
        hash
    }

    fn get(&self, hash: &Hash) -> Option<Vec<u8>> {
        self.data.get(hash).cloned()
    }

    fn contains(&self, hash: &Hash) -> bool {
        self.data.contains_key(hash)
    }

    fn size(&self, hash: &Hash) -> Option<u64> {
        self.data.get(hash).map(|v| v.len() as u64)
    }

    fn get_range(&self, hash: &Hash, range: std::ops::Range<u64>) -> Option<Vec<u8>> {
        let data = self.data.get(hash)?;
        let start = range.start as usize;
        let end = (range.end as usize).min(data.len());
        if start >= data.len() {
            return None;
        }
        Some(data[start..end].to_vec())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ── MemoryNodeStore ───────────────────────────────────────────────────────

    #[test]
    fn node_store_put_get_roundtrip() {
        use crate::{node::SyncNode, op::Transaction};
        let tx = Transaction {
            author: [0xABu8; 32],
            lamport: 1,
            wall_ms: 1000,
            ops: vec![],
            parents: vec![],
        };
        let node = SyncNode::new(tx);
        let id = node.id;

        let mut store = MemoryNodeStore::new();
        assert!(!store.contains(&id));
        store.put(node.clone()).unwrap();
        assert!(store.contains(&id));
        assert_eq!(store.get(&id).unwrap().id, id);
        assert_eq!(store.len(), 1);
        assert_eq!(store.all_ids(), vec![id]);
    }

    #[test]
    fn node_store_put_idempotent() {
        use crate::{node::SyncNode, op::Transaction};
        let tx = Transaction {
            author: [0x01u8; 32],
            lamport: 1,
            wall_ms: 0,
            ops: vec![],
            parents: vec![],
        };
        let node = SyncNode::new(tx);
        let mut store = MemoryNodeStore::new();
        store.put(node.clone()).unwrap();
        store.put(node).unwrap(); // should not error
        assert_eq!(store.len(), 1);
    }

    // ── MemoryBlobStore ───────────────────────────────────────────────────────

    #[test]
    fn blob_store_put_get_contains() {
        let mut store = MemoryBlobStore::new();
        let data = b"hello world".to_vec();
        let hash = store.put(data.clone());

        assert!(store.contains(&hash));
        assert_eq!(store.get(&hash).unwrap(), data);
        assert_eq!(store.size(&hash).unwrap(), data.len() as u64);
    }

    #[test]
    fn blob_store_put_idempotent() {
        let mut store = MemoryBlobStore::new();
        let data = b"idempotent".to_vec();
        let h1 = store.put(data.clone());
        let h2 = store.put(data);
        assert_eq!(h1, h2);
        assert_eq!(store.hashes().count(), 1);
    }

    #[test]
    fn blob_store_size_missing() {
        let store = MemoryBlobStore::new();
        let phantom = Hash::of(b"does not exist");
        assert!(store.size(&phantom).is_none());
    }

    #[test]
    fn blob_store_get_range_full() {
        let mut store = MemoryBlobStore::new();
        let data = b"abcdefghij".to_vec();
        let hash = store.put(data.clone());
        assert_eq!(store.get_range(&hash, 0..10).unwrap(), data);
    }

    #[test]
    fn blob_store_get_range_partial() {
        let mut store = MemoryBlobStore::new();
        let data = b"abcdefghij".to_vec();
        let hash = store.put(data);
        // bytes 2..5 → "cde"
        assert_eq!(store.get_range(&hash, 2..5).unwrap(), b"cde");
    }

    #[test]
    fn blob_store_get_range_clamps_to_end() {
        let mut store = MemoryBlobStore::new();
        let data = b"abcde".to_vec();
        let hash = store.put(data.clone());
        // asking for more than available should clamp, not panic
        assert_eq!(store.get_range(&hash, 3..100).unwrap(), b"de");
    }

    #[test]
    fn blob_store_get_range_out_of_bounds() {
        let mut store = MemoryBlobStore::new();
        let data = b"abcde".to_vec();
        let hash = store.put(data);
        // start past end → None
        assert!(store.get_range(&hash, 10..20).is_none());
    }

    #[test]
    fn blob_store_missing_from() {
        let mut store = MemoryBlobStore::new();
        let h1 = store.put(b"a".to_vec());
        let h2 = Hash::of(b"not stored");
        let remote = [h1, h2];
        let missing = store.missing_from(&remote);
        assert_eq!(missing, vec![&h2]);
    }
}
