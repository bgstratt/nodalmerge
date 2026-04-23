use std::collections::{HashMap, HashSet, BTreeMap};
use crate::{
    compaction::is_snapshot_node,
    error::SyncError,
    frontier::Frontier,
    hash::Hash,
    node::{NodeId, SyncNode},
    op::{Op, MapOp, TextOp, Transaction},
    policy::Policy,
    storage::{NodeStore, MemoryNodeStore},
};

/// The resolved, queryable state of the LWW-Map after applying all nodes.
pub type ResolvedMap = HashMap<String, Vec<u8>>;

/// Outcome of [`StateGraph::apply_remote_batch`].
///
/// Every node passed in lands in exactly one of `accepted` or `rejected`.
/// Duplicates (already in the store, or repeated within the input slice)
/// are silently dropped \u2014 they appear in neither bucket.
#[derive(Debug, Default)]
pub struct BatchResult {
    pub accepted: Vec<NodeId>,
    pub rejected: Vec<(NodeId, SyncError)>,
}

/// Verify the signatures of the nodes at `indices` within `nodes`.
///
/// Tries `ed25519_dalek::verify_batch` first \u2014 a single multi-scalar
/// multiplication that is ~2\u20133\u00d7 faster than per-node verify for chunks
/// \u2265 32. If the batch verify fails (one or more bad signatures, or a
/// malformed key/signature), falls back to per-node verification so the
/// caller can identify exactly which node was bad.
fn verify_chunk(nodes: &[SyncNode], indices: &[usize]) -> Vec<(usize, bool)> {
    use ed25519_dalek::{Signature as DalekSig, VerifyingKey};

    // Build the parallel arrays for `verify_batch`. Skip nodes whose author
    // public key fails to decode \u2014 those are unconditionally bad.
    let mut good: Vec<usize> = Vec::with_capacity(indices.len());
    let mut messages: Vec<&[u8]> = Vec::with_capacity(indices.len());
    let mut sigs: Vec<DalekSig> = Vec::with_capacity(indices.len());
    let mut keys: Vec<VerifyingKey> = Vec::with_capacity(indices.len());
    let mut out: Vec<(usize, bool)> = Vec::with_capacity(indices.len());

    for &i in indices {
        let n = &nodes[i];
        match VerifyingKey::from_bytes(&n.transaction.author) {
            Ok(vk) => {
                good.push(i);
                messages.push(n.id.as_bytes());
                sigs.push(DalekSig::from_bytes(&n.signature.0));
                keys.push(vk);
            }
            Err(_) => out.push((i, false)),
        }
    }

    if good.is_empty() {
        return out;
    }

    if ed25519_dalek::verify_batch(&messages, &sigs, &keys).is_ok() {
        for i in good {
            out.push((i, true));
        }
    } else {
        // One or more bad signatures \u2014 identify which.
        for i in good {
            let ok = nodes[i].verify_signature().is_ok();
            out.push((i, ok));
        }
    }
    out
}

/// E3: Configuration for tick-based op batching.
///
/// When active, `set`/`delete` calls accumulate into a buffer instead of
/// immediately creating a signed node. The caller flushes the buffer either
/// on a timer (`interval_ms`) or when `max_ops_per_tick` is reached.
/// Tick boundaries are a transport optimisation only — the CRDT merge logic
/// operates on individual ops and produces identical resolved state regardless
/// of how ops are grouped into nodes.
#[derive(Clone, Debug)]
pub struct TickConfig {
    /// Desired tick window in milliseconds. Used by the JS layer to drive
    /// `setInterval`; the Rust side does not run timers.
    pub interval_ms: u64,
    /// Flush the buffer early if this many ops have accumulated.
    pub max_ops_per_tick: usize,
}

/// The Sync-Graph: an append-only DAG of `SyncNode`s, generic over the
/// node storage backend.
///
/// # CRDT semantics
/// This implements a Last-Write-Wins Map (LWW-Map) where "last" is defined
/// by the Lamport clock. When two concurrent ops `Set` the same key, the
/// one with the higher `lamport` wins. Ties are broken by author public key
/// (lexicographic), giving a deterministic total order on every peer.
///
/// # Merkle structure
/// Every node links to its parents by hash, forming a DAG identical to git's
/// commit graph. The Merkle root is computed by hashing the sorted set of all
/// leaf node IDs (nodes with no children). Two graphs are equal iff their
/// Merkle roots match, enabling O(1) "do we need to sync?" checks.
#[derive(Debug)]
pub struct StateGraph<N: NodeStore = MemoryNodeStore> {
    /// Node storage backend. Swap for IndexedDB, disk, or S3 adapters.
    nodes: N,
    /// Minimal set of "leaf" node IDs (nodes with no children yet).
    /// Updated incrementally as nodes are inserted.
    leaves: HashSet<NodeId>,
    /// The author's own Lamport clock. Incremented on each local transaction.
    lamport: u64,
    /// Room-level write policy. Enforced in `apply_remote`.
    /// Defaults to `AllowAll` (open room, identical to pre-A5 behaviour).
    policy: Policy,
    /// Serializable frontier — mirrors `leaves` and kept in sync on every
    /// insert. Used by the `hello`/`welcome` handshake (A6) and IBF (B1).
    frontier: Frontier,
    /// E2: IDs of nodes produced by `apply_local` on this peer.
    /// Canonical state excludes these — they are speculative until a remote
    /// peer (e.g. the server) re-broadcasts or confirms them.
    local_node_ids: HashSet<NodeId>,
    /// Session-scoped cache of node IDs whose Ed25519 signature has already
    /// been verified on this peer. Populated by `apply_local` (we signed it)
    /// and successful `apply_remote` / `apply_remote_batch`. Used to skip
    /// re-verification when the same node is re-broadcast (server echo,
    /// reconnect catchup, replay). Never persisted; cleared on process exit.
    verified_ids: HashSet<NodeId>,
}

impl Default for StateGraph<MemoryNodeStore> {
    fn default() -> Self {
        StateGraph { nodes: MemoryNodeStore::new(), leaves: HashSet::new(), lamport: 0, policy: Policy::default(), frontier: Frontier::default(), local_node_ids: HashSet::new(), verified_ids: HashSet::new() }
    }
}

impl StateGraph<MemoryNodeStore> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<N: NodeStore> StateGraph<N> {
    /// Construct a `StateGraph` with a custom storage backend.
    pub fn with_store(nodes: N) -> Self {
        StateGraph { nodes, leaves: HashSet::new(), lamport: 0, policy: Policy::default(), frontier: Frontier::default(), local_node_ids: HashSet::new(), verified_ids: HashSet::new() }
    }

    /// Replace the room policy. All subsequent `apply_remote` calls will
    /// enforce the new policy. Existing nodes are not re-validated.
    pub fn set_policy(&mut self, policy: Policy) {
        self.policy = policy;
    }

    /// Return a reference to the current room policy.
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Current Lamport clock value.  The next `apply_local` call will use
    /// `(lamport + 1).max(wall_ms)` — expose this so callers (e.g. the
    /// bridge's E2EE layer) can pre-compute the lamport that a forthcoming
    /// transaction will carry.
    pub fn lamport(&self) -> u64 {
        self.lamport
    }

    // -------------------------------------------------------------------------
    // Insertion
    // -------------------------------------------------------------------------

    /// Apply a locally-authored transaction, producing a new signed DAG node.
    ///
    /// `signing_key` is the author's Ed25519 signing key. The corresponding
    /// verifying key bytes are stored in `transaction.author` and used by
    /// peers to verify the signature on `apply_remote`.
    pub fn apply_local(
        &mut self,
        signing_key: &ed25519_dalek::SigningKey,
        wall_ms: u64,
        ops: Vec<Op>,
    ) -> Result<NodeId, SyncError> {
        self.lamport = (self.lamport + 1).max(wall_ms);
        let author: [u8; 32] = signing_key.verifying_key().to_bytes();
        let parents: Vec<Hash> = self.leaves.iter().copied().collect();
        let tx = Transaction { author, lamport: self.lamport, wall_ms, ops, parents };
        let node = SyncNode::new_signed(tx, signing_key);
        let id = node.id;
        self.insert_node(node)?;
        // E2: mark this node as locally authored (speculative, not yet confirmed).
        self.local_node_ids.insert(id);
        // We just signed this node ourselves; signature is valid by construction.
        self.verified_ids.insert(id);
        Ok(id)
    }

    /// Insert a node received from a remote peer.
    ///
    /// Validates that:
    /// - The node's ID matches the hash of its transaction.
    /// - The Ed25519 signature is valid (unsigned/zero-sig nodes pass through).
    /// - No parent references an unknown node (use `missing_hashes` first).
    /// - Every op key is permitted for the node's author under the room policy.
    pub fn apply_remote(&mut self, node: SyncNode) -> Result<(), SyncError> {
        // Verify content-addressable integrity.
        let expected = node.transaction.hash();
        if expected != node.id {
            return Err(SyncError::HashMismatch { expected, actual: node.id });
        }
        // Verify Ed25519 signature (no-op for zero/unsigned nodes, and skipped
        // if we already verified this id earlier in the session).
        if !self.verified_ids.contains(&node.id) {
            node.verify_signature()?;
            self.verified_ids.insert(node.id);
        }
        self.apply_remote_verified(node)
    }

    /// Apply a node whose hash + signature have already been validated.
    /// Performs parent + policy checks and inserts. Used by both
    /// `apply_remote` and `apply_remote_batch`.
    fn apply_remote_verified(&mut self, node: SyncNode) -> Result<(), SyncError> {
        // All parents must already exist locally.
        for parent in node.parents() {
            if !self.nodes.contains(parent) {
                return Err(SyncError::MissingParent(*parent));
            }
        }
        // Enforce write policy: every op key must be permitted for this author.
        // Snapshot nodes (D3) carry \x00-prefixed system keys and bypass policy
        // so that compaction checkpoints can be accepted regardless of room rules.
        if !is_snapshot_node(&node) {
            let author = &node.transaction.author;
            for op in &node.transaction.ops {
                let key = match op {
                    Op::Map(MapOp::Set    { key, .. }) => key.as_str(),
                    Op::Map(MapOp::Delete { key })     => key.as_str(),
                    Op::Map(MapOp::SetBlob{ key, .. }) => key.as_str(),
                    Op::Text(TextOp::Insert { key, .. })
                    | Op::Text(TextOp::Delete { key, .. }) => key.as_str(),
                    Op::List(_) => continue,
                };
                if !self.policy.can_write(key, author) {
                    return Err(SyncError::PolicyViolation {
                        author: *author,
                        key: key.to_string(),
                    });
                }
            }
        }
        self.insert_node(node)?;
        Ok(())
    }

    /// Bulk-ingest a batch of remote nodes with parallel batched signature
    /// verification.
    ///
    /// Pipeline (see `apply_remote` doc-comment for per-step rationale):
    ///
    /// 1. **Dedupe** — drop nodes already in the store and any duplicates
    ///    within the batch itself.
    /// 2. **Hash integrity** — cheap pre-crypto reject for tampered nodes.
    /// 3. **Batched verify** — group surviving signed nodes into chunks of
    ///    `BATCH_VERIFY_CHUNK` and run `ed25519_dalek::verify_batch` on each.
    ///    On native targets the chunks run in parallel via `rayon`; on wasm
    ///    they run sequentially. If any chunk fails as a batch, that chunk
    ///    falls back to per-node verification so we can identify which
    ///    specific node was bad without rejecting the rest.
    /// 4. **Apply in input order** — parent ordering is preserved so a node
    ///    can reference an earlier sibling within the same batch.
    ///
    /// Returns a `BatchResult` listing accepted ids and rejected `(id, reason)`
    /// pairs. The function never panics on bad input — every node is
    /// accounted for in exactly one bucket.
    pub fn apply_remote_batch(&mut self, nodes: Vec<SyncNode>) -> BatchResult {
        let mut result = BatchResult::default();

        // Step 1 + 2: dedupe and hash check.
        let mut to_verify: Vec<SyncNode> = Vec::with_capacity(nodes.len());
        let mut seen_in_batch: HashSet<NodeId> = HashSet::with_capacity(nodes.len());
        for node in nodes {
            if !seen_in_batch.insert(node.id) {
                continue;
            }
            if self.nodes.contains(&node.id) {
                continue;
            }
            let expected = node.transaction.hash();
            if expected != node.id {
                result.rejected.push((node.id, SyncError::HashMismatch { expected, actual: node.id }));
                continue;
            }
            to_verify.push(node);
        }

        // Step 3: parallel batched signature verification.
        let verify_ok = self.verify_nodes_batched(&to_verify);

        // Step 4: apply in input order.
        for (node, ok) in to_verify.into_iter().zip(verify_ok) {
            if !ok {
                let id = node.id;
                result.rejected.push((id, SyncError::InvalidSignature(id)));
                continue;
            }
            // Mark verified so any future re-broadcast skips the crypto check.
            self.verified_ids.insert(node.id);
            let id = node.id;
            match self.apply_remote_verified(node) {
                Ok(()) => result.accepted.push(id),
                Err(e) => result.rejected.push((id, e)),
            }
        }

        result
    }

    /// Run batched signature verification over `nodes`. Returns one bool per
    /// input slot in the same order. Zero-signature (legacy/unsigned) nodes
    /// and previously-verified ids are short-circuited to `true`.
    fn verify_nodes_batched(&self, nodes: &[SyncNode]) -> Vec<bool> {
        let mut results = vec![true; nodes.len()];
        let need: Vec<usize> = nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| !n.signature.is_zero() && !self.verified_ids.contains(&n.id))
            .map(|(i, _)| i)
            .collect();

        if need.is_empty() {
            return results;
        }

        const BATCH_VERIFY_CHUNK: usize = 64;

        #[cfg(not(target_arch = "wasm32"))]
        let chunk_outputs: Vec<Vec<(usize, bool)>> = {
            use rayon::prelude::*;
            need.par_chunks(BATCH_VERIFY_CHUNK)
                .map(|chunk| verify_chunk(nodes, chunk))
                .collect()
        };

        #[cfg(target_arch = "wasm32")]
        let chunk_outputs: Vec<Vec<(usize, bool)>> = need
            .chunks(BATCH_VERIFY_CHUNK)
            .map(|chunk| verify_chunk(nodes, chunk))
            .collect();

        for chunk in chunk_outputs {
            for (idx, ok) in chunk {
                results[idx] = ok;
            }
        }
        results
    }

    fn insert_node(&mut self, node: SyncNode) -> Result<(), SyncError> {
        if self.nodes.contains(&node.id) {
            return Err(SyncError::DuplicateNode(node.id));
        }
        // This node's parents are no longer leaves.
        for parent in node.parents() {
            self.leaves.remove(parent);
        }
        self.leaves.insert(node.id);
        // Advance the serializable frontier in lockstep with `leaves`.
        self.frontier.advance(&node);
        // Advance our own Lamport clock past anything we've seen.
        if node.lamport() > self.lamport {
            self.lamport = node.lamport();
        }
        self.nodes.put(node)?;
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Sync / Merkle diffing
    // -------------------------------------------------------------------------

    /// Compute the Merkle root: the Blake3 hash of all sorted leaf IDs.
    ///
    /// Two graphs with the same Merkle root are guaranteed to have identical
    /// state. This is the first value exchanged in the sync handshake.
    pub fn merkle_root(&self) -> Hash {
        let mut sorted: Vec<&Hash> = self.leaves.iter().collect();
        sorted.sort_unstable();
        let mut hasher = blake3::Hasher::new();
        for h in sorted {
            hasher.update(h.as_bytes());
        }
        Hash(*hasher.finalize().as_bytes())
    }

    /// Return the IDs of all nodes we have that the remote is missing.
    ///
    /// `remote_known` is the set of node IDs the remote peer already has
    /// (sent during the handshake). We return everything else.
    pub fn missing_hashes(&self, remote_known: &HashSet<NodeId>) -> Vec<NodeId> {
        self.nodes
            .all_ids()
            .into_iter()
            .filter(|id| !remote_known.contains(id))
            .collect()
    }

    /// Retrieve nodes by ID for transmission to a remote peer.
    pub fn get_nodes(&self, ids: &[NodeId]) -> Vec<&SyncNode> {
        ids.iter().filter_map(|id| self.nodes.get(id)).collect()
    }

    // -------------------------------------------------------------------------
    // State resolution (LWW-Map CRDT)
    // -------------------------------------------------------------------------

    /// Resolve the current value of the shared map by replaying all nodes in
    /// causal order and applying LWW semantics per key.
    ///
    /// Returns the **speculative** view — includes local (unconfirmed) writes.
    /// Use `resolve_canonical()` for confirmed-only state (E2).
    pub fn resolve(&self) -> ResolvedMap {
        self.resolve_inner(false)
    }

    /// Canonical state: like `resolve()` but excludes nodes that were authored
    /// locally on this peer and have not yet been confirmed by a remote peer.
    ///
    /// In Authoritative mode (E1), the server re-broadcasts signed canonical
    /// nodes; once they arrive via `apply_remote` they enter the canonical view.
    /// In AllowAll / Cooperative mode, all remote writes are canonical.
    pub fn resolve_canonical(&self) -> ResolvedMap {
        self.resolve_inner(true)
    }

    /// Per-key read from the speculative view (local + remote nodes).
    pub fn read_speculative(&self, key: &str) -> Option<Vec<u8>> {
        self.resolve_inner(false).remove(key)
    }

    /// Per-key read from the canonical view (remote nodes only).
    pub fn read_canonical(&self, key: &str) -> Option<Vec<u8>> {
        self.resolve_inner(true).remove(key)
    }

    /// Internal LWW scan. When `skip_local` is true, nodes whose IDs are in
    /// `local_node_ids` are excluded (canonical view).
    fn resolve_inner(&self, skip_local: bool) -> ResolvedMap {
        let mut per_key: BTreeMap<String, (u64, [u8; 32], Option<Vec<u8>>)> = BTreeMap::new();

        let node_ids = self.nodes.all_ids();
        for id in &node_ids {
            if skip_local && self.local_node_ids.contains(id) {
                continue; // exclude unconfirmed local writes from canonical view
            }
            let Some(node) = self.nodes.get(id) else { continue };
            let tx = &node.transaction;
            for op in &tx.ops {
                match op {
                    Op::Map(MapOp::Set { key, value }) => {
                        let entry = per_key.entry(key.clone()).or_insert((0, [0u8; 32], None));
                        if (tx.lamport, tx.author) > (entry.0, entry.1) {
                            *entry = (tx.lamport, tx.author, Some(value.clone()));
                        }
                    }
                    Op::Map(MapOp::Delete { key }) => {
                        let entry = per_key.entry(key.clone()).or_insert((0, [0u8; 32], None));
                        if (tx.lamport, tx.author) > (entry.0, entry.1) {
                            *entry = (tx.lamport, tx.author, None);
                        }
                    }
                    Op::Map(MapOp::SetBlob { key, blob_hash }) => {
                        let entry = per_key.entry(key.clone()).or_insert((0, [0u8; 32], None));
                        if (tx.lamport, tx.author) > (entry.0, entry.1) {
                            *entry = (tx.lamport, tx.author, Some(blob_hash.as_bytes().to_vec()));
                        }
                    }
                    Op::List(_) | Op::Text(_) => {} // stubs — Phase C
                }
            }
        }

        per_key
            .into_iter()
            .filter_map(|(key, (_, _, value))| value.map(|v| (key, v)))
            .collect()
    }

    /// Like `resolve()` but also returns the winning Lamport timestamp,
    /// author bytes, and a flag indicating whether the value is a blob hash
    /// (`is_blob = true`) or inline bytes (`is_blob = false`).
    ///
    /// When `is_blob` is true, `value` holds the 32-byte Blake3 blob hash;
    /// callers must look that up in the local `BlobStore` to get actual bytes.
    pub fn resolve_with_meta(&self) -> HashMap<String, (u64, [u8; 32], Vec<u8>, bool)> {
        let mut per_key: BTreeMap<String, (u64, [u8; 32], Option<Vec<u8>>, bool)> = BTreeMap::new();

        let node_ids = self.nodes.all_ids();
        for id in &node_ids {
            let Some(node) = self.nodes.get(id) else { continue };
            let tx = &node.transaction;
            for op in &tx.ops {
                match op {
                    Op::Map(MapOp::Set { key, value }) => {
                        let entry = per_key.entry(key.clone()).or_insert((0, [0u8; 32], None, false));
                        if (tx.lamport, tx.author) > (entry.0, entry.1) {
                            *entry = (tx.lamport, tx.author, Some(value.clone()), false);
                        }
                    }
                    Op::Map(MapOp::Delete { key }) => {
                        let entry = per_key.entry(key.clone()).or_insert((0, [0u8; 32], None, false));
                        if (tx.lamport, tx.author) > (entry.0, entry.1) {
                            *entry = (tx.lamport, tx.author, None, false);
                        }
                    }
                    Op::Map(MapOp::SetBlob { key, blob_hash }) => {
                        let entry = per_key.entry(key.clone()).or_insert((0, [0u8; 32], None, false));
                        if (tx.lamport, tx.author) > (entry.0, entry.1) {
                            *entry = (tx.lamport, tx.author, Some(blob_hash.as_bytes().to_vec()), true);
                        }
                    }
                    Op::List(_) | Op::Text(_) => {} // stubs — Phase C
                }
            }
        }

        per_key
            .into_iter()
            .filter_map(|(key, (lamport, author, value, is_blob))| {
                value.map(|v| (key, (lamport, author, v, is_blob)))
            })
            .collect()
    }

    /// Return the set of all blob hashes referenced by `SetBlob` ops in the
    /// graph. Used by the bridge to determine which blobs must be fetched
    /// from peers.
    pub fn referenced_blob_hashes(&self) -> HashSet<Hash> {
        let mut hashes = HashSet::new();
        let node_ids = self.nodes.all_ids();
        for id in &node_ids {
            let Some(node) = self.nodes.get(id) else { continue };
            for op in &node.transaction.ops {
                if let Op::Map(MapOp::SetBlob { blob_hash, .. }) = op {
                    hashes.insert(*blob_hash);
                }
            }
        }
        hashes
    }

    // -------------------------------------------------------------------------
    // Accessors
    // -------------------------------------------------------------------------

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn current_lamport(&self) -> u64 {
        self.lamport
    }

    pub fn leaf_ids(&self) -> &HashSet<NodeId> {
        &self.leaves
    }

    pub fn all_node_ids(&self) -> Vec<NodeId> {
        self.nodes.all_ids()
    }

    /// Return the current frontier (serializable DAG tips).
    pub fn frontier(&self) -> Frontier {
        self.frontier.clone()
    }

    // -------------------------------------------------------------------------
    // C1: RGA Collaborative Text
    // -------------------------------------------------------------------------

    /// Resolve the visible RGA character sequence for `key`.
    ///
    /// Returns `(OpId, char)` pairs in sequence order.  Tombstoned characters
    /// are excluded from the output but their IDs remain valid anchors for
    /// future insertions around them.
    pub fn resolve_text_seq(&self, key: &str) -> Vec<(crate::op::OpId, char)> {
        let node_ids = self.nodes.all_ids();
        let nodes: Vec<&SyncNode> = node_ids.iter()
            .filter_map(|id| self.nodes.get(id))
            .collect();
        crate::text::resolve_text_seq(&nodes, key)
    }

    /// Alias of `resolve_text_seq` — kept for bridge call-sites that want
    /// the full `(OpId, char)` pairs explicitly named.
    pub fn resolve_text_seq_with_chars(&self, key: &str) -> Vec<(crate::op::OpId, char)> {
        self.resolve_text_seq(key)
    }

    /// Resolve the RGA text for `key` as a plain UTF-8 string.
    pub fn resolve_text(&self, key: &str) -> String {
        let node_ids = self.nodes.all_ids();
        let nodes: Vec<&SyncNode> = node_ids.iter()
            .filter_map(|id| self.nodes.get(id))
            .collect();
        crate::text::resolve_text(&nodes, key)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn key_a() -> SigningKey { SigningKey::from_bytes(&[0x0Au8; 32]) }
    fn key_b() -> SigningKey { SigningKey::from_bytes(&[0x0Bu8; 32]) }

    fn set(key: &str, val: &str) -> Op {
        Op::Map(MapOp::Set { key: key.into(), value: val.as_bytes().to_vec() })
    }

    #[test]
    fn basic_set_and_resolve() {
        let mut g = StateGraph::new();
        g.apply_local(&key_a(), 0, vec![set("name", "Alice")]).unwrap();
        let state = g.resolve();
        assert_eq!(state.get("name").map(|v| v.as_slice()), Some(b"Alice".as_slice()));
    }

    #[test]
    fn lww_last_write_wins() {
        let mut g = StateGraph::new();
        g.apply_local(&key_a(), 0, vec![Op::Map(MapOp::Set { key: "score".into(), value: b"10".to_vec() })]).unwrap();
        g.apply_local(&key_a(), 1, vec![Op::Map(MapOp::Set { key: "score".into(), value: b"20".to_vec() })]).unwrap();
        let state = g.resolve();
        assert_eq!(state.get("score").map(|v| v.as_slice()), Some(b"20".as_slice()));
    }

    #[test]
    fn delete_removes_key() {
        let mut g = StateGraph::new();
        g.apply_local(&key_a(), 0, vec![set("x", "1")]).unwrap();
        g.apply_local(&key_a(), 1, vec![Op::Map(MapOp::Delete { key: "x".into() })]).unwrap();
        let state = g.resolve();
        assert!(!state.contains_key("x"));
    }

    #[test]
    fn merkle_root_changes_on_new_node() {
        let mut g = StateGraph::new();
        let root0 = g.merkle_root();
        g.apply_local(&key_a(), 0, vec![set("k", "v")]).unwrap();
        let root1 = g.merkle_root();
        assert_ne!(root0, root1);
    }

    #[test]
    fn signature_verified_on_apply_remote() {
        let ka = key_a();
        let mut g = StateGraph::new();
        let id = g.apply_local(&ka, 0, vec![set("x", "1")]).unwrap();
        // Extract the signed node and re-apply to a fresh graph — should pass.
        let node = g.get_nodes(&[id])[0].clone();
        let mut g2 = StateGraph::new();
        g2.apply_remote(node).unwrap();
        assert_eq!(g2.node_count(), 1);
    }

    #[test]
    fn tampered_signature_rejected() {
        let ka = key_a();
        let mut g = StateGraph::new();
        let id = g.apply_local(&ka, 0, vec![set("x", "1")]).unwrap();
        let mut node = g.get_nodes(&[id])[0].clone();
        node.signature.0[0] ^= 0xff; // corrupt first byte
        let mut g2 = StateGraph::new();
        assert!(matches!(g2.apply_remote(node), Err(SyncError::InvalidSignature(_))));
    }

    #[test]
    fn concurrent_conflict_resolved_deterministically() {
        let ka = key_a();
        let kb = key_b();
        // Build two signed nodes with distinct lamport clocks.
        let tx_a = Transaction {
            author: ka.verifying_key().to_bytes(),
            lamport: 1,
            wall_ms: 1000,
            ops: vec![Op::Map(MapOp::Set { key: "val".into(), value: b"from_A".to_vec() })],
            parents: vec![],
        };
        let node_a = SyncNode::new_signed(tx_a, &ka);

        let tx_b = Transaction {
            author: kb.verifying_key().to_bytes(),
            lamport: 2,
            wall_ms: 999,
            ops: vec![Op::Map(MapOp::Set { key: "val".into(), value: b"from_B".to_vec() })],
            parents: vec![],
        };
        let node_b = SyncNode::new_signed(tx_b, &kb);

        let mut g1 = StateGraph::new();
        g1.apply_remote(node_a.clone()).unwrap();
        g1.apply_remote(node_b.clone()).unwrap();

        let mut g2 = StateGraph::new();
        g2.apply_remote(node_b).unwrap();
        g2.apply_remote(node_a).unwrap();

        let r1 = g1.resolve();
        let r2 = g2.resolve();
        assert_eq!(r1.get("val"), r2.get("val"));
        assert_eq!(r1.get("val").map(|v| v.as_slice()), Some(b"from_B".as_slice()));
    }

    #[test]
    fn missing_hashes_for_sync() {
        let ka = key_a();
        let mut g_server = StateGraph::new();
        g_server.apply_local(&ka, 0, vec![set("k1", "v1")]).unwrap();
        g_server.apply_local(&ka, 1, vec![set("k2", "v2")]).unwrap();

        let client_known: HashSet<NodeId> = HashSet::new();
        let missing = g_server.missing_hashes(&client_known);
        assert_eq!(missing.len(), 2);
    }

    // -------------------------------------------------------------------------
    // A5 policy enforcement tests
    // -------------------------------------------------------------------------

    fn make_node(key: &str, val: &str, signing_key: &SigningKey) -> SyncNode {
        let tx = Transaction {
            author: signing_key.verifying_key().to_bytes(),
            lamport: 1,
            wall_ms: 0,
            ops: vec![Op::Map(MapOp::Set { key: key.into(), value: val.as_bytes().to_vec() })],
            parents: vec![],
        };
        SyncNode::new_signed(tx, signing_key)
    }

    #[test]
    fn policy_open_room_allows_any_author() {
        // Default policy (AllowAll) — no rules — any author may write anything.
        let mut g = StateGraph::new();
        let node = make_node("world/pos", "up", &key_a());
        g.apply_remote(node).unwrap();
        assert_eq!(g.node_count(), 1);
    }

    #[test]
    fn policy_deny_unauthorized_author() {
        use crate::policy::{Policy, PolicyDefault, PolicyRule};

        let ka = key_a();
        let kb = key_b();
        let authorized = ka.verifying_key().to_bytes();

        let policy = Policy {
            rules: vec![PolicyRule {
                path_glob: "world/**".into(),
                can_write: vec![authorized],
                can_read: vec![],
                can_derive: vec![],
            }],
            default: PolicyDefault::DenyAll,
        };

        let mut g = StateGraph::new();
        g.set_policy(policy);

        // key_a is authorized — should succeed.
        let good_node = make_node("world/pos", "up", &ka);
        g.apply_remote(good_node).unwrap();

        // key_b is NOT authorized — should be rejected.
        let bad_node = make_node("world/pos", "down", &kb);
        assert!(matches!(
            g.apply_remote(bad_node),
            Err(SyncError::PolicyViolation { .. })
        ));

        // Only the authorized node was stored.
        assert_eq!(g.node_count(), 1);
    }

    #[test]
    fn policy_wildcard_allows_matching_paths() {
        use crate::policy::{Policy, PolicyDefault, PolicyRule};

        let ka = key_a();
        let kb = key_b();
        let pub_a = ka.verifying_key().to_bytes();
        let pub_b = kb.verifying_key().to_bytes();

        let policy = Policy {
            rules: vec![
                // Only A may write to world/**
                PolicyRule {
                    path_glob: "world/**".into(),
                    can_write: vec![pub_a],
                    can_read: vec![],
                    can_derive: vec![],
                },
                // Both A and B may write to intent/*
                PolicyRule {
                    path_glob: "intent/*".into(),
                    can_write: vec![pub_a, pub_b],
                    can_read: vec![],
                    can_derive: vec![],
                },
            ],
            default: PolicyDefault::DenyAll,
        };

        let mut g = StateGraph::new();
        g.set_policy(policy);

        // A writes to world/** — allowed
        g.apply_remote(make_node("world/player/pos", "1,2", &ka)).unwrap();

        // B writes to world/** — denied
        assert!(matches!(
            g.apply_remote(make_node("world/player/pos", "3,4", &kb)),
            Err(SyncError::PolicyViolation { .. })
        ));

        // B writes to intent/* — allowed
        g.apply_remote(make_node("intent/move", "up", &kb)).unwrap();

        // unmatched path — DenyAll default blocks both
        assert!(matches!(
            g.apply_remote(make_node("other/key", "x", &ka)),
            Err(SyncError::PolicyViolation { .. })
        ));

        assert_eq!(g.node_count(), 2);
    }

    // -------------------------------------------------------------------------
    // E2: Speculative vs. Canonical State
    // -------------------------------------------------------------------------

    #[test]
    fn local_write_is_speculative_not_canonical() {
        let mut g = StateGraph::new();
        g.apply_local(&key_a(), 0, vec![set("pos", "local")]).unwrap();

        // Speculative view (all nodes) sees the local write.
        assert_eq!(
            g.resolve().get("pos").map(|v| v.as_slice()),
            Some(b"local".as_slice())
        );
        assert_eq!(g.read_speculative("pos").as_deref(), Some(b"local".as_ref()));

        // Canonical view excludes local-only nodes — key is absent.
        assert!(g.resolve_canonical().get("pos").is_none());
        assert!(g.read_canonical("pos").is_none());
    }

    #[test]
    fn remote_write_is_canonical_and_speculative() {
        let tx = Transaction {
            author: key_a().verifying_key().to_bytes(),
            lamport: 1,
            wall_ms: 0,
            ops: vec![Op::Map(MapOp::Set { key: "pos".into(), value: b"remote".to_vec() })],
            parents: vec![],
        };
        let node = SyncNode::new_signed(tx, &key_a());
        let mut g = StateGraph::new();
        g.apply_remote(node).unwrap();

        // Both views see the remote write.
        assert_eq!(g.read_speculative("pos").as_deref(), Some(b"remote".as_ref()));
        assert_eq!(g.read_canonical("pos").as_deref(), Some(b"remote".as_ref()));
    }

    #[test]
    fn snap_back_canonical_wins_on_conflict() {
        // Client writes locally (speculative).
        let ka = key_a();
        let kb = key_b();
        let mut g = StateGraph::new();
        g.apply_local(&ka, 1, vec![set("pos", "client")]).unwrap();

        // Speculative = client value; canonical = absent.
        assert_eq!(g.read_speculative("pos").as_deref(), Some(b"client".as_ref()));
        assert!(g.read_canonical("pos").is_none());

        // Server sends an authoritative node with higher lamport → wins LWW.
        let server_tx = Transaction {
            author: kb.verifying_key().to_bytes(),
            lamport: 5,
            wall_ms: 0,
            ops: vec![Op::Map(MapOp::Set { key: "pos".into(), value: b"server".to_vec() })],
            parents: vec![],
        };
        let server_node = SyncNode::new_signed(server_tx, &kb);
        g.apply_remote(server_node).unwrap();

        // Canonical view now shows server value.
        assert_eq!(g.read_canonical("pos").as_deref(), Some(b"server".as_ref()));
        // Speculative also resolves to server value (higher lamport wins LWW).
        assert_eq!(g.read_speculative("pos").as_deref(), Some(b"server".as_ref()));
    }

    #[test]
    fn same_ops_different_tick_groupings_converge() {
        // E3 determinism: ops in 1 node vs ops in 2 nodes → same resolved state.
        let ka = key_a();

        // One batch: both ops in a single node.
        let mut g1 = StateGraph::new();
        g1.apply_local(&ka, 1, vec![set("x", "1"), set("y", "2")]).unwrap();

        // Two batches: each op in its own node.
        let mut g2 = StateGraph::new();
        g2.apply_local(&ka, 1, vec![set("x", "1")]).unwrap();
        g2.apply_local(&ka, 2, vec![set("y", "2")]).unwrap();

        // Canonical from g1's perspective (no remote nodes) is empty, but
        // speculative state must be identical for the same content.
        assert_eq!(g1.read_speculative("x"), g2.read_speculative("x"));
        assert_eq!(g1.read_speculative("y"), g2.read_speculative("y"));
    }
}
