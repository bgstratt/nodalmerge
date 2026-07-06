//! Thin CAPCOMP wrapper for the minting path: reads
//! `NODALMERGE_CAPABILITY_PROFILE_PATH`, caches the loaded profile for the
//! process lifetime, and delegates the actual DAG expansion + version gate
//! to the shared `nodalmerge-capability-profile` crate (also used by
//! `nodalmerge-server`). See `docs/CAPCOMP_PARITY_PLAN.md`.

use nodalmerge_capability_profile::{
    profile_supports_version, CapabilityProfile, CapabilityProfileError,
};
use std::path::PathBuf;
use std::sync::OnceLock;

static PROFILE_CACHE: OnceLock<Result<Option<CapabilityProfile>, CapabilityProfileError>> =
    OnceLock::new();

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
        .ok_or_else(|| {
            "missing capability_profile_version while composition is enabled".to_string()
        })?;

    if !profile_supports_version(profile, claimed) {
        return Err(CapabilityProfileError::ProfileVersionMismatch.to_string());
    }

    nodalmerge_capability_profile::flatten_capabilities(profile, assigned).map_err(|e| e.to_string())
}

fn load_profile_from_env() -> Result<Option<CapabilityProfile>, CapabilityProfileError> {
    let Some(path_raw) = std::env::var_os("NODALMERGE_CAPABILITY_PROFILE_PATH") else {
        return Ok(None);
    };

    let path = PathBuf::from(path_raw);
    nodalmerge_capability_profile::load_capability_profile_from_path(&path).map(Some)
}

#[cfg(test)]
mod tests {
    use super::{expand_minted_capabilities_with_profile, CapabilityProfile};
    use nodalmerge_capability_profile::CapabilityNode;

    #[test]
    fn compatibility_window_accepts_supported_profile_version() {
        let profile = CapabilityProfile {
            profile_version: "capprof-v3".to_string(),
            supported_profile_versions: vec!["capprof-v2".to_string()],
            nodes: vec![CapabilityNode {
                capability: "room.member".to_string(),
                inherits: Vec::new(),
            }],
            limits: None,
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
            limits: None,
        };

        let assigned = vec!["room.member".to_string()];
        let err =
            expand_minted_capabilities_with_profile(&profile, &assigned, Some("capprof-v1"))
                .expect_err("expected unsupported version to fail");
        assert_eq!(err, "capability profile version mismatch");
    }
}
