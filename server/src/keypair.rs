//! Persistent server Ed25519 keypair (E1).
//!
//! On first start the server generates a random 32-byte seed, writes it to
//! `server.key` in the current working directory, and derives a `SigningKey`
//! from it.  On subsequent starts the same seed is loaded and the same keypair
//! is recovered.  The public key appears in log output so operators can pin it
//! in room policies.
//!
//! # File format
//! Raw 32-byte seed, no framing.  Simple and unambiguous.

use std::{fs, io};
use ed25519_dalek::SigningKey;

const KEY_FILE: &str = "server.key";

/// Load the server keypair from `server.key`, or generate and persist a new one.
pub fn load_or_generate() -> SigningKey {
    match load() {
        Ok(sk) => {
            let hex = hex(&sk.verifying_key().to_bytes());
            tracing::info!(pubkey = %&hex[..16], "loaded server keypair");
            sk
        }
        Err(_) => {
            let sk = generate();
            let hex = hex(&sk.verifying_key().to_bytes());
            tracing::info!(pubkey = %&hex[..16], "generated new server keypair");
            sk
        }
    }
}

fn load() -> io::Result<SigningKey> {
    let bytes = fs::read(KEY_FILE)?;
    if bytes.len() != 32 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "key file must be 32 bytes"));
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes);
    Ok(SigningKey::from_bytes(&seed))
}

fn generate() -> SigningKey {
    use rand_core::OsRng;
    let sk = SigningKey::generate(&mut OsRng);
    let seed: &[u8] = sk.as_bytes();
    if let Err(e) = fs::write(KEY_FILE, seed) {
        tracing::warn!(error = %e, "could not persist server.key");
    }
    sk
}

/// Return the 64-char hex of the server verifying key, for use in policies.
pub fn pubkey_hex(sk: &SigningKey) -> String {
    hex(&sk.verifying_key().to_bytes())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
