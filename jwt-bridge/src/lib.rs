//! # activesync-jwt-bridge
//!
//! Trusted-issuer JWT → `RoomToken` minting. Plug your app's existing auth
//! provider (Clerk, Supabase, Auth0, a homegrown issuer — anything that signs
//! JWTs) in front of ActiveSync without teaching ActiveSync about identity.
//!
//! ## Flow
//!
//! 1. Your auth server issues a JWT whose claims embed the room id, the
//!    connecting peer's Ed25519 public key (hex), the ActiveSync token
//!    expiry, and optional capability strings. It signs the JWT with a secret
//!    (HS256) or private key (RS256 / ES256) *you* own.
//! 2. A small service (usually colocated with `activesync-server`) receives
//!    the JWT from the client, calls [`mint_room_token`], and returns the
//!    resulting `RoomToken` wire fields to the client.
//! 3. The client puts those fields into its `hello` message exactly like a
//!    [`RoomToken`] it signed locally — the server has no idea a JWT was
//!    involved.
//!
//! The bridge owns one secret (the JWT verifier) and one key (the room's
//! Ed25519 signing key). Identity vs. capability stays cleanly separated:
//! the JWT answers "who is asking", the `RoomToken` answers "what they may
//! do inside this room".
//!
//! ## Example
//!
//! ```
//! use activesync_jwt_bridge::{mint_room_token, BridgeConfig, JwtVerifier};
//! use ed25519_dalek::SigningKey;
//! use jsonwebtoken::{encode, EncodingKey, Header};
//! use serde_json::json;
//!
//! // Secret shared with your JWT issuer.
//! let jwt_secret = b"super-secret-issuer-key";
//! // The room's Ed25519 signing key — lives on your auth/bridge service.
//! let room_key = SigningKey::from_bytes(&[0x42u8; 32]);
//!
//! // Your auth server signs a JWT like this:
//! let claims = json!({
//!     "room":   "game-42",
//!     "pubkey": "aa".repeat(32),
//!     "caps":   ["read:world/**", "write:intent/**"],
//!     "exp":    2_000_000_000u64,      // RoomToken & JWT share this
//! });
//! let jwt = encode(
//!     &Header::default(),
//!     &claims,
//!     &EncodingKey::from_secret(jwt_secret),
//! ).unwrap();
//!
//! // Your bridge service verifies & mints:
//! let cfg = BridgeConfig {
//!     verifier: JwtVerifier::hs256(jwt_secret),
//!     room_key,
//!     allowed_issuers: vec![],
//!     allowed_audiences: vec![],
//! };
//! let token = mint_room_token(&cfg, &jwt).unwrap();
//! assert_eq!(token.capabilities.len(), 2);
//! ```

use activesync_core::{RoomToken, TokenError};
use ed25519_dalek::SigningKey;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use thiserror::Error;

mod capability_profile;
use capability_profile::maybe_expand_minted_capabilities;

// ─── Errors ────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("jwt verification failed: {0}")]
    BadJwt(#[from] jsonwebtoken::errors::Error),
    #[error("jwt is missing required claim: {0}")]
    MissingClaim(&'static str),
    #[error("jwt claim has wrong type or shape: {0}")]
    BadClaim(&'static str),
    #[error("could not mint RoomToken: {0}")]
    RoomToken(#[from] TokenError),
    #[error("capability profile expansion failed: {0}")]
    CapabilityProfile(String),
}

// ─── Config ────────────────────────────────────────────────────────────────

/// How to verify incoming JWTs.
#[derive(Clone)]
pub struct JwtVerifier {
    pub(crate) key: DecodingKey,
    pub(crate) validation: Validation,
}

impl JwtVerifier {
    /// HMAC-SHA256 with a shared secret (simplest — good for homegrown issuers).
    pub fn hs256(secret: &[u8]) -> Self {
        Self {
            key: DecodingKey::from_secret(secret),
            validation: Validation::new(Algorithm::HS256),
        }
    }

    /// RSA-SHA256 against a PEM-encoded public key (Auth0, Clerk, Supabase…).
    pub fn rs256_from_pem(pem: &[u8]) -> Result<Self, jsonwebtoken::errors::Error> {
        Ok(Self {
            key: DecodingKey::from_rsa_pem(pem)?,
            validation: Validation::new(Algorithm::RS256),
        })
    }

    /// ECDSA-P256-SHA256 against a PEM-encoded public key.
    pub fn es256_from_pem(pem: &[u8]) -> Result<Self, jsonwebtoken::errors::Error> {
        Ok(Self {
            key: DecodingKey::from_ec_pem(pem)?,
            validation: Validation::new(Algorithm::ES256),
        })
    }
}

/// Bridge-wide configuration. In production, instantiate once at startup.
pub struct BridgeConfig {
    /// How to verify the inbound JWT signature + standard claims.
    pub verifier: JwtVerifier,
    /// The room's Ed25519 signing key (the one that locked the room).
    pub room_key: SigningKey,
    /// If non-empty, require the JWT `iss` claim to appear in this list.
    pub allowed_issuers: Vec<String>,
    /// If non-empty, require the JWT `aud` claim to appear in this list.
    pub allowed_audiences: Vec<String>,
}

// ─── Claim shape ───────────────────────────────────────────────────────────

/// Shape of the JWT claims the bridge understands. Unknown fields are
/// ignored; standard JWT fields (`exp`, `nbf`, `iss`, `aud`) are validated by
/// the `jsonwebtoken` crate itself based on the configured `Validation`.
#[derive(Debug, Deserialize)]
struct BridgeClaims {
    /// Required. The ActiveSync room id.
    room: String,
    /// Required. The connecting peer's Ed25519 pubkey, 64 hex chars.
    pubkey: String,
    /// Required. Unix epoch seconds. Used for BOTH the JWT `exp` check and
    /// the `RoomToken.expiry_secs` field. (They are the same clock.)
    exp: u64,
    /// Optional. Path-scoped grants e.g. `["read:world/**", "write:intent/**"]`.
    #[serde(default)]
    caps: Vec<String>,
    /// Optional. Host capability profile version expected by this token.
    #[serde(default)]
    capability_profile_version: Option<String>,
}

// ─── Minting ───────────────────────────────────────────────────────────────

/// Verify a JWT and mint an ActiveSync `RoomToken`.
///
/// On success, the returned `RoomToken` can be put directly into a `hello`
/// message by the client (serialize fields to the expected JSON shape —
/// typically `peer_pubkey_hex` + `expiry_secs` + `capabilities` + `sig_hex`).
pub fn mint_room_token(cfg: &BridgeConfig, jwt: &str) -> Result<RoomToken, BridgeError> {
    // 1. Apply issuer/audience allow-lists if configured. Build a local copy
    //    of the validator so BridgeConfig stays shareable across threads.
    let mut validation = cfg.verifier.validation.clone();
    if !cfg.allowed_issuers.is_empty() {
        validation.set_issuer(&cfg.allowed_issuers);
    }
    if !cfg.allowed_audiences.is_empty() {
        validation.set_audience(&cfg.allowed_audiences);
    }

    // 2. Verify signature + standard claims (`exp`, `nbf`, `iss`, `aud`).
    let data = decode::<BridgeClaims>(jwt, &cfg.verifier.key, &validation)?;
    let claims = data.claims;

    // 3. Pull out the ActiveSync-specific bits.
    let pubkey = hex_to_32(&claims.pubkey)
        .ok_or(BridgeError::BadClaim("pubkey (expected 64 hex chars)"))?;

    let caps = maybe_expand_minted_capabilities(
        &claims.caps,
        claims.capability_profile_version.as_deref(),
    )
    .map_err(BridgeError::CapabilityProfile)?;

    // 4. Mint. The RoomToken signature binds `room | pubkey | expiry | caps`
    //    with the room's Ed25519 key — identical to what `RoomToken::sign`
    //    would produce locally. Downstream server-side verification doesn't
    //    know or care that the trigger was a JWT.
    Ok(RoomToken::sign(
        &claims.room,
        &pubkey,
        claims.exp,
        &caps,
        &cfg.room_key,
    ))
}

// ─── Helpers ───────────────────────────────────────────────────────────────

fn hex_to_32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 { return None; }
    let mut out = [0u8; 32];
    let b = s.as_bytes();
    for i in 0..32 {
        out[i] = (hex_digit(b[2 * i])? << 4) | hex_digit(b[2 * i + 1])?;
    }
    Some(out)
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::VerifyingKey;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::json;

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
    }

    fn setup() -> (Vec<u8>, SigningKey, VerifyingKey) {
        let secret = b"test-secret".to_vec();
        let room_key = SigningKey::from_bytes(&[0x11u8; 32]);
        let room_vk = room_key.verifying_key();
        (secret, room_key, room_vk)
    }

    fn issue(secret: &[u8], claims: serde_json::Value) -> String {
        encode(&Header::default(), &claims, &EncodingKey::from_secret(secret)).unwrap()
    }

    #[test]
    fn happy_path_mints_valid_room_token() {
        let (secret, room_key, room_vk) = setup();
        let peer_key = SigningKey::from_bytes(&[0x22u8; 32]);
        let peer_hex: String = peer_key.verifying_key().to_bytes()
            .iter().map(|b| format!("{:02x}", b)).collect();
        let exp = now() + 3600;

        let jwt = issue(&secret, json!({
            "room":   "r1",
            "pubkey": peer_hex,
            "caps":   ["write:intent/**"],
            "exp":    exp,
        }));

        let cfg = BridgeConfig {
            verifier: JwtVerifier::hs256(&secret),
            room_key,
            allowed_issuers: vec![],
            allowed_audiences: vec![],
        };

        let token = mint_room_token(&cfg, &jwt).unwrap();
        // Token must be verifiable with the room's public key — proves the
        // bridge signed with the right key and the right preimage.
        let peer_bytes = peer_key.verifying_key().to_bytes();
        token.verify(&"r1", &room_vk, &peer_bytes, now()).unwrap();
        assert_eq!(token.capabilities, vec!["write:intent/**".to_string()]);
    }

    #[test]
    fn expired_jwt_is_rejected() {
        let (secret, room_key, _) = setup();
        let jwt = issue(&secret, json!({
            "room": "r1",
            "pubkey": "aa".repeat(32),
            "exp": now() - 120,
        }));
        let cfg = BridgeConfig {
            verifier: JwtVerifier::hs256(&secret),
            room_key,
            allowed_issuers: vec![],
            allowed_audiences: vec![],
        };
        assert!(matches!(mint_room_token(&cfg, &jwt), Err(BridgeError::BadJwt(_))));
    }

    #[test]
    fn wrong_secret_is_rejected() {
        let (secret, room_key, _) = setup();
        let jwt = issue(&secret, json!({
            "room": "r1",
            "pubkey": "aa".repeat(32),
            "exp": now() + 3600,
        }));
        let cfg = BridgeConfig {
            verifier: JwtVerifier::hs256(b"DIFFERENT-SECRET"),
            room_key,
            allowed_issuers: vec![],
            allowed_audiences: vec![],
        };
        assert!(matches!(mint_room_token(&cfg, &jwt), Err(BridgeError::BadJwt(_))));
    }

    #[test]
    fn missing_pubkey_fails_with_bad_jwt() {
        // `jsonwebtoken` treats missing required fields as a decode error —
        // they surface as `BadJwt`, not `MissingClaim`, because the JWT layer
        // rejects before our struct ever materializes.
        let (secret, room_key, _) = setup();
        let jwt = issue(&secret, json!({
            "room": "r1",
            "exp":  now() + 3600,
        }));
        let cfg = BridgeConfig {
            verifier: JwtVerifier::hs256(&secret),
            room_key,
            allowed_issuers: vec![],
            allowed_audiences: vec![],
        };
        assert!(mint_room_token(&cfg, &jwt).is_err());
    }

    #[test]
    fn malformed_pubkey_is_rejected() {
        let (secret, room_key, _) = setup();
        let jwt = issue(&secret, json!({
            "room": "r1",
            "pubkey": "not-hex",
            "exp": now() + 3600,
        }));
        let cfg = BridgeConfig {
            verifier: JwtVerifier::hs256(&secret),
            room_key,
            allowed_issuers: vec![],
            allowed_audiences: vec![],
        };
        assert!(matches!(
            mint_room_token(&cfg, &jwt),
            Err(BridgeError::BadClaim(_)),
        ));
    }

    #[test]
    fn issuer_allow_list_is_enforced() {
        let (secret, room_key, _) = setup();
        let jwt = issue(&secret, json!({
            "room":   "r1",
            "pubkey": "aa".repeat(32),
            "exp":    now() + 3600,
            "iss":    "https://auth.example.com",
        }));
        let cfg = BridgeConfig {
            verifier: JwtVerifier::hs256(&secret),
            room_key,
            allowed_issuers: vec!["https://other.example.com".into()],
            allowed_audiences: vec![],
        };
        assert!(mint_room_token(&cfg, &jwt).is_err());
    }

    #[test]
    fn caps_default_to_empty() {
        let (secret, room_key, _) = setup();
        let jwt = issue(&secret, json!({
            "room":   "r1",
            "pubkey": "aa".repeat(32),
            "exp":    now() + 3600,
        }));
        let cfg = BridgeConfig {
            verifier: JwtVerifier::hs256(&secret),
            room_key,
            allowed_issuers: vec![],
            allowed_audiences: vec![],
        };
        let tok = mint_room_token(&cfg, &jwt).unwrap();
        assert!(tok.capabilities.is_empty());
    }
}
