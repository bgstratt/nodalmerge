//! Asserts the Rust file-store blob layout against the canonical vectors
//! (`engine/commands/blob-layout-vectors.v1.json`). The .NET mirror is
//! `hosts/dotnet/tests/NodalMerge.DotNetHost.Tests/BlobLayoutParityTests.cs`;
//! the S3-key vectors are asserted by
//! `server/s3-blobs/tests/blob_layout_vectors.rs`. To change the layout,
//! update the vectors, both harnesses, and `docs/BLOB_STORAGE_LAYOUT.md`
//! together.

use nodalmerge_core::Hash;
use nodalmerge_server::store::{blob_relative_path, is_canonical_blob_name, tombstone_relative_path};
use serde::Deserialize;

const VECTORS_JSON: &str = include_str!("../../../engine/commands/blob-layout-vectors.v1.json");

#[derive(Debug, Deserialize)]
struct VectorsFile {
    path_vectors: Vec<PathVector>,
    name_conformance_vectors: Vec<NameVector>,
}

#[derive(Debug, Deserialize)]
struct PathVector {
    id: String,
    hash: String,
    #[serde(default)]
    blob_relative_path: Option<String>,
    #[serde(default)]
    tombstone_relative_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NameVector {
    id: String,
    name: String,
    canonical: bool,
}

fn parse_hash(hex: &str) -> Hash {
    assert_eq!(hex.len(), 64, "vector hash must be 64 hex chars");
    let mut out = [0u8; 32];
    let bytes = hex.as_bytes();
    for i in 0..32 {
        let hi = (bytes[2 * i] as char).to_digit(16).expect("valid hex") as u8;
        let lo = (bytes[2 * i + 1] as char).to_digit(16).expect("valid hex") as u8;
        out[i] = (hi << 4) | lo;
    }
    Hash(out)
}

/// Normalize a `PathBuf` to forward slashes so vectors written with `/`
/// compare correctly on Windows.
fn normalize(path: std::path::PathBuf) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[test]
fn blob_layout_vectors_match_rust_path_derivation() {
    let file: VectorsFile =
        serde_json::from_str(VECTORS_JSON).expect("engine/commands/blob-layout-vectors.v1.json must parse");

    let mut failures = Vec::new();

    for vector in &file.path_vectors {
        let hash = parse_hash(&vector.hash);

        if let Some(expected) = &vector.blob_relative_path {
            let actual = normalize(blob_relative_path(&hash));
            if &actual != expected {
                failures.push(format!(
                    "vector `{}`: expected blob_relative_path {expected}, got {actual}",
                    vector.id
                ));
            }
        }

        if let Some(expected) = &vector.tombstone_relative_path {
            let actual = normalize(tombstone_relative_path(&hash));
            if &actual != expected {
                failures.push(format!(
                    "vector `{}`: expected tombstone_relative_path {expected}, got {actual}",
                    vector.id
                ));
            }
        }
    }

    for vector in &file.name_conformance_vectors {
        let actual = is_canonical_blob_name(&vector.name);
        if actual != vector.canonical {
            failures.push(format!(
                "vector `{}`: expected canonical={}, got canonical={} (name `{}`)",
                vector.id, vector.canonical, actual, vector.name
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "blob layout vectors drifted from Rust file-store derivation:\n{}",
        failures.join("\n")
    );
}
