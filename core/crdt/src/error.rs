use thiserror::Error;
use crate::hash::Hash;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: Hash, actual: Hash },

    #[error("missing parent node: {0}")]
    MissingParent(Hash),

    #[error("duplicate node: {0}")]
    DuplicateNode(Hash),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("blob not found: {0}")]
    BlobNotFound(Hash),

    #[error("invalid ed25519 signature on node {0}")]
    InvalidSignature(Hash),

    #[error("policy violation: author {author:?} not permitted to write key '{key}'")]
    PolicyViolation { author: [u8; 32], key: String },

    #[error("encryption failed")]
    EncryptionFailed,

    #[error("decryption failed — wrong key or corrupt ciphertext")]
    DecryptionFailed,

    /// G5: node's Lamport clock is farther ahead than the local clock
    /// plus `graph::LAMPORT_SLACK`. A well-behaved concurrent fan-out
    /// never comes close to the slack window; crossing it is almost
    /// certainly a malformed or malicious node.
    #[error("lamport ceiling exceeded on node {id}: lamport={lamport}, ceiling={ceiling}")]
    LamportCeiling { id: Hash, lamport: u64, ceiling: u64 },

    /// G5: node's `wall_ms` is further in the future than
    /// `graph::WALL_SKEW_MAX_MS` past the caller-supplied `now_ms`.
    /// `wall_ms` is informational — this only guards the "sort to top
    /// of the display timeline" class of abuse for apps that render
    /// by wall clock.
    #[error("wall clock skew on node {id}: wall_ms={wall_ms} exceeds now={now_ms} by more than 24h")]
    WallClockSkew { id: Hash, wall_ms: u64, now_ms: u64 },
}

impl From<serde_json::Error> for SyncError {
    fn from(e: serde_json::Error) -> Self {
        SyncError::Serialization(e.to_string())
    }
}

impl From<postcard::Error> for SyncError {
    fn from(e: postcard::Error) -> Self {
        SyncError::Serialization(e.to_string())
    }
}
