use serde::{Deserialize, Serialize};
use crate::hash::Hash;
use crate::list::FracIdx;
use crate::text_range::TextRangeAnchor;

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

/// Stable identity of a list item (UUIDv4, 16 random bytes).
///
/// Independent of position — survives moves. The inserter generates a fresh
/// random `ItemId` per item; collisions are cosmologically unlikely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ItemId(pub [u8; 16]);

impl ItemId {
    /// Format as a 32-char lowercase hex string. Used by the bridge as a
    /// dictionary key on the JS side.
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(32);
        for b in &self.0 {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    /// Parse a 32-char hex string. Returns `None` on malformed input.
    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 32 {
            return None;
        }
        let mut bytes = [0u8; 16];
        for i in 0..16 {
            bytes[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(ItemId(bytes))
    }
}

/// F8: Fractional-index list operations.
///
/// `Insert` and `Move` are LWW per `(item_id, list_key)` on `(lamport, author)`.
/// `Delete` is absorbing — once observed, all later/concurrent ops on the
/// same item id lose. See [`crate::list`] for the resolution algorithm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ListOp {
    /// Add `item_id` to the list at `position`. The item's content lives in
    /// a sidecar Map; the SDK manages that composition.
    Insert {
        list_key: String,
        item_id: ItemId,
        position: FracIdx,
    },
    /// Tombstone `item_id`. Idempotent and absorbing.
    Delete {
        list_key: String,
        item_id: ItemId,
    },
    /// Reposition an existing item. LWW with Insert on `(lamport, author)`.
    Move {
        list_key: String,
        item_id: ItemId,
        position: FracIdx,
    },
}

impl ListOp {
    /// Key this op acts on.
    pub fn key(&self) -> &str {
        match self {
            ListOp::Insert { list_key, .. }
            | ListOp::Delete { list_key, .. }
            | ListOp::Move   { list_key, .. } => list_key,
        }
    }
}

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
    /// Insert a text span at an anchor. Runtime paths lower this to
    /// deterministic one-char inserts.
    InsertRange {
        key: String,
        anchor: TextRangeAnchor,
        text: String,
    },
    /// Delete a visible character span from an anchor. Runtime paths lower
    /// this to deterministic one-char deletes.
    DeleteRange {
        key: String,
        anchor: TextRangeAnchor,
        len_chars: usize,
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
    ///
    /// The hash is over the canonical `postcard` serialization — the same
    /// format we use on the wire. `postcard` gives us a format-stable,
    /// allocation-light binary encoding (3.46× smaller and 1.83× faster to
    /// produce than `serde_json`), and it removes a latent footgun where a
    /// future `serde_json` formatting change could silently invalidate
    /// every existing signature.
    pub fn hash(&self) -> crate::hash::Hash {
        let bytes = postcard::to_allocvec(self).expect("Transaction is always serializable");
        Hash::of(&bytes)
    }
}

impl MapOp {
    /// Key this op acts on.
    pub fn key(&self) -> &str {
        match self {
            MapOp::Set { key, .. } | MapOp::Delete { key } | MapOp::SetBlob { key, .. } => key,
        }
    }
}

impl TextOp {
    /// Key this op acts on.
    pub fn key(&self) -> &str {
        match self {
            TextOp::Insert { key, .. }
            | TextOp::Delete { key, .. }
            | TextOp::InsertRange { key, .. }
            | TextOp::DeleteRange { key, .. } => key,
        }
    }
}

impl Op {
    /// Path-key this op targets. Every current op variant carries a key.
    pub fn key(&self) -> Option<&str> {
        match self {
            Op::Map(m) => Some(m.key()),
            Op::Text(t) => Some(t.key()),
            Op::List(l) => Some(l.key()),
        }
    }
}
