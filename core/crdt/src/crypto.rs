//! End-to-End Encryption primitives (D1).
//!
//! # Design
//! A room's symmetric key is derived from the room's Ed25519 seed (the same
//! 32-byte secret used to sign capability tokens in C3) using HKDF-SHA256.
//! Only peers that know the room seed can decrypt; the server sees opaque
//! ciphertext and cannot read op values.
//!
//! # Wire format
//! An encrypted node carries a single sentinel op:
//!   `Op::Map(MapOp::Set { key: E2EE_KEY, value: nonce_12 || ciphertext })`
//!
//! The ciphertext is the AES-256-GCM encryption of the postcard-serialized
//! `Vec<Op>` that the node would normally carry in plaintext.
//!
//! The nonce is derived deterministically from the room key + author pubkey +
//! Lamport clock, so no extra randomness source is needed in WASM.
//!
//! # Invariants
//! - `node.id` (= `transaction.hash()`) covers the encrypted payload.
//!   Integrity is preserved end-to-end through the existing Ed25519 + Blake3
//!   chain; the server verifies signatures without decrypting.
//! - Policy checks in `apply_remote` run against `E2EE_KEY`. In AllowAll
//!   rooms this is always permitted. In Authoritative rooms the policy must
//!   explicitly allow the sentinel key (or use DenyAll, which blocks everyone).

use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use hkdf::Hkdf;
use sha2::Sha256;

use crate::{SyncError, op::{MapOp, Op}};

/// The sentinel key used for encrypted op payloads.
/// The null-byte prefix makes it impossible to collide with user key-paths.
pub const E2EE_KEY: &str = "\x00e2ee";

// ---------------------------------------------------------------------------
// Key derivation
// ---------------------------------------------------------------------------

/// Derive a 32-byte AES-256-GCM key from a room Ed25519 seed.
///
/// Uses HKDF-SHA256 with a fixed info string so the derivation is
/// domain-separated from any other use of the seed.
pub fn derive_room_key(room_seed: &[u8; 32]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(None, room_seed);
    let mut key = [0u8; 32];
    hk.expand(b"nodalmerge-e2ee-v1", &mut key)
        .expect("32-byte output is always valid for HKDF-SHA256");
    key
}

// ---------------------------------------------------------------------------
// Nonce derivation
// ---------------------------------------------------------------------------

/// Derive a 12-byte AES-GCM nonce from the room key, author pubkey, and
/// Lamport clock.  Deterministic: same inputs always produce the same nonce.
///
/// Safety: AES-GCM nonce reuse under the same key is catastrophic.  It is
/// safe here because `(room_key, author_32, lamport)` is globally unique:
/// - `room_key` is unique per room.
/// - `(author_32, lamport)` is unique per node (each author's Lamport clock
///   strictly increases, and it is never reset).
fn derive_nonce(room_key: &[u8; 32], author: &[u8; 32], lamport: u64) -> [u8; 12] {
    let hk = Hkdf::<Sha256>::new(None, room_key);
    let mut info = Vec::with_capacity(32 + 8 + 14);
    info.extend_from_slice(b"nodalmerge-nonce-v1");
    info.extend_from_slice(author);
    info.extend_from_slice(&lamport.to_le_bytes());
    let mut nonce = [0u8; 12];
    hk.expand(&info, &mut nonce)
        .expect("12-byte output is always valid for HKDF-SHA256");
    nonce
}

// ---------------------------------------------------------------------------
// Encrypt / decrypt
// ---------------------------------------------------------------------------

/// Encrypt a set of ops for a node authored by `author` at Lamport clock
/// `lamport`.  Returns `nonce_12 || ciphertext` as a single byte vector.
///
/// Pass the returned bytes as the `value` of a sentinel `E2EE_KEY` op.
pub fn encrypt_ops(
    room_key: &[u8; 32],
    author: &[u8; 32],
    lamport: u64,
    ops: &[Op],
) -> Result<Vec<u8>, SyncError> {
    let nonce_bytes = derive_nonce(room_key, author, lamport);
    let plaintext = postcard::to_allocvec(ops)
        .map_err(|e| SyncError::Serialization(e.to_string()))?;
    let cipher = Aes256Gcm::new_from_slice(room_key)
        .expect("room_key is always 32 bytes");
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_slice())
        .map_err(|_| SyncError::EncryptionFailed)?;
    let mut out = Vec::with_capacity(12 + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt an E2EE payload (produced by `encrypt_ops`).
///
/// `payload` must be `nonce_12 || ciphertext`.  Returns the decrypted
/// `Vec<Op>` on success, or an error if the key is wrong or data is corrupt.
pub fn decrypt_ops(room_key: &[u8; 32], payload: &[u8]) -> Result<Vec<Op>, SyncError> {
    if payload.len() < 12 {
        return Err(SyncError::DecryptionFailed);
    }
    let (nonce_bytes, ciphertext) = payload.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(room_key)
        .expect("room_key is always 32 bytes");
    let nonce = Nonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| SyncError::DecryptionFailed)?;
    let ops: Vec<Op> = postcard::from_bytes(&plaintext)
        .map_err(|e| SyncError::Serialization(e.to_string()))?;
    Ok(ops)
}

// ---------------------------------------------------------------------------
// Node inspection helpers
// ---------------------------------------------------------------------------

/// Return `true` if `ops` is exactly one E2EE sentinel op.
pub fn is_encrypted_node(ops: &[Op]) -> bool {
    matches!(
        ops,
        [Op::Map(MapOp::Set { key, .. })] if key == E2EE_KEY
    )
}

/// Extract the encrypted payload from a slice of ops that `is_encrypted_node`
/// returned `true` for.
pub fn extract_encrypted_payload(ops: &[Op]) -> Option<&[u8]> {
    if let [Op::Map(MapOp::Set { key, value })] = ops {
        if key == E2EE_KEY {
            return Some(value.as_slice());
        }
    }
    None
}

/// Wrap `ops` inside a single sentinel op ready for a `Transaction`.
pub fn wrap_encrypted_ops(
    room_key: &[u8; 32],
    author: &[u8; 32],
    lamport: u64,
    ops: &[Op],
) -> Result<Vec<Op>, SyncError> {
    let payload = encrypt_ops(room_key, author, lamport, ops)?;
    Ok(vec![Op::Map(MapOp::Set {
        key: E2EE_KEY.to_string(),
        value: payload,
    })])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::op::MapOp;

    fn room_seed() -> [u8; 32] { [0xAB; 32] }
    fn author()    -> [u8; 32] { [0x01; 32] }
    fn sample_ops() -> Vec<Op> {
        vec![
            Op::Map(MapOp::Set { key: "hello".into(), value: b"world".to_vec() }),
            Op::Map(MapOp::Delete { key: "gone".into() }),
        ]
    }

    #[test]
    fn derive_room_key_deterministic() {
        let seed = room_seed();
        let k1 = derive_room_key(&seed);
        let k2 = derive_room_key(&seed);
        assert_eq!(k1, k2);
    }

    #[test]
    fn derive_room_key_different_seeds_differ() {
        let k1 = derive_room_key(&[0xAB; 32]);
        let k2 = derive_room_key(&[0xCD; 32]);
        assert_ne!(k1, k2);
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = derive_room_key(&room_seed());
        let ops = sample_ops();
        let payload = encrypt_ops(&key, &author(), 42, &ops).unwrap();
        let recovered = decrypt_ops(&key, &payload).unwrap();
        assert_eq!(ops, recovered);
    }

    #[test]
    fn wrong_key_fails_decryption() {
        let key1 = derive_room_key(&[0xAB; 32]);
        let key2 = derive_room_key(&[0xCD; 32]);
        let payload = encrypt_ops(&key1, &author(), 1, &sample_ops()).unwrap();
        assert!(decrypt_ops(&key2, &payload).is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let key = derive_room_key(&room_seed());
        let mut payload = encrypt_ops(&key, &author(), 7, &sample_ops()).unwrap();
        let last = payload.len() - 1;
        payload[last] ^= 0xFF; // flip a bit in the auth tag
        assert!(decrypt_ops(&key, &payload).is_err());
    }

    #[test]
    fn nonce_unique_per_lamport() {
        let key = derive_room_key(&room_seed());
        let p1 = encrypt_ops(&key, &author(), 1, &sample_ops()).unwrap();
        let p2 = encrypt_ops(&key, &author(), 2, &sample_ops()).unwrap();
        // Nonces are the first 12 bytes — must differ.
        assert_ne!(&p1[..12], &p2[..12]);
    }

    #[test]
    fn nonce_unique_per_author() {
        let key = derive_room_key(&room_seed());
        let a1: [u8; 32] = [0x01; 32];
        let a2: [u8; 32] = [0x02; 32];
        let p1 = encrypt_ops(&key, &a1, 1, &sample_ops()).unwrap();
        let p2 = encrypt_ops(&key, &a2, 1, &sample_ops()).unwrap();
        assert_ne!(&p1[..12], &p2[..12]);
    }

    #[test]
    fn is_encrypted_node_true_for_sentinel() {
        let key = derive_room_key(&room_seed());
        let wrapped = wrap_encrypted_ops(&key, &author(), 3, &sample_ops()).unwrap();
        assert!(is_encrypted_node(&wrapped));
    }

    #[test]
    fn is_encrypted_node_false_for_plaintext() {
        let ops = sample_ops();
        assert!(!is_encrypted_node(&ops));
    }

    #[test]
    fn wrap_and_unwrap_roundtrip() {
        let key = derive_room_key(&room_seed());
        let ops = sample_ops();
        let wrapped = wrap_encrypted_ops(&key, &author(), 5, &ops).unwrap();
        let payload = extract_encrypted_payload(&wrapped).unwrap();
        let recovered = decrypt_ops(&key, payload).unwrap();
        assert_eq!(ops, recovered);
    }

    #[test]
    fn payload_too_short_returns_error() {
        let key = derive_room_key(&room_seed());
        assert!(decrypt_ops(&key, &[0u8; 5]).is_err());
    }
}
