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
