//! Cryptographic capability tokens for room access (C3).
//!
//! A token authorises exactly one peer (identified by their Ed25519 public key)
//! to join exactly one room until an expiry timestamp.  It is signed by the
//! room admin's Ed25519 keypair.
//!
//! # Architecture note
//!
//! **Room = transport / connection scope.**  The token answers "who may connect."
//! **Policy (A5) = authority.**  Once connected, `Policy` rules answer "what may
//! they read or write" by matching op key-paths.
//!
//! The `capabilities` field carries optional path-scoped grants that the
//! Super-Peer (E1) will enforce.  An empty `capabilities` list means full
//! access, which is the current default.  The field is included in the signed
//! message so it cannot be tampered with after issuance.
//!
//! # Signed message layout
//! ```text
//! DOMAIN || 0x00 || room_id_utf8 || 0x00 || peer_pubkey_32 || expiry_u64_le
//!        || 0x00 || caps_sorted_newline_joined
//! ```
//! `caps_sorted_newline_joined` is the capability strings sorted
//! lexicographically and joined by `'\n'`.  An empty list produces an empty
//! byte sequence after the final `0x00`.  Sorting ensures the signature is
//! deterministic regardless of insertion order.
//!
//! # Wire format (inside `hello.token`)
//! ```json
//! { "peer_pubkey": "<64 hex>", "expiry": <u64>,
//!   "caps": ["read:world/**", "write:intent/**"], "sig": "<128 hex>" }
//! ```

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use thiserror::Error;

const DOMAIN: &[u8] = b"activesync-room-token-v1";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TokenError {
    #[error("token has expired")]
    Expired,
    #[error("signature verification failed")]
    BadSignature,
    #[error("token peer_pubkey does not match connecting peer")]
    PeerMismatch,
    #[error("malformed token field: {0}")]
    Malformed(String),
}

/// A signed capability token granting one peer access to one room.
#[derive(Debug, Clone)]
pub struct RoomToken {
    /// 32-byte Ed25519 public key of the peer being granted access.
    pub peer_pubkey: [u8; 32],
    /// Unix timestamp (seconds); token is invalid at or after this time.
    pub expiry_secs: u64,
    /// Optional path-scoped grants, e.g. `"read:world/**"`, `"write:intent/**"`.
    ///
    /// Empty = full access (current default).  These are carried in the signed
    /// message so they cannot be tampered with.  Enforcement is deferred to E1
    /// (Super-Peer) — for now the server records them per-connection.
    pub capabilities: Vec<String>,
    /// Ed25519 signature by the room's signing key.
    pub signature: [u8; 64],
}

impl RoomToken {
    /// Parse from wire fields (JSON `hello.token.*`).
    /// `peer_pubkey_hex`: 64 hex chars; `sig_hex`: 128 hex chars.
    /// `capabilities`: list of path-scoped grant strings; may be empty.
    pub fn from_wire(
        peer_pubkey_hex: &str,
        expiry_secs: u64,
        capabilities: Vec<String>,
        sig_hex: &str,
    ) -> Result<Self, TokenError> {
        let peer_pubkey = hex_to_array::<32>(peer_pubkey_hex)
            .ok_or_else(|| TokenError::Malformed("peer_pubkey".into()))?;
        let sig_bytes = hex_to_array::<64>(sig_hex)
            .ok_or_else(|| TokenError::Malformed("sig".into()))?;
        Ok(RoomToken { peer_pubkey, expiry_secs, capabilities, signature: sig_bytes })
    }

    /// Sign a new token authorizing `peer_pubkey` to join `room_id`.
    ///
    /// `capabilities`: optional path-scoped grants.  Pass `&[]` for full access
    /// (equivalent to no restrictions — current default behaviour).
    pub fn sign(
        room_id: &str,
        peer_pubkey: &[u8; 32],
        expiry_secs: u64,
        capabilities: &[String],
        signing_key: &SigningKey,
    ) -> Self {
        let mut sorted_caps: Vec<String> = capabilities.to_vec();
        sorted_caps.sort_unstable();
        let msg = token_message(room_id, peer_pubkey, expiry_secs, &sorted_caps);
        let sig = signing_key.sign(&msg);
        RoomToken {
            peer_pubkey: *peer_pubkey,
            expiry_secs,
            capabilities: sorted_caps,
            signature: sig.to_bytes(),
        }
    }

    /// Verify this token.
    ///
    /// Checks (in order):
    /// 1. `connecting_peer_pubkey` matches the token's `peer_pubkey`.
    /// 2. `now_secs < expiry_secs` (not yet expired).
    /// 3. Signature is valid under `room_verifying_key` (covers capabilities too).
    pub fn verify(
        &self,
        room_id: &str,
        room_verifying_key: &VerifyingKey,
        connecting_peer_pubkey: &[u8; 32],
        now_secs: u64,
    ) -> Result<(), TokenError> {
        if self.peer_pubkey != *connecting_peer_pubkey {
            return Err(TokenError::PeerMismatch);
        }
        if now_secs >= self.expiry_secs {
            return Err(TokenError::Expired);
        }
        // Caps must be sorted when signing; sort again here in case the token
        // was constructed directly (e.g. in tests).
        let mut sorted_caps = self.capabilities.clone();
        sorted_caps.sort_unstable();
        let msg = token_message(room_id, &self.peer_pubkey, self.expiry_secs, &sorted_caps);
        let sig = Signature::from_bytes(&self.signature);
        room_verifying_key.verify(&msg, &sig).map_err(|_| TokenError::BadSignature)
    }

    /// Return the signature as a 128-char hex string for wire transmission.
    pub fn sig_hex(&self) -> String {
        bytes_to_hex(&self.signature)
    }

    /// Return `peer_pubkey` as a 64-char hex string.
    pub fn peer_pubkey_hex(&self) -> String {
        bytes_to_hex(&self.peer_pubkey)
    }
}

/// Build the canonical byte string that is signed and verified.
///
/// `sorted_caps` must already be sorted lexicographically — callers are
/// responsible for sorting before passing here.
///
/// Encoding:
/// ```text
/// DOMAIN || 0x00 || room_id_utf8 || 0x00 || peer_pubkey_32 || expiry_u64_le
///        || 0x00 || cap0 || 0x0A || cap1 || 0x0A || …
/// ```
/// (`0x0A` = `'\n'` used as separator between capability strings)
pub fn token_message(
    room_id: &str,
    peer_pubkey: &[u8; 32],
    expiry_secs: u64,
    sorted_caps: &[String],
) -> Vec<u8> {
    let caps_bytes: Vec<u8> = sorted_caps.join("\n").into_bytes();
    let mut msg =
        Vec::with_capacity(DOMAIN.len() + 1 + room_id.len() + 1 + 32 + 8 + 1 + caps_bytes.len());
    msg.extend_from_slice(DOMAIN);
    msg.push(0x00);
    msg.extend_from_slice(room_id.as_bytes());
    msg.push(0x00);
    msg.extend_from_slice(peer_pubkey);
    msg.extend_from_slice(&expiry_secs.to_le_bytes());
    msg.push(0x00);
    msg.extend_from_slice(&caps_bytes);
    msg
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn hex_to_array<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 {
        return None;
    }
    let mut bytes = [0u8; N];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        bytes[i] = (hi << 4) | lo;
    }
    Some(bytes)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn room_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn peer_pk(seed: u8) -> [u8; 32] {
        SigningKey::from_bytes(&[seed; 32]).verifying_key().to_bytes()
    }

    #[test]
    fn sign_and_verify() {
        let rk = room_key(1);
        let peer = peer_pk(2);
        let token = RoomToken::sign("my-room", &peer, u64::MAX, &[], &rk);
        assert!(token.verify("my-room", &rk.verifying_key(), &peer, 0).is_ok());
    }

    #[test]
    fn sign_and_verify_with_caps() {
        let rk = room_key(1);
        let peer = peer_pk(2);
        let caps = vec!["write:intent/**".to_string(), "read:world/**".to_string()];
        let token = RoomToken::sign("my-room", &peer, u64::MAX, &caps, &rk);
        // Caps are sorted at sign time
        assert_eq!(token.capabilities, vec!["read:world/**", "write:intent/**"]);
        assert!(token.verify("my-room", &rk.verifying_key(), &peer, 0).is_ok());
    }

    #[test]
    fn tampered_caps_rejected() {
        let rk = room_key(1);
        let peer = peer_pk(2);
        let caps = vec!["read:world/**".to_string()];
        let mut token = RoomToken::sign("my-room", &peer, u64::MAX, &caps, &rk);
        // Attacker upgrades read to write — signature must not verify
        token.capabilities = vec!["write:world/**".to_string()];
        assert!(matches!(
            token.verify("my-room", &rk.verifying_key(), &peer, 0),
            Err(TokenError::BadSignature)
        ));
    }

    #[test]
    fn expired() {
        let rk = room_key(1);
        let peer = peer_pk(2);
        let token = RoomToken::sign("my-room", &peer, 100, &[], &rk);
        assert!(matches!(
            token.verify("my-room", &rk.verifying_key(), &peer, 101),
            Err(TokenError::Expired)
        ));
    }

    #[test]
    fn expiry_boundary() {
        let rk = room_key(1);
        let peer = peer_pk(2);
        let token = RoomToken::sign("my-room", &peer, 100, &[], &rk);
        // At exactly expiry_secs the token is invalid (>=)
        assert!(matches!(
            token.verify("my-room", &rk.verifying_key(), &peer, 100),
            Err(TokenError::Expired)
        ));
        // One second before is still valid
        assert!(token.verify("my-room", &rk.verifying_key(), &peer, 99).is_ok());
    }

    #[test]
    fn peer_mismatch() {
        let rk = room_key(1);
        let token = RoomToken::sign("my-room", &peer_pk(2), u64::MAX, &[], &rk);
        assert!(matches!(
            token.verify("my-room", &rk.verifying_key(), &peer_pk(3), 0),
            Err(TokenError::PeerMismatch)
        ));
    }

    #[test]
    fn wrong_room() {
        let rk = room_key(1);
        let peer = peer_pk(2);
        let token = RoomToken::sign("room-a", &peer, u64::MAX, &[], &rk);
        assert!(matches!(
            token.verify("room-b", &rk.verifying_key(), &peer, 0),
            Err(TokenError::BadSignature)
        ));
    }

    #[test]
    fn wrong_key() {
        let rk_a = room_key(1);
        let rk_b = room_key(99);
        let peer = peer_pk(2);
        let token = RoomToken::sign("my-room", &peer, u64::MAX, &[], &rk_a);
        assert!(matches!(
            token.verify("my-room", &rk_b.verifying_key(), &peer, 0),
            Err(TokenError::BadSignature)
        ));
    }

    #[test]
    fn from_wire_roundtrip() {
        let rk = room_key(42);
        let peer = peer_pk(7);
        let caps = vec!["read:world/**".to_string(), "write:intent/**".to_string()];
        let token = RoomToken::sign("test-room", &peer, 9_999_999, &caps, &rk);
        let parsed = RoomToken::from_wire(
            &token.peer_pubkey_hex(),
            token.expiry_secs,
            token.capabilities.clone(),
            &token.sig_hex(),
        )
        .unwrap();
        assert!(parsed.verify("test-room", &rk.verifying_key(), &peer, 0).is_ok());
    }

    #[test]
    fn malformed_rejected() {
        assert!(RoomToken::from_wire("not-hex", 0, vec![], "also-not-hex").is_err());
        assert!(RoomToken::from_wire(&"ab".repeat(32), 0, vec![], "short-sig").is_err());
    }
}
