//! Hard-ceiling clamp on profile-supplied `limits`
//! (plans/blob-cas-remediation.md §4.4, finding #15).
//!
//! On `main` the mint-path safety caps were unconditional constants
//! (128 caps / 128-char tokens / 8 KiB payload / depth 16 / 64 edges —
//! `server/jwt-bridge/src/capability_profile.rs` pre-convergence). This
//! crate made them profile-file-supplied, so a `limits` block that used to
//! be inert could *raise* its own ceilings and lift the RoomToken guard.
//! These tests pin the restored contract: a profile may lower a limit but
//! never raise it past the built-in maximum, and a profile with no
//! `limits` behaves exactly as the historical constants did.
//!
//! Everything here drives the public API only (`flatten_capabilities`,
//! `load_capability_profile_from_json`) so the file compiles and runs
//! against the pre-fix crate unchanged — the clamp-warn tests live in the
//! crate's inline test module because they need the `tracing` dependency
//! the fix introduces.

use nodalmerge_capability_profile::{
    flatten_capabilities, load_capability_profile_from_json, CapabilityNode, CapabilityProfile,
    CapabilityProfileError, CapabilityProfileLimits,
};

// The historical unconditional constants — duplicated literally rather than
// referenced through the crate's `DEFAULT_*` consts so a future edit to
// those consts cannot silently move this suite's goalposts.
const CEILING_COUNT: usize = 128;
const CEILING_LENGTH: usize = 128;
const CEILING_PAYLOAD_BYTES: usize = 8 * 1024;
const CEILING_DEPTH: usize = 16;
const CEILING_EDGES: usize = 64;

fn profile_with(nodes: Vec<CapabilityNode>, limits: Option<CapabilityProfileLimits>) -> CapabilityProfile {
    CapabilityProfile {
        profile_version: "capprof-v1".to_string(),
        supported_profile_versions: vec![],
        nodes,
        limits,
    }
}

fn node(capability: &str, inherits: Vec<String>) -> CapabilityNode {
    CapabilityNode {
        capability: capability.to_string(),
        inherits,
    }
}

fn limits_at_ceilings() -> CapabilityProfileLimits {
    CapabilityProfileLimits {
        max_capability_count: CEILING_COUNT,
        max_capability_length: CEILING_LENGTH,
        max_flattened_payload_bytes: CEILING_PAYLOAD_BYTES,
        max_dag_depth: CEILING_DEPTH,
        max_edges_per_node: CEILING_EDGES,
    }
}

/// `root` inheriting `leaf_count` distinct leaves: expansion size is
/// `leaf_count + 1`. Only for edge-cap tests — `leaf_count` above 64 puts
/// the root itself over the edge ceiling.
fn fan_out_nodes(leaf_count: usize) -> Vec<CapabilityNode> {
    let leaves: Vec<String> = (0..leaf_count).map(|i| format!("leaf{i:04}")).collect();
    let mut nodes = vec![node("root", leaves.clone())];
    nodes.extend(leaves.iter().map(|l| node(l, vec![])));
    nodes
}

/// `total` flattened caps (root → 2 mids → leaves) with every node at or
/// under 64 edges and depth 2, so only the *count* cap can fire. Valid for
/// `total` up to 131.
fn layered_nodes(total: usize) -> Vec<CapabilityNode> {
    assert!((4..=131).contains(&total), "layered_nodes supports 4..=131");
    let leaf_count = total - 3;
    let leaves: Vec<String> = (0..leaf_count).map(|i| format!("leaf{i:04}")).collect();
    let (first, second) = leaves.split_at(leaf_count.min(64));
    let mut nodes = vec![
        node("root", vec!["mid0".to_string(), "mid1".to_string()]),
        node("mid0", first.to_vec()),
        node("mid1", second.to_vec()),
    ];
    nodes.extend(leaves.iter().map(|l| node(l, vec![])));
    nodes
}

/// A linear chain of `len` nodes: `c0000 → c0001 → …`. The deepest node
/// sits at DFS depth `len - 1`.
fn chain_nodes(len: usize) -> Vec<CapabilityNode> {
    (0..len)
        .map(|i| {
            let inherits = if i + 1 < len {
                vec![format!("c{:04}", i + 1)]
            } else {
                vec![]
            };
            node(&format!("c{i:04}"), inherits)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// RED: oversized `limits` must be clamped to the historical ceilings
// (pre-fix each of these mints/validates successfully — the raised limit is
// honored; post-fix the input is rejected exactly as `main` rejected it).
// ---------------------------------------------------------------------------

#[test]
fn oversized_count_limit_is_clamped_to_historical_ceiling() {
    // 130 flattened caps: over the historical 128 cap, under the profile's
    // claimed 1024 (layered so no node trips the edge cap first).
    let profile = profile_with(
        layered_nodes(CEILING_COUNT + 2),
        Some(CapabilityProfileLimits {
            max_capability_count: 1024,
            ..limits_at_ceilings()
        }),
    );

    let err = flatten_capabilities(&profile, &["root".to_string()])
        .expect_err("a 130-cap expansion must not out-mint the built-in 128-cap ceiling");
    assert!(
        matches!(err, CapabilityProfileError::CountExceeded { max } if max == CEILING_COUNT),
        "expected CountExceeded at the built-in ceiling, got: {err}"
    );
}

#[test]
fn oversized_depth_limit_keeps_dfs_bounded_at_historical_ceiling() {
    // An 18-node chain reaches DFS depth 17 — one past the historical
    // bound of 16 — while the profile claims depth 64 is fine.
    let profile = profile_with(
        chain_nodes(CEILING_DEPTH + 2),
        Some(CapabilityProfileLimits {
            max_dag_depth: 64,
            ..limits_at_ceilings()
        }),
    );

    let err = flatten_capabilities(&profile, &["c0000".to_string()])
        .expect_err("the DFS must stop at the built-in depth ceiling, not the profile's 64");
    assert!(
        matches!(err, CapabilityProfileError::DepthExceeded { max } if max == CEILING_DEPTH),
        "expected DepthExceeded at the built-in ceiling, got: {err}"
    );
}

#[test]
fn oversized_edges_limit_is_clamped_to_historical_ceiling() {
    // 65 direct edges on one node, profile claims 512 are allowed.
    let profile = profile_with(
        fan_out_nodes(CEILING_EDGES + 1),
        Some(CapabilityProfileLimits {
            max_edges_per_node: 512,
            ..limits_at_ceilings()
        }),
    );

    let err = flatten_capabilities(&profile, &["root".to_string()])
        .expect_err("65 edges on one node must exceed the built-in 64-edge ceiling");
    assert!(
        matches!(err, CapabilityProfileError::TooManyEdges { max, .. } if max == CEILING_EDGES),
        "expected TooManyEdges at the built-in ceiling, got: {err}"
    );
}

#[test]
fn oversized_payload_limit_is_clamped_to_historical_ceiling() {
    // 73 caps of 120 chars ≈ 8.6 KiB comma-joined — over the historical
    // 8 KiB, under the profile's claimed 1 MiB, and inside the count, edge
    // and length caps so only the payload cap can fire (root → 2 mids →
    // 35 leaves each).
    let pad = |prefix: &str, i: usize| format!("{prefix}{i:03}{}", "x".repeat(120 - prefix.len() - 3));
    let left: Vec<String> = (0..35).map(|i| pad("pl", i)).collect();
    let right: Vec<String> = (0..35).map(|i| pad("pr", i)).collect();
    let mid0 = pad("m0", 0);
    let mid1 = pad("m1", 0);
    let mut nodes = vec![
        node(&pad("rt", 0), vec![mid0.clone(), mid1.clone()]),
        node(&mid0, left.clone()),
        node(&mid1, right.clone()),
    ];
    nodes.extend(left.iter().chain(right.iter()).map(|l| node(l, vec![])));

    let joined_len: usize = nodes.iter().map(|n| n.capability.len()).sum::<usize>() + nodes.len() - 1;
    assert!(
        joined_len > CEILING_PAYLOAD_BYTES,
        "test-shape precondition: payload {joined_len} must exceed {CEILING_PAYLOAD_BYTES}"
    );

    let profile = profile_with(
        nodes,
        Some(CapabilityProfileLimits {
            max_flattened_payload_bytes: 1_000_000,
            ..limits_at_ceilings()
        }),
    );

    let root = profile.nodes[0].capability.clone();
    let err = flatten_capabilities(&profile, &[root])
        .expect_err("an 8.4 KiB payload must exceed the built-in 8 KiB ceiling");
    assert!(
        matches!(err, CapabilityProfileError::PayloadExceeded { max } if max == CEILING_PAYLOAD_BYTES),
        "expected PayloadExceeded at the built-in ceiling, got: {err}"
    );
}

#[test]
fn oversized_length_limit_is_clamped_to_historical_ceiling() {
    // A 129-char token, profile claims 256 chars are allowed.
    let long = format!("a{}", "b".repeat(CEILING_LENGTH));
    let profile = profile_with(
        vec![node(&long, vec![])],
        Some(CapabilityProfileLimits {
            max_capability_length: 256,
            ..limits_at_ceilings()
        }),
    );

    let err = flatten_capabilities(&profile, &[long])
        .expect_err("a 129-char token must exceed the built-in 128-char ceiling");
    assert!(
        matches!(err, CapabilityProfileError::InvalidCapabilityToken(_)),
        "expected InvalidCapabilityToken at the built-in ceiling, got: {err}"
    );
}

#[test]
fn oversized_limits_in_profile_json_are_clamped_at_load_shape_validation() {
    // The load path (`validate_profile_shape`) resolves limits too: a
    // profile FILE whose `limits` raises the edge cap must still be
    // rejected at load when a node exceeds the built-in ceiling. Also pins
    // that the file format is unchanged — the oversized block parses; only
    // resolution semantics clamp it.
    let inherits: Vec<String> = (0..CEILING_EDGES + 1).map(|i| format!("\"e{i:03}\"")).collect();
    let leaf_nodes: Vec<String> = (0..CEILING_EDGES + 1)
        .map(|i| format!("{{ \"capability\": \"e{i:03}\", \"inherits\": [] }}"))
        .collect();
    let raw = format!(
        r#"{{
            "profile_version": "capprof-v1",
            "limits": {{ "max_edges_per_node": 512 }},
            "nodes": [
                {{ "capability": "root", "inherits": [{}] }},
                {}
            ]
        }}"#,
        inherits.join(", "),
        leaf_nodes.join(",\n                ")
    );

    let err = load_capability_profile_from_json(&raw)
        .err()
        .expect("load must reject 65 edges regardless of the profile's own limits block");
    assert!(
        matches!(err, CapabilityProfileError::TooManyEdges { max, .. } if max == CEILING_EDGES),
        "expected TooManyEdges at the built-in ceiling, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Green-side equivalence pins: legit profiles are provably unchanged.
// ---------------------------------------------------------------------------

#[test]
fn no_limits_profile_enforces_exactly_the_historical_constants() {
    // Green-side pin (passes pre- and post-fix): with no `limits` block the
    // boundaries sit exactly at the historical constants.

    // 128 flattened caps pass; 129 fail.
    let at_cap = profile_with(layered_nodes(CEILING_COUNT), None);
    assert_eq!(
        flatten_capabilities(&at_cap, &["root".to_string()])
            .expect("128 caps must pass with no limits block")
            .len(),
        CEILING_COUNT
    );
    let over_cap = profile_with(layered_nodes(CEILING_COUNT + 1), None);
    let err = flatten_capabilities(&over_cap, &["root".to_string()])
        .expect_err("129 caps must fail with no limits block");
    assert!(matches!(err, CapabilityProfileError::CountExceeded { max } if max == CEILING_COUNT));

    // Depth 16 passes; depth 17 fails.
    let at_depth = profile_with(chain_nodes(CEILING_DEPTH + 1), None);
    assert!(flatten_capabilities(&at_depth, &["c0000".to_string()]).is_ok());
    let over_depth = profile_with(chain_nodes(CEILING_DEPTH + 2), None);
    let err = flatten_capabilities(&over_depth, &["c0000".to_string()])
        .expect_err("depth 17 must fail with no limits block");
    assert!(matches!(err, CapabilityProfileError::DepthExceeded { max } if max == CEILING_DEPTH));
}

#[test]
fn limits_at_the_ceilings_behave_identically_to_no_limits() {
    // Green-side pin: restating the ceilings verbatim is a no-op.
    let over_depth = profile_with(chain_nodes(CEILING_DEPTH + 2), Some(limits_at_ceilings()));
    let err = flatten_capabilities(&over_depth, &["c0000".to_string()])
        .expect_err("depth 17 must fail with limits at the ceilings");
    assert!(matches!(err, CapabilityProfileError::DepthExceeded { max } if max == CEILING_DEPTH));

    let at_depth = profile_with(chain_nodes(CEILING_DEPTH + 1), Some(limits_at_ceilings()));
    assert!(flatten_capabilities(&at_depth, &["c0000".to_string()]).is_ok());
}

#[test]
fn lowered_limits_still_lower() {
    // Green-side pin (already the behavior pre-fix, locked by the
    // `limits-depth-exceeded` parity vector): lowering stays honored.
    let profile = profile_with(
        chain_nodes(4),
        Some(CapabilityProfileLimits {
            max_dag_depth: 2,
            ..limits_at_ceilings()
        }),
    );
    let err = flatten_capabilities(&profile, &["c0000".to_string()])
        .expect_err("a lowered depth limit must still be enforced");
    assert!(matches!(err, CapabilityProfileError::DepthExceeded { max } if max == 2));
}

#[test]
fn oversized_limits_block_still_parses_unchanged() {
    // Format pin: the `limits` block stays parseable exactly as-is — the
    // parsed values are preserved verbatim; only resolution clamps.
    let raw = r#"{
        "profile_version": "capprof-v1",
        "limits": { "max_dag_depth": 64, "max_capability_count": 1024 },
        "nodes": [ { "capability": "room.member", "inherits": [] } ]
    }"#;
    let profile = load_capability_profile_from_json(raw)
        .expect("an oversized limits block must still parse — format is unchanged");
    let limits = profile.limits.expect("limits block must be preserved");
    assert_eq!(limits.max_dag_depth, 64);
    assert_eq!(limits.max_capability_count, 1024);
    // Unspecified fields still fall back per-field.
    assert_eq!(limits.max_edges_per_node, CEILING_EDGES);
}
