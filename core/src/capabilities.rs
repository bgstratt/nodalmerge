//! Sync capability negotiation (A7).
//!
//! Each peer advertises which protocol features it supports in the `hello`
//! message. The server computes the intersection and echoes the negotiated set
//! in `welcome`. Both sides then operate using only features in the intersection.
//!
//! # Extensibility rule
//! Every field is `#[serde(default)]`. New capabilities added in future phases
//! are invisible to older peers — they simply get `false`/`None` and fall back
//! to the baseline behaviour. No version bump, no breaking change.
//!
//! # Current defaults
//! `supports_postcard` defaults to `true` because all current peers use the
//! postcard wire format (A3). Every other capability defaults to `false`.

use serde::{Deserialize, Serialize};

/// Feature flags exchanged in `hello` / `welcome`.
///
/// Both the client and server include a `caps` field.  The server responds
/// with the **intersection** so both sides know exactly which features are
/// active for this session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncCapabilities {
    /// Invertible Bloom Filter set-reconciliation (B1).
    /// When true the peer can send/receive IBF handshake messages.
    pub supports_ibf: bool,

    /// Merkle Search Tree sync (B2).
    /// Requires `supports_ibf` to be true on both sides first.
    pub supports_mst: bool,

    /// postcard binary node packs (A3).
    /// Always `true` for current peers — included so future mixed-format
    /// environments can detect a pure-JSON fallback peer.
    pub supports_postcard: bool,

    /// End-to-end encryption (D1).
    /// Op bytes are AES-256-GCM encrypted before signing.
    pub supports_encryption: bool,

    /// WebRTC data-channel transport (D2).
    /// Peer can negotiate a direct data channel for node and blob transfer.
    pub supports_webrtc: bool,

    /// Maximum tick-batching interval this peer will accept, in milliseconds (E3).
    /// `None` means the peer does not support tick batching.
    pub max_tick_interval_ms: Option<u64>,
}

impl Default for SyncCapabilities {
    fn default() -> Self {
        SyncCapabilities {
            supports_ibf:         true,   // B1 implemented
            supports_mst:         true,    // B2 implemented
            supports_postcard:    true,  // all current peers use postcard (A3)
            supports_encryption:  true,   // D1 implemented
            supports_webrtc:      false,
            max_tick_interval_ms: None,
        }
    }
}

impl SyncCapabilities {
    /// Compute the **intersection** of two capability sets.
    ///
    /// The result is the set of features that *both* peers have confirmed
    /// support for. Pass this to the JS/server layer as the active feature set
    /// for the session.
    pub fn intersect(&self, other: &SyncCapabilities) -> SyncCapabilities {
        SyncCapabilities {
            supports_ibf:         self.supports_ibf         && other.supports_ibf,
            supports_mst:         self.supports_mst         && other.supports_mst,
            supports_postcard:    self.supports_postcard    && other.supports_postcard,
            supports_encryption:  self.supports_encryption  && other.supports_encryption,
            supports_webrtc:      self.supports_webrtc      && other.supports_webrtc,
            max_tick_interval_ms: match (self.max_tick_interval_ms, other.max_tick_interval_ms) {
                (Some(a), Some(b)) => Some(a.max(b)), // use the larger of the two intervals
                _ => None,                             // either peer doesn't support batching
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_caps_postcard_true_rest_false() {
        let c = SyncCapabilities::default();
        assert!(c.supports_postcard);
        assert!(c.supports_ibf);          // B1 implemented
        assert!(c.supports_mst);          // B2 implemented
        assert!(c.supports_encryption);   // D1 implemented
        assert!(!c.supports_webrtc);
        assert!(c.max_tick_interval_ms.is_none());
    }

    #[test]
    fn intersect_both_support_ibf() {
        let a = SyncCapabilities { supports_ibf: true, ..Default::default() };
        let b = SyncCapabilities { supports_ibf: true, ..Default::default() };
        let n = a.intersect(&b);
        assert!(n.supports_ibf);
    }

    #[test]
    fn intersect_one_missing_ibf_disables_it() {
        let a = SyncCapabilities { supports_ibf: true,  ..Default::default() };
        let b = SyncCapabilities { supports_ibf: false, ..Default::default() };
        assert!(!a.intersect(&b).supports_ibf);
        assert!(!b.intersect(&a).supports_ibf);
    }

    #[test]
    fn intersect_postcard_requires_both() {
        let a = SyncCapabilities { supports_postcard: false, ..Default::default() };
        let b = SyncCapabilities::default(); // supports_postcard: true
        let n = a.intersect(&b);
        assert!(!n.supports_postcard);
    }

    #[test]
    fn intersect_tick_interval_uses_larger() {
        let a = SyncCapabilities { max_tick_interval_ms: Some(16),  ..Default::default() };
        let b = SyncCapabilities { max_tick_interval_ms: Some(32),  ..Default::default() };
        assert_eq!(a.intersect(&b).max_tick_interval_ms, Some(32));
    }

    #[test]
    fn intersect_tick_interval_none_if_either_absent() {
        let a = SyncCapabilities { max_tick_interval_ms: Some(16), ..Default::default() };
        let b = SyncCapabilities::default(); // None
        assert!(a.intersect(&b).max_tick_interval_ms.is_none());
    }

    #[test]
    fn serde_missing_field_defaults() {
        // A client that omits unknown or all capability fields gets Default values.
        let json = r#"{}"#;
        let caps: SyncCapabilities = serde_json::from_str(json).unwrap();
        assert_eq!(caps, SyncCapabilities::default());
    }

    #[test]
    fn serde_partial_caps_roundtrip() {
        let caps = SyncCapabilities { supports_ibf: true, ..Default::default() };
        let json = serde_json::to_string(&caps).unwrap();
        let decoded: SyncCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(caps, decoded);
    }
}
