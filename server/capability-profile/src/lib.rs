//! Shared capability-composition (CAPCOMP) profile loading and DAG
//! expansion.
//!
//! A capability profile is a small inheritance DAG over capability tokens
//! (`room.admin` inherits `tick.admin`, `policy.admin`, …). Given a set of
//! *assigned* capabilities and a claimed profile version, [`flatten_capabilities`]
//! expands the DAG into the full, deduplicated, sorted set of effective
//! capabilities that should be embedded in a minted `RoomToken`.
//!
//! This crate is the single source of truth for that algorithm — it is
//! consumed by both `nodalmerge-server` (inbound `hello` token validation)
//! and `nodalmerge-jwt-bridge` (RoomToken minting from a third-party JWT).
//! A parallel implementation lives in the .NET host
//! (`NodalMerge.Host.Composition.CapabilityProfileExpander`); the two are
//! kept in lockstep by the shared vectors file
//! `engine/commands/capcomp-vectors.v1.json` — see
//! `docs/CAPCOMP_PARITY_PLAN.md`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::Path;

pub const DEFAULT_MAX_CAPABILITY_COUNT: usize = 128;
pub const DEFAULT_MAX_CAPABILITY_LENGTH: usize = 128;
pub const DEFAULT_MAX_FLATTENED_PAYLOAD_BYTES: usize = 8 * 1024;
pub const DEFAULT_MAX_DAG_DEPTH: usize = 16;
pub const DEFAULT_MAX_EDGES_PER_NODE: usize = 64;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CapabilityProfile {
    pub profile_version: String,
    #[serde(default)]
    pub supported_profile_versions: Vec<String>,
    pub nodes: Vec<CapabilityNode>,
    #[serde(default)]
    pub limits: Option<CapabilityProfileLimits>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CapabilityNode {
    pub capability: String,
    #[serde(default)]
    pub inherits: Vec<String>,
}

/// Every field defaults independently when omitted from a profile's
/// `limits` object — matching the .NET expander, whose
/// `CapabilityProfileLimits` properties each carry their own default. A
/// `limits` object may therefore specify any subset of fields; unspecified
/// ones fall back individually rather than requiring the whole object.
///
/// These are the values *as parsed*: the file format accepts anything, but
/// resolution clamps each field to its `DEFAULT_MAX_*` ceiling — see
/// [`resolved_limits`]. A profile may lower a limit, never raise one.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CapabilityProfileLimits {
    #[serde(default = "default_max_capability_count")]
    pub max_capability_count: usize,
    #[serde(default = "default_max_capability_length")]
    pub max_capability_length: usize,
    #[serde(default = "default_max_flattened_payload_bytes")]
    pub max_flattened_payload_bytes: usize,
    #[serde(default = "default_max_dag_depth")]
    pub max_dag_depth: usize,
    #[serde(default = "default_max_edges_per_node")]
    pub max_edges_per_node: usize,
}

fn default_max_capability_count() -> usize {
    DEFAULT_MAX_CAPABILITY_COUNT
}

fn default_max_capability_length() -> usize {
    DEFAULT_MAX_CAPABILITY_LENGTH
}

fn default_max_flattened_payload_bytes() -> usize {
    DEFAULT_MAX_FLATTENED_PAYLOAD_BYTES
}

fn default_max_dag_depth() -> usize {
    DEFAULT_MAX_DAG_DEPTH
}

fn default_max_edges_per_node() -> usize {
    DEFAULT_MAX_EDGES_PER_NODE
}

impl Default for CapabilityProfileLimits {
    fn default() -> Self {
        Self {
            max_capability_count: DEFAULT_MAX_CAPABILITY_COUNT,
            max_capability_length: DEFAULT_MAX_CAPABILITY_LENGTH,
            max_flattened_payload_bytes: DEFAULT_MAX_FLATTENED_PAYLOAD_BYTES,
            max_dag_depth: DEFAULT_MAX_DAG_DEPTH,
            max_edges_per_node: DEFAULT_MAX_EDGES_PER_NODE,
        }
    }
}

/// A CAPCOMP failure, with a stable, cross-runtime [`class`](Self::class)
/// string used by the parity vectors harnesses. Display text is free to
/// carry human-friendly context and is *not* part of the parity contract.
#[derive(Debug)]
pub enum CapabilityProfileError {
    Io(String),
    Parse(String),
    InvalidProfileVersion,
    MissingProfileVersion,
    ProfileVersionMismatch,
    DuplicateCapability(String),
    UnknownCapability(String),
    InvalidCapabilityToken(String),
    TooManyEdges { capability: String, max: usize },
    CycleDetected(String),
    DepthExceeded { max: usize },
    CountExceeded { max: usize },
    PayloadExceeded { max: usize },
}

impl CapabilityProfileError {
    /// Stable class string, shared with the .NET expander and asserted by
    /// `engine/commands/capcomp-vectors.v1.json`. Never change an existing
    /// mapping without updating the vectors file and both runtimes.
    pub fn class(&self) -> &'static str {
        match self {
            Self::Io(_) | Self::Parse(_) | Self::InvalidProfileVersion => "invalid_profile",
            Self::MissingProfileVersion => "missing_profile_version",
            Self::ProfileVersionMismatch => "profile_version_mismatch",
            Self::DuplicateCapability(_) => "duplicate_capability",
            Self::UnknownCapability(_) => "unknown_capability",
            Self::InvalidCapabilityToken(_) => "invalid_token",
            Self::TooManyEdges { .. } => "too_many_edges",
            Self::CycleDetected(_) => "cycle",
            Self::DepthExceeded { .. } => "depth_exceeded",
            Self::CountExceeded { .. } => "count_exceeded",
            Self::PayloadExceeded { .. } => "payload_exceeded",
        }
    }
}

impl fmt::Display for CapabilityProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "io error: {msg}"),
            Self::Parse(msg) => write!(f, "parse error: {msg}"),
            Self::InvalidProfileVersion => write!(f, "invalid profile version"),
            Self::MissingProfileVersion => write!(
                f,
                "missing capability_profile_version while composition is enabled"
            ),
            Self::ProfileVersionMismatch => write!(f, "capability profile version mismatch"),
            Self::DuplicateCapability(cap) => write!(f, "duplicate capability in profile: {cap}"),
            Self::UnknownCapability(cap) => {
                write!(f, "unknown capability in profile expansion input: {cap}")
            }
            Self::InvalidCapabilityToken(cap) => write!(f, "invalid capability token: {cap}"),
            Self::TooManyEdges { capability, max } => {
                write!(
                    f,
                    "capability '{capability}' exceeds max edges per node: {max}"
                )
            }
            Self::CycleDetected(cap) => write!(f, "cycle detected at capability: {cap}"),
            Self::DepthExceeded { max } => write!(f, "capability graph depth exceeded max: {max}"),
            Self::CountExceeded { max } => {
                write!(f, "flattened capability count exceeded max: {max}")
            }
            Self::PayloadExceeded { max } => {
                write!(f, "flattened capability payload bytes exceeded max: {max}")
            }
        }
    }
}

impl std::error::Error for CapabilityProfileError {}

pub fn load_capability_profile_from_path(
    path: &Path,
) -> Result<CapabilityProfile, CapabilityProfileError> {
    let raw = fs::read_to_string(path).map_err(|e| {
        CapabilityProfileError::Io(format!("failed to read {}: {e}", path.display()))
    })?;
    load_capability_profile_from_json(&raw)
}

pub fn load_capability_profile_from_json(
    raw: &str,
) -> Result<CapabilityProfile, CapabilityProfileError> {
    let profile: CapabilityProfile =
        serde_json::from_str(raw).map_err(|e| CapabilityProfileError::Parse(e.to_string()))?;
    validate_profile_shape(&profile)?;
    Ok(profile)
}

pub fn load_capability_profile_from_value(
    value: &Value,
) -> Result<CapabilityProfile, CapabilityProfileError> {
    let profile: CapabilityProfile = serde_json::from_value(value.clone())
        .map_err(|e| CapabilityProfileError::Parse(e.to_string()))?;
    validate_profile_shape(&profile)?;
    Ok(profile)
}

/// Expand `assigned_capabilities` through `profile`'s inheritance DAG into
/// the deduplicated, ordinal-sorted, effective capability set. Does not
/// check the caller's claimed profile version — see
/// [`expand_with_version_gate`] for the full mint/validate-time contract.
pub fn flatten_capabilities(
    profile: &CapabilityProfile,
    assigned_capabilities: &[String],
) -> Result<Vec<String>, CapabilityProfileError> {
    let limits = resolved_limits(profile);
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();

    for node in &profile.nodes {
        let cap = canonicalize_capability(&node.capability, limits.max_capability_length)?;
        let mut inherits = Vec::with_capacity(node.inherits.len());
        for parent in &node.inherits {
            inherits.push(canonicalize_capability(
                parent,
                limits.max_capability_length,
            )?);
        }
        if inherits.len() > limits.max_edges_per_node {
            return Err(CapabilityProfileError::TooManyEdges {
                capability: cap,
                max: limits.max_edges_per_node,
            });
        }
        if graph.insert(cap.clone(), inherits).is_some() {
            return Err(CapabilityProfileError::DuplicateCapability(cap));
        }
    }

    for (cap, parents) in &graph {
        for parent in parents {
            if !graph.contains_key(parent) {
                return Err(CapabilityProfileError::UnknownCapability(format!(
                    "{parent} (referenced by {cap})"
                )));
            }
        }
    }

    let mut expanded: BTreeSet<String> = BTreeSet::new();
    let mut visiting: HashSet<String> = HashSet::new();

    for raw in assigned_capabilities {
        let cap = canonicalize_capability(raw, limits.max_capability_length)?;
        if !graph.contains_key(&cap) {
            return Err(CapabilityProfileError::UnknownCapability(cap));
        }
        dfs_expand(&cap, &graph, &limits, 0, &mut visiting, &mut expanded)?;
    }

    if expanded.len() > limits.max_capability_count {
        return Err(CapabilityProfileError::CountExceeded {
            max: limits.max_capability_count,
        });
    }

    let payload_bytes = expanded
        .iter()
        .enumerate()
        .map(|(i, s)| s.len() + usize::from(i > 0))
        .sum::<usize>();

    if payload_bytes > limits.max_flattened_payload_bytes {
        return Err(CapabilityProfileError::PayloadExceeded {
            max: limits.max_flattened_payload_bytes,
        });
    }

    Ok(expanded.into_iter().collect())
}

pub fn profile_supports_version(profile: &CapabilityProfile, requested: &str) -> bool {
    let requested_trimmed = requested.trim();
    if requested_trimmed.is_empty() {
        return false;
    }

    if requested_trimmed == profile.profile_version {
        return true;
    }

    profile
        .supported_profile_versions
        .iter()
        .any(|v| v.trim() == requested_trimmed)
}

/// The full mint/validate-time contract in one call: gate on the claimed
/// profile version, then expand. This is the canonical entry point the
/// cross-runtime parity vectors harness drives — see
/// `docs/CAPCOMP_PARITY_PLAN.md` §3. Production call sites in
/// `nodalmerge-server` and `nodalmerge-jwt-bridge` implement this same
/// sequence inline (to preserve their existing error message text); if you
/// change the sequence here, mirror the change there.
pub fn expand_with_version_gate(
    profile: &CapabilityProfile,
    assigned_capabilities: &[String],
    claimed_profile_version: Option<&str>,
) -> Result<Vec<String>, CapabilityProfileError> {
    let claimed = claimed_profile_version
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(CapabilityProfileError::MissingProfileVersion)?;

    if !profile_supports_version(profile, claimed) {
        return Err(CapabilityProfileError::ProfileVersionMismatch);
    }

    flatten_capabilities(profile, assigned_capabilities)
}

fn dfs_expand(
    cap: &str,
    graph: &HashMap<String, Vec<String>>,
    limits: &CapabilityProfileLimits,
    depth: usize,
    visiting: &mut HashSet<String>,
    expanded: &mut BTreeSet<String>,
) -> Result<(), CapabilityProfileError> {
    if depth > limits.max_dag_depth {
        return Err(CapabilityProfileError::DepthExceeded {
            max: limits.max_dag_depth,
        });
    }

    if visiting.contains(cap) {
        return Err(CapabilityProfileError::CycleDetected(cap.to_string()));
    }

    if expanded.contains(cap) {
        return Ok(());
    }

    visiting.insert(cap.to_string());

    if let Some(parents) = graph.get(cap) {
        for parent in parents {
            dfs_expand(parent, graph, limits, depth + 1, visiting, expanded)?;
        }
    }

    visiting.remove(cap);
    expanded.insert(cap.to_string());
    Ok(())
}

fn validate_profile_shape(profile: &CapabilityProfile) -> Result<(), CapabilityProfileError> {
    if profile.profile_version.trim().is_empty() {
        return Err(CapabilityProfileError::InvalidProfileVersion);
    }

    let limits = resolved_limits(profile);
    let mut seen = HashSet::new();

    for node in &profile.nodes {
        let capability = canonicalize_capability(&node.capability, limits.max_capability_length)?;
        if !seen.insert(capability.clone()) {
            return Err(CapabilityProfileError::DuplicateCapability(capability));
        }

        if node.inherits.len() > limits.max_edges_per_node {
            return Err(CapabilityProfileError::TooManyEdges {
                capability,
                max: limits.max_edges_per_node,
            });
        }

        for parent in &node.inherits {
            let _ = canonicalize_capability(parent, limits.max_capability_length)?;
        }
    }

    for version in &profile.supported_profile_versions {
        if version.trim().is_empty() {
            return Err(CapabilityProfileError::InvalidProfileVersion);
        }
    }

    Ok(())
}

/// Resolve a profile's effective limits. The `DEFAULT_MAX_*` constants are
/// hard ceilings, not just fallbacks: a `limits` block may lower a cap but
/// never raise it, otherwise a hostile or fat-fingered profile file would
/// lift the RoomToken mint/validate guard and unbound the expansion DFS
/// (which on `main` were unconditional constants). A profile with no
/// `limits` therefore behaves exactly as the historical constants did.
/// Clamping warns per field so an operator who requested 256 learns they
/// got 128 from the log, not from silent behavior.
fn resolved_limits(profile: &CapabilityProfile) -> CapabilityProfileLimits {
    let mut limits = profile.limits.clone().unwrap_or_default();
    clamp_to_ceiling(
        &mut limits.max_capability_count,
        DEFAULT_MAX_CAPABILITY_COUNT,
        "max_capability_count",
    );
    clamp_to_ceiling(
        &mut limits.max_capability_length,
        DEFAULT_MAX_CAPABILITY_LENGTH,
        "max_capability_length",
    );
    clamp_to_ceiling(
        &mut limits.max_flattened_payload_bytes,
        DEFAULT_MAX_FLATTENED_PAYLOAD_BYTES,
        "max_flattened_payload_bytes",
    );
    clamp_to_ceiling(&mut limits.max_dag_depth, DEFAULT_MAX_DAG_DEPTH, "max_dag_depth");
    clamp_to_ceiling(
        &mut limits.max_edges_per_node,
        DEFAULT_MAX_EDGES_PER_NODE,
        "max_edges_per_node",
    );
    limits
}

fn clamp_to_ceiling(value: &mut usize, ceiling: usize, field: &'static str) {
    if *value > ceiling {
        tracing::warn!(
            field,
            requested = *value,
            ceiling,
            "capability profile `limits` value exceeds the built-in hard ceiling; clamped"
        );
        *value = ceiling;
    }
}

/// Canonicalize a capability token: Unicode-trim, then fold only ASCII
/// `A-Z` to lowercase (non-ASCII characters are never folded into the
/// accepted charset — this must match the .NET expander's normalization
/// exactly, see `docs/CAPCOMP_PARITY_PLAN.md` §3).
fn canonicalize_capability(raw: &str, max_len: usize) -> Result<String, CapabilityProfileError> {
    let token = raw.trim().to_ascii_lowercase();
    if token.is_empty() || token.len() > max_len {
        return Err(CapabilityProfileError::InvalidCapabilityToken(
            raw.to_string(),
        ));
    }

    if !is_valid_capability_token(&token) {
        return Err(CapabilityProfileError::InvalidCapabilityToken(
            raw.to_string(),
        ));
    }

    Ok(token)
}

fn is_valid_capability_token(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }

    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_deduplicates_and_sorts() {
        let profile = CapabilityProfile {
            profile_version: "capprof-v1".to_string(),
            supported_profile_versions: vec![],
            nodes: vec![
                CapabilityNode {
                    capability: "root.admin".to_string(),
                    inherits: vec!["tick.admin".to_string(), "policy.admin".to_string()],
                },
                CapabilityNode {
                    capability: "tick.admin".to_string(),
                    inherits: vec![],
                },
                CapabilityNode {
                    capability: "policy.admin".to_string(),
                    inherits: vec![],
                },
            ],
            limits: None,
        };

        let flattened = flatten_capabilities(&profile, &[" root.admin ".to_string()]).unwrap();
        assert_eq!(
            flattened,
            vec![
                "policy.admin".to_string(),
                "root.admin".to_string(),
                "tick.admin".to_string()
            ]
        );
    }

    #[test]
    fn flatten_rejects_cycle() {
        let profile = CapabilityProfile {
            profile_version: "capprof-v1".to_string(),
            supported_profile_versions: vec![],
            nodes: vec![
                CapabilityNode {
                    capability: "a".to_string(),
                    inherits: vec!["b".to_string()],
                },
                CapabilityNode {
                    capability: "b".to_string(),
                    inherits: vec!["a".to_string()],
                },
            ],
            limits: None,
        };

        let err = flatten_capabilities(&profile, &["a".to_string()]).unwrap_err();
        assert!(matches!(err, CapabilityProfileError::CycleDetected(_)));
        assert_eq!(err.class(), "cycle");
    }

    #[test]
    fn flatten_rejects_depth_overflow() {
        let profile = CapabilityProfile {
            profile_version: "capprof-v1".to_string(),
            supported_profile_versions: vec![],
            nodes: vec![
                CapabilityNode {
                    capability: "a".to_string(),
                    inherits: vec!["b".to_string()],
                },
                CapabilityNode {
                    capability: "b".to_string(),
                    inherits: vec!["c".to_string()],
                },
                CapabilityNode {
                    capability: "c".to_string(),
                    inherits: vec![],
                },
            ],
            limits: Some(CapabilityProfileLimits {
                max_capability_count: 128,
                max_capability_length: 128,
                max_flattened_payload_bytes: 8192,
                max_dag_depth: 1,
                max_edges_per_node: 64,
            }),
        };

        let err = flatten_capabilities(&profile, &["a".to_string()]).unwrap_err();
        assert!(matches!(err, CapabilityProfileError::DepthExceeded { .. }));
        assert_eq!(err.class(), "depth_exceeded");
    }

    #[test]
    fn expand_with_version_gate_rejects_missing_version() {
        let profile = CapabilityProfile {
            profile_version: "capprof-v1".to_string(),
            supported_profile_versions: vec![],
            nodes: vec![CapabilityNode {
                capability: "room.member".to_string(),
                inherits: vec![],
            }],
            limits: None,
        };

        let err = expand_with_version_gate(&profile, &["room.member".to_string()], None)
            .unwrap_err();
        assert_eq!(err.class(), "missing_profile_version");
    }

    #[test]
    fn expand_with_version_gate_rejects_unsupported_version() {
        let profile = CapabilityProfile {
            profile_version: "capprof-v3".to_string(),
            supported_profile_versions: vec!["capprof-v2".to_string()],
            nodes: vec![CapabilityNode {
                capability: "room.member".to_string(),
                inherits: vec![],
            }],
            limits: None,
        };

        let err = expand_with_version_gate(
            &profile,
            &["room.member".to_string()],
            Some("capprof-v1"),
        )
        .unwrap_err();
        assert_eq!(err.class(), "profile_version_mismatch");
    }

    /// Minimal capturing subscriber so the clamp-warn tests can live here
    /// without a `tracing-subscriber` dev-dependency. `with_default` is
    /// thread-local, so parallel tests cannot cross-contaminate captures.
    mod warn_capture {
        use std::fmt::Write as _;
        use std::sync::{Arc, Mutex};

        pub(super) struct Capture(pub(super) Arc<Mutex<Vec<String>>>);

        struct Visitor(String);

        impl tracing::field::Visit for Visitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                let _ = write!(self.0, "{}={:?} ", field.name(), value);
            }
        }

        impl tracing::Subscriber for Capture {
            fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
                tracing::span::Id::from_u64(1)
            }
            fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
            fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
            fn event(&self, event: &tracing::Event<'_>) {
                let mut visitor = Visitor(format!("{} ", event.metadata().level()));
                event.record(&mut visitor);
                self.0.lock().unwrap().push(visitor.0);
            }
            fn enter(&self, _: &tracing::span::Id) {}
            fn exit(&self, _: &tracing::span::Id) {}
        }

        pub(super) fn captured(f: impl FnOnce()) -> Vec<String> {
            let events = Arc::new(Mutex::new(Vec::new()));
            tracing::subscriber::with_default(Capture(Arc::clone(&events)), f);
            let captured = events.lock().unwrap().clone();
            captured
        }
    }

    #[test]
    fn clamp_warns_with_field_requested_and_ceiling() {
        let profile = CapabilityProfile {
            profile_version: "capprof-v1".to_string(),
            supported_profile_versions: vec![],
            nodes: vec![CapabilityNode {
                capability: "room.member".to_string(),
                inherits: vec![],
            }],
            limits: Some(CapabilityProfileLimits {
                max_dag_depth: 64,
                ..CapabilityProfileLimits::default()
            }),
        };

        let events = warn_capture::captured(|| {
            flatten_capabilities(&profile, &["room.member".to_string()]).unwrap();
        });

        let warn = events
            .iter()
            .find(|e| e.contains("max_dag_depth"))
            .expect("clamping an oversized limit must emit a warn naming the field");
        assert!(warn.starts_with("WARN"), "clamp log must be warn-level: {warn}");
        assert!(warn.contains("requested=64"), "warn must carry the requested value: {warn}");
        assert!(
            warn.contains(&format!("ceiling={DEFAULT_MAX_DAG_DEPTH}")),
            "warn must carry the ceiling: {warn}"
        );
    }

    #[test]
    fn clamp_warn_does_not_fire_for_lowered_or_absent_limits() {
        let node = CapabilityNode {
            capability: "room.member".to_string(),
            inherits: vec![],
        };
        let no_limits = CapabilityProfile {
            profile_version: "capprof-v1".to_string(),
            supported_profile_versions: vec![],
            nodes: vec![node.clone()],
            limits: None,
        };
        let lowered = CapabilityProfile {
            limits: Some(CapabilityProfileLimits {
                max_dag_depth: 2,
                ..CapabilityProfileLimits::default()
            }),
            ..no_limits.clone()
        };

        let events = warn_capture::captured(|| {
            flatten_capabilities(&no_limits, &["room.member".to_string()]).unwrap();
            flatten_capabilities(&lowered, &["room.member".to_string()]).unwrap();
        });

        assert!(
            events.is_empty(),
            "no clamp engaged, so no warn may fire: {events:?}"
        );
    }

    #[test]
    fn expand_with_version_gate_accepts_compatibility_window_version() {
        let profile = CapabilityProfile {
            profile_version: "capprof-v3".to_string(),
            supported_profile_versions: vec!["capprof-v2".to_string()],
            nodes: vec![CapabilityNode {
                capability: "room.member".to_string(),
                inherits: vec![],
            }],
            limits: None,
        };

        let flattened = expand_with_version_gate(
            &profile,
            &["room.member".to_string()],
            Some("capprof-v2"),
        )
        .expect("expected supported compatibility-window version to pass");
        assert_eq!(flattened, vec!["room.member".to_string()]);
    }
}
