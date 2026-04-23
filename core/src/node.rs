use serde::{Deserialize, Deserializer, Serialize, Serializer};
use crate::{error::SyncError, hash::Hash, op::{Op, Transaction}};

/// Unique identifier for a node in the sync graph.
pub type NodeId = Hash;

/// Ed25519 signature (64 bytes), serialized as a 128-char hex string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature(pub [u8; 64]);

impl Signature {
    pub fn zero() -> Self { Signature([0u8; 64]) }
    pub fn is_zero(&self) -> bool { self.0 == [0u8; 64] }
    pub fn to_bytes(&self) -> [u8; 64] { self.0 }
}

impl Serialize for Signature {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let hex: String = self.0.iter().map(|b| format!("{:02x}", b)).collect();
        s.serialize_str(&hex)
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let hex = String::deserialize(d)?;
        if hex.len() != 128 {
            return Err(serde::de::Error::custom("signature must be 128 hex chars"));
        }
        let mut bytes = [0u8; 64];
        for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
            let hi = hex_nibble(chunk[0]).ok_or_else(|| serde::de::Error::custom("bad hex"))?;
            let lo = hex_nibble(chunk[1]).ok_or_else(|| serde::de::Error::custom("bad hex"))?;
            bytes[i] = (hi << 4) | lo;
        }
        Ok(Signature(bytes))
    }
}

impl Default for Signature {
    fn default() -> Self { Signature::zero() }
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// A node in the Sync-Graph DAG.
///
/// The `signature` field holds an Ed25519 signature over `id.as_bytes()`,
/// signed by the private key corresponding to `transaction.author` (the
/// Ed25519 public key). Zero-signature nodes are treated as unsigned/legacy
/// and accepted without verification; the server enforces non-zero signatures.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncNode {
    /// Content-addressable ID — equals `transaction.hash()`.
    pub id: NodeId,
    /// The transaction payload carried by this node.
    pub transaction: Transaction,
    /// Ed25519 signature over `id.as_bytes()`. Zero = unsigned (legacy).
    #[serde(default)]
    pub signature: Signature,
}

impl SyncNode {
    /// Construct an *unsigned* node from a transaction.
    pub fn new(transaction: Transaction) -> Self {
        let id = transaction.hash();
        SyncNode { id, transaction, signature: Signature::zero() }
    }

    /// Construct a node and sign it with the given Ed25519 signing key.
    pub fn new_signed(
        transaction: Transaction,
        signing_key: &ed25519_dalek::SigningKey,
    ) -> Self {
        use ed25519_dalek::Signer;
        let id = transaction.hash();
        let sig = signing_key.sign(id.as_bytes());
        SyncNode { id, transaction, signature: Signature(sig.to_bytes()) }
    }

    /// Verify the signature. Returns `Ok(())` for unsigned (zero) nodes.
    pub fn verify_signature(&self) -> Result<(), crate::error::SyncError> {
        use ed25519_dalek::Verifier;
        if self.signature.is_zero() {
            return Ok(());
        }
        let vk = ed25519_dalek::VerifyingKey::from_bytes(&self.transaction.author)
            .map_err(|_| crate::error::SyncError::InvalidSignature(self.id))?;
        let sig = ed25519_dalek::Signature::from_bytes(&self.signature.to_bytes());
        vk.verify(self.id.as_bytes(), &sig)
            .map_err(|_| crate::error::SyncError::InvalidSignature(self.id))
    }

    /// Parent hashes — convenience accessor into the transaction.
    pub fn parents(&self) -> &[Hash] {
        &self.transaction.parents
    }

    /// Lamport clock — convenience accessor.
    pub fn lamport(&self) -> u64 {
        self.transaction.lamport
    }

    /// Encode this node to compact postcard binary for wire transport.
    /// Uses raw bytes for the signature (64 B) instead of the hex-string JSON
    /// representation (128 chars), and binary-encodes all other fields.
    pub fn to_postcard(&self) -> Vec<u8> {
        let wire = WireNode::from(self);
        postcard::to_allocvec(&wire).expect("WireNode is always serializable")
    }

    /// Decode a node from postcard binary produced by `to_postcard`.
    pub fn from_postcard(bytes: &[u8]) -> Result<Self, SyncError> {
        let wire: WireNode = postcard::from_bytes(bytes)?;
        Ok(SyncNode::from(wire))
    }
}

// =============================================================================
// Wire format — compact postcard representation
// =============================================================================

/// Postcard-optimised shadow of `SyncNode`.
/// The only difference from `SyncNode` is `signature: [[u8; 32]; 2]` — raw bytes
/// split into two 32-byte halves (serde derives don't support [T; 64]) instead of
/// the hex-string serde impl on `Signature`, saving ~64 bytes/node on the wire.
#[derive(Serialize, Deserialize)]
struct WireNode {
    id:          [u8; 32],
    transaction: WireTx,
    /// Ed25519 signature as two 32-byte halves: [lo, hi]
    signature:   [[u8; 32]; 2],
}

#[derive(Serialize, Deserialize)]
struct WireTx {
    author:   [u8; 32],
    lamport:  u64,
    wall_ms:  u64,
    ops:      Vec<Op>,
    parents:  Vec<Hash>, // Hash derives Serialize → 32 raw bytes in postcard
}

impl From<&SyncNode> for WireNode {
    fn from(n: &SyncNode) -> Self {
        let sig = n.signature.0;
        WireNode {
            id: n.id.0,
            transaction: WireTx {
                author:  n.transaction.author,
                lamport: n.transaction.lamport,
                wall_ms: n.transaction.wall_ms,
                ops:     n.transaction.ops.clone(),
                parents: n.transaction.parents.clone(),
            },
            signature: [
                sig[..32].try_into().expect("sig lo"),
                sig[32..].try_into().expect("sig hi"),
            ],
        }
    }
}

impl From<WireNode> for SyncNode {
    fn from(w: WireNode) -> Self {
        let [lo, hi] = w.signature;
        let mut sig = [0u8; 64];
        sig[..32].copy_from_slice(&lo);
        sig[32..].copy_from_slice(&hi);
        SyncNode {
            id: Hash(w.id),
            transaction: Transaction {
                author:  w.transaction.author,
                lamport: w.transaction.lamport,
                wall_ms: w.transaction.wall_ms,
                ops:     w.transaction.ops,
                parents: w.transaction.parents,
            },
            signature: Signature(sig),
        }
    }
}

/// Encode a slice of node references to a single postcard byte string.
/// This is what goes in the `nodes` field of every `pack` WebSocket message.
pub fn pack_nodes(nodes: &[&SyncNode]) -> Vec<u8> {
    let wires: Vec<WireNode> = nodes.iter().map(|n| WireNode::from(*n)).collect();
    postcard::to_allocvec(&wires).expect("Vec<WireNode> is always serializable")
}

/// Decode a postcard byte string produced by `pack_nodes`.
pub fn unpack_nodes(bytes: &[u8]) -> Result<Vec<SyncNode>, SyncError> {
    let wires: Vec<WireNode> = postcard::from_bytes(bytes)?;
    Ok(wires.into_iter().map(SyncNode::from).collect())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::op::{MapOp, Op, Transaction};
    use ed25519_dalek::SigningKey;

    fn make_nodes(n: u64) -> Vec<SyncNode> {
        let key = SigningKey::from_bytes(&[0x42u8; 32]);
        (1..=n).map(|i| {
            let tx = Transaction {
                author:  key.verifying_key().to_bytes(),
                lamport: i,
                wall_ms: 1_700_000_000_000 + i,
                ops: vec![Op::Map(MapOp::Set {
                    key:   format!("key{i}"),
                    value: format!("value{i}").into_bytes(),
                })],
                parents: vec![],
            };
            SyncNode::new_signed(tx, &key)
        }).collect()
    }

    #[test]
    fn pack_unpack_roundtrip() {
        let nodes = make_nodes(5);
        let refs: Vec<&SyncNode> = nodes.iter().collect();
        let bytes = pack_nodes(&refs);
        let decoded = unpack_nodes(&bytes).unwrap();
        assert_eq!(decoded.len(), nodes.len());
        for (orig, dec) in nodes.iter().zip(decoded.iter()) {
            assert_eq!(orig.id,                          dec.id);
            assert_eq!(orig.transaction.lamport,         dec.transaction.lamport);
            assert_eq!(orig.transaction.ops,             dec.transaction.ops);
            assert_eq!(orig.signature.0,                 dec.signature.0);
        }
    }

    #[test]
    fn postcard_at_least_50_percent_smaller_than_json() {
        let nodes = make_nodes(10);
        let refs: Vec<&SyncNode> = nodes.iter().collect();
        let pc_bytes  = pack_nodes(&refs);
        let json_bytes = serde_json::to_vec(&nodes).unwrap();
        assert!(
            pc_bytes.len() * 2 < json_bytes.len(),
            "postcard ({} B) should be <50% of JSON ({} B)",
            pc_bytes.len(), json_bytes.len()
        );
    }
}
