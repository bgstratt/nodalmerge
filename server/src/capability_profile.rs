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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CapabilityProfileLimits {
    pub max_capability_count: usize,
    pub max_capability_length: usize,
    pub max_flattened_payload_bytes: usize,
    pub max_dag_depth: usize,
    pub max_edges_per_node: usize,
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

#[derive(Debug)]
pub enum CapabilityProfileError {
    Io(String),
    Parse(String),
    InvalidProfileVersion,
    DuplicateCapability(String),
    UnknownCapability(String),
    InvalidCapabilityToken(String),
    TooManyEdges { capability: String, max: usize },
    CycleDetected(String),
    DepthExceeded { max: usize },
    CountExceeded { max: usize },
    PayloadExceeded { max: usize },
}

impl fmt::Display for CapabilityProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "io error: {msg}"),
            Self::Parse(msg) => write!(f, "parse error: {msg}"),
            Self::InvalidProfileVersion => write!(f, "invalid profile version"),
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

fn resolved_limits(profile: &CapabilityProfile) -> CapabilityProfileLimits {
    profile.limits.clone().unwrap_or_default()
}

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
    }
}
