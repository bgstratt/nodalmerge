use serde::Deserialize;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;

const MAX_CAPABILITY_COUNT: usize = 128;
const MAX_CAPABILITY_LENGTH: usize = 128;
const MAX_FLATTENED_PAYLOAD_BYTES: usize = 8 * 1024;
const MAX_DAG_DEPTH: usize = 16;
const MAX_EDGES_PER_NODE: usize = 64;

#[derive(Debug, Clone, Deserialize)]
struct CapabilityProfile {
    profile_version: String,
    #[serde(default)]
    supported_profile_versions: Vec<String>,
    nodes: Vec<CapabilityNode>,
}

#[derive(Debug, Clone, Deserialize)]
struct CapabilityNode {
    capability: String,
    #[serde(default)]
    inherits: Vec<String>,
}

#[derive(Debug)]
enum CapabilityProfileError {
    ProfileVersionMismatch,
    InvalidToken(String),
    UnknownCapability(String),
    DuplicateCapability(String),
    TooManyEdges(String),
    Cycle(String),
    DepthExceeded,
    CountExceeded,
    PayloadExceeded,
    Io(String),
    Parse(String),
}

impl std::fmt::Display for CapabilityProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProfileVersionMismatch => write!(f, "capability profile version mismatch"),
            Self::InvalidToken(v) => write!(f, "invalid capability token: {v}"),
            Self::UnknownCapability(v) => write!(f, "unknown capability: {v}"),
            Self::DuplicateCapability(v) => write!(f, "duplicate capability: {v}"),
            Self::TooManyEdges(v) => write!(f, "capability has too many inheritance edges: {v}"),
            Self::Cycle(v) => write!(f, "cycle detected at capability: {v}"),
            Self::DepthExceeded => write!(f, "capability graph depth exceeded"),
            Self::CountExceeded => write!(f, "flattened capability count exceeded"),
            Self::PayloadExceeded => write!(f, "flattened capability payload exceeded"),
            Self::Io(v) => write!(f, "{v}"),
            Self::Parse(v) => write!(f, "{v}"),
        }
    }
}

static PROFILE_CACHE: OnceLock<Result<Option<CapabilityProfile>, CapabilityProfileError>> = OnceLock::new();

pub fn maybe_expand_minted_capabilities(
    assigned: &[String],
    claimed_profile_version: Option<&str>,
) -> Result<Vec<String>, String> {
    let profile = match PROFILE_CACHE.get_or_init(load_profile_from_env) {
        Ok(Some(profile)) => profile,
        Ok(None) => return Ok(assigned.to_vec()),
        Err(err) => return Err(err.to_string()),
    };

    expand_minted_capabilities_with_profile(profile, assigned, claimed_profile_version)
}

fn expand_minted_capabilities_with_profile(
    profile: &CapabilityProfile,
    assigned: &[String],
    claimed_profile_version: Option<&str>,
) -> Result<Vec<String>, String> {

    let claimed = claimed_profile_version
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "missing capability_profile_version while composition is enabled".to_string())?;

    if !profile_supports_version(profile, claimed) {
        return Err(CapabilityProfileError::ProfileVersionMismatch.to_string());
    }

    flatten(profile, assigned).map_err(|e| e.to_string())
}

fn load_profile_from_env() -> Result<Option<CapabilityProfile>, CapabilityProfileError> {
    let Some(path_raw) = std::env::var_os("ACTIVESYNC_CAPABILITY_PROFILE_PATH") else {
        return Ok(None);
    };

    let path = PathBuf::from(path_raw);
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| CapabilityProfileError::Io(format!("failed to read {}: {e}", path.display())))?;

    let profile: CapabilityProfile =
        serde_json::from_str(&raw).map_err(|e| CapabilityProfileError::Parse(format!("invalid capability profile json: {e}")))?;

    if profile.profile_version.trim().is_empty() {
        return Err(CapabilityProfileError::Parse(
            "capability profile version must not be empty".to_string(),
        ));
    }

    for v in &profile.supported_profile_versions {
        if v.trim().is_empty() {
            return Err(CapabilityProfileError::Parse(
                "supported_profile_versions entries must not be empty".to_string(),
            ));
        }
    }

    Ok(Some(profile))
}

fn profile_supports_version(profile: &CapabilityProfile, requested: &str) -> bool {
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

fn flatten(profile: &CapabilityProfile, assigned: &[String]) -> Result<Vec<String>, CapabilityProfileError> {
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();

    for node in &profile.nodes {
        let cap = normalize_token(&node.capability)?;
        let mut inherits = Vec::with_capacity(node.inherits.len());
        for parent in &node.inherits {
            inherits.push(normalize_token(parent)?);
        }

        if inherits.len() > MAX_EDGES_PER_NODE {
            return Err(CapabilityProfileError::TooManyEdges(cap));
        }

        if graph.insert(cap.clone(), inherits).is_some() {
            return Err(CapabilityProfileError::DuplicateCapability(cap));
        }
    }

    for parents in graph.values() {
        for parent in parents {
            if !graph.contains_key(parent) {
                return Err(CapabilityProfileError::UnknownCapability(parent.clone()));
            }
        }
    }

    let mut expanded = BTreeSet::new();
    let mut visiting = HashSet::new();

    for source in assigned {
        let source_cap = normalize_token(source)?;
        if !graph.contains_key(&source_cap) {
            return Err(CapabilityProfileError::UnknownCapability(source_cap));
        }
        dfs_expand(&source_cap, 0, &graph, &mut visiting, &mut expanded)?;
    }

    if expanded.len() > MAX_CAPABILITY_COUNT {
        return Err(CapabilityProfileError::CountExceeded);
    }

    let payload_bytes = expanded
        .iter()
        .enumerate()
        .map(|(i, s)| s.len() + usize::from(i > 0))
        .sum::<usize>();

    if payload_bytes > MAX_FLATTENED_PAYLOAD_BYTES {
        return Err(CapabilityProfileError::PayloadExceeded);
    }

    Ok(expanded.into_iter().collect())
}

fn dfs_expand(
    cap: &str,
    depth: usize,
    graph: &HashMap<String, Vec<String>>,
    visiting: &mut HashSet<String>,
    expanded: &mut BTreeSet<String>,
) -> Result<(), CapabilityProfileError> {
    if depth > MAX_DAG_DEPTH {
        return Err(CapabilityProfileError::DepthExceeded);
    }

    if visiting.contains(cap) {
        return Err(CapabilityProfileError::Cycle(cap.to_string()));
    }

    if expanded.contains(cap) {
        return Ok(());
    }

    visiting.insert(cap.to_string());

    if let Some(parents) = graph.get(cap) {
        for parent in parents {
            dfs_expand(parent, depth + 1, graph, visiting, expanded)?;
        }
    }

    visiting.remove(cap);
    expanded.insert(cap.to_string());

    Ok(())
}

fn normalize_token(raw: &str) -> Result<String, CapabilityProfileError> {
    let token = raw.trim().to_ascii_lowercase();
    if token.is_empty() || token.len() > MAX_CAPABILITY_LENGTH {
        return Err(CapabilityProfileError::InvalidToken(raw.to_string()));
    }

    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return Err(CapabilityProfileError::InvalidToken(raw.to_string()));
    };

    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(CapabilityProfileError::InvalidToken(raw.to_string()));
    }

    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-') {
        return Err(CapabilityProfileError::InvalidToken(raw.to_string()));
    }

    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::{expand_minted_capabilities_with_profile, CapabilityNode, CapabilityProfile};

    #[test]
    fn compatibility_window_accepts_supported_profile_version() {
        let profile = CapabilityProfile {
            profile_version: "capprof-v3".to_string(),
            supported_profile_versions: vec!["capprof-v2".to_string()],
            nodes: vec![CapabilityNode {
                capability: "room.member".to_string(),
                inherits: Vec::new(),
            }],
        };

        let assigned = vec!["room.member".to_string()];
        let flattened =
            expand_minted_capabilities_with_profile(&profile, &assigned, Some("capprof-v2"))
                .expect("expected supported compatibility-window version to pass");
        assert_eq!(flattened, vec!["room.member".to_string()]);
    }

    #[test]
    fn compatibility_window_rejects_unsupported_profile_version() {
        let profile = CapabilityProfile {
            profile_version: "capprof-v3".to_string(),
            supported_profile_versions: vec!["capprof-v2".to_string()],
            nodes: vec![CapabilityNode {
                capability: "room.member".to_string(),
                inherits: Vec::new(),
            }],
        };

        let assigned = vec!["room.member".to_string()];
        let err =
            expand_minted_capabilities_with_profile(&profile, &assigned, Some("capprof-v1"))
                .expect_err("expected unsupported version to fail");
        assert_eq!(err, "capability profile version mismatch");
    }
}
