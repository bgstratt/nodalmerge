//! Frontier / Version Vector (A6).
//!
//! A frontier is the minimal set of DAG tips — the `NodeId`s that currently
//! have no successors. It compactly encodes "everything this peer has seen":
//! any peer that has all frontier heads (and their transitive ancestors) is
//! fully caught up.
//!
//! # Relationship to `leaves`
//! `StateGraph` already maintains an internal `leaves: HashSet<NodeId>` for
//! Merkle root computation. `Frontier` is the *serializable* form of that
//! same set, designed for wire transmission and future IBF integration (B1).
//!
//! # Wire encoding
//! `Frontier::encode()` emits a compact postcard byte string for use in the
//! `hello`/`welcome` handshake. B1 (IBF) will use this as its starting point.

use serde::{Deserialize, Serialize};
use crate::node::{NodeId, SyncNode};

/// Minimal set of DAG tips. Fully describes "what this peer has seen."
///
/// Two peers with identical `Frontier`s have identical graph state (assuming
/// the same ancestor nodes — Merkle root confirms this).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Frontier {
    /// The current set of DAG heads (leaf nodes with no successors yet).
    /// Sorted for deterministic serialization.
    pub heads: Vec<NodeId>,
}

impl Frontier {
    /// Construct a frontier from an existing set of head IDs.
    pub fn from_heads(mut heads: Vec<NodeId>) -> Self {
        heads.sort_unstable();
        heads.dedup();
        Frontier { heads }
    }

    /// Returns `true` if `id` appears in the current heads.
    ///
    /// Note: this is a shallow check — it only tests frontier membership, not
    /// full ancestry. Full ancestry queries require the node store. The IBF
    /// in B1 will use this for efficient set reconciliation.
    pub fn contains(&self, id: &NodeId) -> bool {
        self.heads.contains(id)
    }

    /// Advance the frontier when a new node is appended or merged.
    ///
    /// Removes every parent of `new_node` from `heads` (they are no longer
    /// tips), then adds `new_node.id` as a new head.
    pub fn advance(&mut self, new_node: &SyncNode) {
        // Remove parents — they are no longer leaves.
        for parent in new_node.parents() {
            self.heads.retain(|h| h != parent);
        }
        // Add this node as a new head (avoid duplicates).
        if !self.heads.contains(&new_node.id) {
            self.heads.push(new_node.id);
        }
        // Keep sorted for deterministic serialization.
        self.heads.sort_unstable();
    }

    /// Number of heads in the frontier.
    pub fn len(&self) -> usize {
        self.heads.len()
    }

    /// True if no nodes have been seen yet (empty graph).
    pub fn is_empty(&self) -> bool {
        self.heads.is_empty()
    }

    /// Encode as a compact postcard byte string for wire transmission.
    ///
    /// Used in `hello`/`welcome` messages. B1 (IBF) will extend this to
    /// carry the full IBF structure alongside the frontier.
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("Frontier is always serializable")
    }

    /// Decode from postcard bytes produced by `encode()`.
    pub fn decode(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }

    /// Encode the heads as a JSON array of hex strings for the WebSocket
    /// `hello`/`welcome` envelope (the outer message stays JSON per the
    /// wire protocol spec — only the inner node pack uses postcard).
    pub fn to_hex_vec(&self) -> Vec<String> {
        self.heads.iter().map(|h| h.to_hex()).collect()
    }

    /// Reconstruct a `Frontier` from a JSON hex array (as received in hello).
    pub fn from_hex_vec(hexes: &[String]) -> Self {
        let heads: Vec<NodeId> = hexes
            .iter()
            .filter_map(|h| parse_hex_hash(h))
            .collect();
        Frontier::from_heads(heads)
    }
}

fn parse_hex_hash(hex: &str) -> Option<NodeId> {
    if hex.len() != 64 { return None; }
    let mut bytes = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        bytes[i] = (hi << 4) | lo;
    }
    Some(crate::hash::Hash(bytes))
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StateGraph, Op, MapOp};
    use ed25519_dalek::SigningKey;

    fn key() -> SigningKey { SigningKey::from_bytes(&[0x0Au8; 32]) }

    fn set(k: &str) -> Op {
        Op::Map(MapOp::Set { key: k.into(), value: b"v".to_vec() })
    }

    #[test]
    fn empty_frontier_is_empty() {
        let f = Frontier::default();
        assert!(f.is_empty());
        assert_eq!(f.len(), 0);
    }

    #[test]
    fn frontier_tracks_single_head() {
        let mut g = StateGraph::new();
        let k = key();
        g.apply_local(&k, 0, vec![set("a")]).unwrap();

        let f = g.frontier();
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn frontier_advances_on_new_node() {
        let mut g = StateGraph::new();
        let k = key();
        g.apply_local(&k, 0, vec![set("a")]).unwrap();
        let f1 = g.frontier();

        g.apply_local(&k, 1, vec![set("b")]).unwrap();
        let f2 = g.frontier();

        // Second node extends the first → frontier still has 1 head (the new tip).
        assert_eq!(f1.len(), 1);
        assert_eq!(f2.len(), 1);
        assert_ne!(f1.heads, f2.heads);
    }

    #[test]
    fn frontier_encodes_and_decodes() {
        let mut g = StateGraph::new();
        let k = key();
        g.apply_local(&k, 0, vec![set("x")]).unwrap();
        g.apply_local(&k, 1, vec![set("y")]).unwrap();

        let f = g.frontier();
        let bytes = f.encode();
        let decoded = Frontier::decode(&bytes).unwrap();
        assert_eq!(f, decoded);
    }

    #[test]
    fn frontier_hex_roundtrip() {
        let mut g = StateGraph::new();
        let k = key();
        g.apply_local(&k, 0, vec![set("a")]).unwrap();

        let f = g.frontier();
        let hexes = f.to_hex_vec();
        let restored = Frontier::from_hex_vec(&hexes);
        assert_eq!(f, restored);
    }

    #[test]
    fn two_concurrent_nodes_give_two_heads() {
        use crate::{SyncNode, op::Transaction};
        let ka = SigningKey::from_bytes(&[0x0Au8; 32]);
        let kb = SigningKey::from_bytes(&[0x0Bu8; 32]);

        // Two nodes with no parents — concurrent roots.
        let tx_a = Transaction {
            author: ka.verifying_key().to_bytes(),
            lamport: 1, wall_ms: 0,
            ops: vec![set("x")],
            parents: vec![],
        };
        let tx_b = Transaction {
            author: kb.verifying_key().to_bytes(),
            lamport: 1, wall_ms: 0,
            ops: vec![set("y")],
            parents: vec![],
        };
        let node_a = SyncNode::new_signed(tx_a, &ka);
        let node_b = SyncNode::new_signed(tx_b, &kb);

        let mut g = StateGraph::new();
        g.apply_remote(node_a).unwrap();
        g.apply_remote(node_b).unwrap();

        let f = g.frontier();
        assert_eq!(f.len(), 2);
    }
}
