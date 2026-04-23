//! Scoped Write Policies (A5).
//!
//! A `Policy` governs which public keys may write, read, or derive state for
//! a given key-path pattern within a room. It is attached to a room via a
//! genesis-level node and enforced locally by every peer in `apply_remote`.
//!
//! # Enforcement model
//! - `can_write` is enforced NOW (A5): any node whose author is not permitted
//!   to write a matched key is rejected by `apply_remote`.
//! - `can_read` is parsed and stored but NOT YET enforced — enforcement
//!   requires E2EE (D1).
//! - `can_derive` is parsed and stored but NOT YET enforced — enforcement
//!   requires Super-Peer compute authority (E1).
//!
//! # Glob syntax
//! Patterns use a minimal subset of glob matching:
//! - `*`  matches any sequence of characters that does not contain `/`.
//! - `**` matches any sequence of characters including `/`.
//! - All other characters match literally.
//!
//! Rules are evaluated in order; the first matching rule wins. If no rule
//! matches, `Policy::default` applies (`AllowAll` or `DenyAll`).

use serde::{Deserialize, Serialize};

/// An Ed25519 verifying key (public key), 32 bytes.
pub type PublicKey = [u8; 32];

/// What happens when no `PolicyRule` matches an op key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyDefault {
    /// Any signer may write (current open-room behaviour).
    AllowAll,
    /// No signer may write unless explicitly permitted by a rule.
    DenyAll,
}

/// A single access-control rule, evaluated against a key-path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    /// Glob pattern matched against the op key (e.g. `"world/**"`, `"intent/*"`).
    pub path_glob: String,
    /// Public keys permitted to write (sign ops) to matching paths.
    pub can_write: Vec<PublicKey>,
    /// Public keys permitted to read (decrypt) values at matching paths.
    /// Parsed and stored; enforcement deferred to D1 (E2EE).
    pub can_read: Vec<PublicKey>,
    /// Public keys permitted to emit derived / computed state from matching paths.
    /// Parsed and stored; enforcement deferred to E1 (Super-Peer compute).
    pub can_derive: Vec<PublicKey>,
}

/// Room-level access control policy.
///
/// Rules are evaluated in order; the first match wins. A room with no policy
/// (i.e. `Policy::default()`) behaves identically to the pre-A5 open-room
/// model — any signer may write anything.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// Rules evaluated in order; first match wins.
    pub rules: Vec<PolicyRule>,
    /// Fallback when no rule matches.
    pub default: PolicyDefault,
}

impl Default for Policy {
    /// The zero-config policy: no rules, allow all writes. Identical to
    /// pre-A5 behaviour so existing rooms are unaffected.
    fn default() -> Self {
        Policy { rules: Vec::new(), default: PolicyDefault::AllowAll }
    }
}

impl Policy {
    /// Returns `true` if `author` is permitted to write to `key`.
    ///
    /// Walks `self.rules` in order. The first rule whose `path_glob` matches
    /// `key` determines the answer: the author must appear in `can_write`.
    /// If no rule matches, falls back to `self.default`.
    pub fn can_write(&self, key: &str, author: &PublicKey) -> bool {
        for rule in &self.rules {
            if glob_matches(&rule.path_glob, key) {
                return rule.can_write.contains(author);
            }
        }
        self.default == PolicyDefault::AllowAll
    }
}

// ---------------------------------------------------------------------------
// Glob matching
// ---------------------------------------------------------------------------

/// Minimal glob matcher supporting `*` (no `/`) and `**` (any chars).
///
/// Implemented iteratively to avoid recursion depth issues on long patterns.
fn glob_matches(pattern: &str, text: &str) -> bool {
    glob_matches_inner(pattern.as_bytes(), text.as_bytes())
}

fn glob_matches_inner(mut pat: &[u8], mut txt: &[u8]) -> bool {
    loop {
        match pat.first() {
            None => return txt.is_empty(),

            // `**` — matches any sequence of characters including `/`
            Some(b'*') if pat.get(1) == Some(&b'*') => {
                pat = &pat[2..];
                // Skip any trailing separator after `**`
                if pat.first() == Some(&b'/') {
                    pat = &pat[1..];
                }
                // Try matching from every position in txt
                loop {
                    if glob_matches_inner(pat, txt) {
                        return true;
                    }
                    if txt.is_empty() {
                        return false;
                    }
                    txt = &txt[1..];
                }
            }

            // `*` — matches any sequence of non-`/` characters
            Some(b'*') => {
                pat = &pat[1..];
                loop {
                    if glob_matches_inner(pat, txt) {
                        return true;
                    }
                    if txt.is_empty() || txt[0] == b'/' {
                        return false;
                    }
                    txt = &txt[1..];
                }
            }

            // Literal character match
            Some(&pc) => {
                match txt.first() {
                    Some(&tc) if tc == pc => {
                        pat = &pat[1..];
                        txt = &txt[1..];
                    }
                    _ => return false,
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const AUTHOR_A: PublicKey = [1u8; 32];
    const AUTHOR_B: PublicKey = [2u8; 32];

    fn rule(glob: &str, writers: Vec<PublicKey>) -> PolicyRule {
        PolicyRule {
            path_glob: glob.to_string(),
            can_write: writers,
            can_read:  Vec::new(),
            can_derive: Vec::new(),
        }
    }

    // --- glob tests ---

    #[test]
    fn glob_exact_match() {
        assert!(glob_matches("foo", "foo"));
        assert!(!glob_matches("foo", "bar"));
    }

    #[test]
    fn glob_single_star_matches_segment() {
        assert!(glob_matches("intent/*", "intent/move"));
        assert!(glob_matches("intent/*", "intent/jump"));
        assert!(!glob_matches("intent/*", "intent/nested/deep"));
        assert!(!glob_matches("intent/*", "other/move"));
    }

    #[test]
    fn glob_double_star_matches_any_depth() {
        assert!(glob_matches("world/**", "world/player1/pos"));
        assert!(glob_matches("world/**", "world/a/b/c/d"));
        assert!(glob_matches("world/**", "world/x"));
        assert!(!glob_matches("world/**", "other/x"));
    }

    #[test]
    fn glob_double_star_at_root() {
        assert!(glob_matches("**", "anything/at/all"));
        assert!(glob_matches("**", "simple"));
    }

    // --- policy enforcement tests ---

    #[test]
    fn open_policy_allows_anyone() {
        let policy = Policy::default();
        assert!(policy.can_write("any/key", &AUTHOR_A));
        assert!(policy.can_write("any/key", &AUTHOR_B));
    }

    #[test]
    fn deny_all_default_blocks_unmatched() {
        let policy = Policy { rules: Vec::new(), default: PolicyDefault::DenyAll };
        assert!(!policy.can_write("anything", &AUTHOR_A));
    }

    #[test]
    fn rule_allows_specific_author() {
        let policy = Policy {
            rules: vec![rule("world/**", vec![AUTHOR_A])],
            default: PolicyDefault::DenyAll,
        };
        assert!(policy.can_write("world/player/pos", &AUTHOR_A));
        assert!(!policy.can_write("world/player/pos", &AUTHOR_B));
    }

    #[test]
    fn rule_first_match_wins() {
        // First rule: only A can write "world/**"
        // Second rule (never reached for "world/…"): everyone can write "**"
        let policy = Policy {
            rules: vec![
                rule("world/**", vec![AUTHOR_A]),
                rule("**",       vec![AUTHOR_A, AUTHOR_B]),
            ],
            default: PolicyDefault::DenyAll,
        };
        // world/** matched by first rule — B is blocked
        assert!(policy.can_write("world/pos", &AUTHOR_A));
        assert!(!policy.can_write("world/pos", &AUTHOR_B));
        // intent/move not matched by first rule → falls through to "**" → both allowed
        assert!(policy.can_write("intent/move", &AUTHOR_A));
        assert!(policy.can_write("intent/move", &AUTHOR_B));
    }

    #[test]
    fn wildcard_rule_allows_all_writers() {
        let policy = Policy {
            rules: vec![rule("**", vec![AUTHOR_A, AUTHOR_B])],
            default: PolicyDefault::DenyAll,
        };
        assert!(policy.can_write("any/path/here", &AUTHOR_A));
        assert!(policy.can_write("any/path/here", &AUTHOR_B));
    }

    #[test]
    fn single_star_does_not_cross_slash() {
        let policy = Policy {
            rules: vec![rule("intent/*", vec![AUTHOR_A])],
            default: PolicyDefault::DenyAll,
        };
        assert!(policy.can_write("intent/move", &AUTHOR_A));
        // "intent/nested/deep" has a slash — `*` won't match it
        assert!(!policy.can_write("intent/nested/deep", &AUTHOR_A));
        // Unrelated path falls through to DenyAll
        assert!(!policy.can_write("world/pos", &AUTHOR_A));
    }
}
