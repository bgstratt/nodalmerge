//! S5.3 — config-driven `AdminPinStore`.
//!
//! Simplest honest implementation of the frozen `AdminPinStore` contract:
//! a fixed set of hex hashes supplied at startup (env var and/or repeated
//! CLI flag), never mutated at runtime. Good enough for "protect this one
//! blob a human cares about" — a live admin API is future work, not needed
//! to satisfy `docs/delegated-storage-gc.md`'s "Pin/lease protections
//! override deletion eligibility" safety rule.

use std::collections::HashSet;

use nodalmerge_gc::contracts::AdminPinStore;
use nodalmerge_gc::GcResult;

#[derive(Debug, Default)]
pub struct StaticPinStore {
    pinned: HashSet<String>,
}

impl StaticPinStore {
    pub fn new(pinned: impl IntoIterator<Item = String>) -> Self {
        Self {
            pinned: pinned.into_iter().map(|h| h.to_lowercase()).collect(),
        }
    }

    /// Parse `NODALMERGE_GC_ADMIN_PINS` — a comma-separated list of hex
    /// blob hashes — plus any `--gc-admin-pin <hash>` CLI flags (repeatable).
    /// Both sources are unioned; neither is required.
    pub fn from_env_and_args(args: &[String]) -> Self {
        let mut pinned: HashSet<String> = HashSet::new();
        if let Ok(env_list) = std::env::var("NODALMERGE_GC_ADMIN_PINS") {
            for tok in env_list.split(',') {
                let tok = tok.trim();
                if !tok.is_empty() {
                    pinned.insert(tok.to_lowercase());
                }
            }
        }
        let mut i = 1;
        while i < args.len() {
            let a = &args[i];
            if a == "--gc-admin-pin" {
                if let Some(v) = args.get(i + 1) {
                    pinned.insert(v.to_lowercase());
                }
            } else if let Some(v) = a.strip_prefix("--gc-admin-pin=") {
                pinned.insert(v.to_lowercase());
            }
            i += 1;
        }
        Self { pinned }
    }
}

impl AdminPinStore for StaticPinStore {
    fn is_pinned(&self, hash: &str) -> GcResult<bool> {
        Ok(self.pinned.contains(&hash.to_lowercase()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_and_flag_pins_are_unioned() {
        std::env::set_var("NODALMERGE_GC_ADMIN_PINS", "AAAA,bbbb");
        let args = vec![
            "nodalmerge-server".to_string(),
            "--gc-admin-pin".to_string(),
            "CCCC".to_string(),
        ];
        let store = StaticPinStore::from_env_and_args(&args);
        assert!(store.is_pinned("aaaa").unwrap());
        assert!(store.is_pinned("bbbb").unwrap());
        assert!(store.is_pinned("cccc").unwrap());
        assert!(!store.is_pinned("dddd").unwrap());
        std::env::remove_var("NODALMERGE_GC_ADMIN_PINS");
    }
}
