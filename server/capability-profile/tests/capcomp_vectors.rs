//! Asserts the Rust CAPCOMP implementation against the canonical vectors
//! (`engine/commands/capcomp-vectors.v1.json`). The .NET mirror is
//! `hosts/dotnet/tests/NodalMerge.DotNetHost.Tests/CapcompParityTests.cs`,
//! which reads the same file. To change the algorithm, update the vectors,
//! both harnesses, and `docs/CAPCOMP_PARITY_PLAN.md` together.
//!
//! `mode: "expand"` vectors load a profile and run the real mint/validate-time
//! entry point (`expand_with_version_gate`), asserting either the flattened
//! capability list or a stable error class. `mode: "passthrough"` vectors
//! have no profile; this harness only asserts the vector's own
//! self-consistency (assigned == expect.ok) since there is no
//! profile-optional entry point in this shared crate — the .NET harness
//! exercises the real "disabled" code path via `CapabilityProfileExpander`.

use nodalmerge_capability_profile::{
    expand_with_version_gate, load_capability_profile_from_value, CapabilityProfileError,
};
use serde::Deserialize;
use serde_json::Value;

const VECTORS_JSON: &str = include_str!("../../../engine/commands/capcomp-vectors.v1.json");

#[derive(Debug, Deserialize)]
struct VectorsFile {
    vectors: Vec<Vector>,
}

#[derive(Debug, Deserialize)]
struct Vector {
    id: String,
    mode: String,
    #[serde(default)]
    profile: Option<Value>,
    assigned: Vec<String>,
    #[serde(default)]
    claimed_profile_version: Option<String>,
    expect: Expect,
}

#[derive(Debug, Deserialize)]
struct Expect {
    #[serde(default)]
    ok: Option<Vec<String>>,
    #[serde(default)]
    error: Option<String>,
}

#[test]
fn capcomp_vectors_match_rust_expansion() {
    let file: VectorsFile =
        serde_json::from_str(VECTORS_JSON).expect("engine/commands/capcomp-vectors.v1.json must parse");
    assert!(!file.vectors.is_empty(), "vectors file must not be empty");

    let mut failures = Vec::new();

    for vector in &file.vectors {
        match vector.mode.as_str() {
            "expand" => {
                let profile_value = vector
                    .profile
                    .as_ref()
                    .unwrap_or_else(|| panic!("vector `{}`: mode `expand` requires a profile", vector.id));

                let result = load_capability_profile_from_value(profile_value).and_then(|profile| {
                    expand_with_version_gate(
                        &profile,
                        &vector.assigned,
                        vector.claimed_profile_version.as_deref(),
                    )
                });

                check_expand_result(&vector.id, result, &vector.expect, &mut failures);
            }
            "passthrough" => {
                let expected_ok = vector.expect.ok.as_ref().unwrap_or_else(|| {
                    panic!("vector `{}`: mode `passthrough` requires expect.ok", vector.id)
                });
                if &vector.assigned != expected_ok {
                    failures.push(format!(
                        "vector `{}`: passthrough vector is not self-consistent (assigned {:?} != expect.ok {:?})",
                        vector.id, vector.assigned, expected_ok
                    ));
                }
            }
            other => panic!("vector `{}`: unknown mode `{other}`", vector.id),
        }
    }

    assert!(
        failures.is_empty(),
        "CAPCOMP vectors drifted from Rust expansion:\n{}",
        failures.join("\n")
    );
}

fn check_expand_result(
    id: &str,
    result: Result<Vec<String>, CapabilityProfileError>,
    expect: &Expect,
    failures: &mut Vec<String>,
) {
    match (result, &expect.ok, &expect.error) {
        (Ok(actual), Some(expected_ok), None) => {
            if &actual != expected_ok {
                failures.push(format!(
                    "vector `{id}`: expected ok {expected_ok:?}, got ok {actual:?}"
                ));
            }
        }
        (Ok(actual), None, Some(expected_class)) => {
            failures.push(format!(
                "vector `{id}`: expected error class `{expected_class}`, got ok {actual:?}"
            ));
        }
        (Err(err), Some(expected_ok), None) => {
            failures.push(format!(
                "vector `{id}`: expected ok {expected_ok:?}, got error `{err}` (class `{}`)",
                err.class()
            ));
        }
        (Err(err), None, Some(expected_class)) => {
            if err.class() != expected_class {
                failures.push(format!(
                    "vector `{id}`: expected error class `{expected_class}`, got class `{}` (message: `{err}`)",
                    err.class()
                ));
            }
        }
        _ => {
            failures.push(format!(
                "vector `{id}`: malformed expect block (must have exactly one of ok/error)"
            ));
        }
    }
}
