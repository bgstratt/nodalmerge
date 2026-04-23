use serde::{Deserialize, Serialize};
use crate::hash::Hash;

/// Stable identity of a character in the RGA text sequence.
///
/// Each `TextOp::Insert` transaction carries exactly one insert per key,
/// so `(lamport, author)` uniquely identifies the character across all peers.
/// Higher `lamport` (then `author`) means higher priority — that character
/// appears leftward when two insertions target the same predecessor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OpId {
    pub lamport: u64,
    pub author: [u8; 32],
}

impl PartialOrd for OpId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OpId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.lamport, self.author).cmp(&(other.lamport, other.author))
    }
}

/// Operations on a LWW-Map key namespace.
/// Migrated from flat `Op` variants in A2; see `Op::Map`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MapOp {
    /// Set a key to a value.
    /// Value is raw bytes — callers encode their types (JSON, msgpack, etc.).
    Set { key: String, value: Vec<u8> },
    /// Remove a key from the LWW-Map.
    Delete { key: String },
    /// Reference a large blob stored in the CAS by its hash.
    /// Use this instead of inline bytes for payloads > ~4 KB.
    SetBlob { key: String, blob_hash: Hash },
}

/// Stub for Phase C (RGA list CRDT). No variants yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ListOp {}

/// RGA text operations (Phase C1).
///
/// Each `Insert` transaction must carry exactly one `TextOp::Insert` per key.
/// The character's stable RGA identity is the containing transaction's
/// `(lamport, author)` pair — accessible as `OpId { lamport: tx.lamport, author: tx.author }`.
///
/// Deletions tombstone the target character: it is removed from the visible
/// sequence but its position remains in the DAG so concurrent insertions
/// around it stay stable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TextOp {
    /// Insert `ch` after the character identified by `after`, or at the
    /// start of the sequence when `after` is `None`.
    Insert {
        key: String,
        after: Option<OpId>,
        ch: char,
    },
    /// Tombstone the character with RGA identity `target`.
    Delete {
        key: String,
        target: OpId,
    },
}

/// A single mutation to the shared state.
/// Operations are the atomic unit of change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    /// LWW-Map operation (the only active variant until Phase C).
    Map(MapOp),
    /// Reserved for Phase C (RGA list CRDT).
    List(ListOp),
    /// Reserved for Phase C (RGA text CRDT).
    Text(TextOp),
}

/// A signed, Merkle-linked batch of operations.
/// This is the fundamental unit written to the DAG.
///
/// The hash of a Transaction is computed over its canonical serialized form,
/// making it content-addressable. Transactions are immutable once created.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    /// Ed25519 public key (32 bytes) identifying the author.
    pub author: [u8; 32],
    /// Lamport clock value from the author's perspective.
    /// Used for deterministic tie-breaking during merges.
    pub lamport: u64,
    /// Wall-clock timestamp in milliseconds since Unix epoch.
    /// Informational only — lamport is used for ordering.
    pub wall_ms: u64,
    /// Operations in this transaction (applied atomically).
    pub ops: Vec<Op>,
    /// Merkle links to the parent transactions this one extends.
    /// Empty only for the genesis (first) transaction in a graph.
    pub parents: Vec<Hash>,
}

impl Transaction {
    /// Compute the content-addressable hash of this transaction.
    /// The hash is over the canonical JSON serialization.
    pub fn hash(&self) -> crate::hash::Hash {
        let bytes = serde_json::to_vec(self).expect("Transaction is always serializable");
        Hash::of(&bytes)
    }
}
