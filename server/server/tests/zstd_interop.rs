//! Cross-runtime zstd interop — Phase 0 slice 0.1 of
//! nodalmerge-studio/plans/blob-cas-remediation.md (finding #4).
//!
//! Every prior `.zst` fixture/test was same-runtime. This test proves the
//! **other** direction: Rust decoding a frame produced by .NET's
//! `ZstdSharp.Compressor.Wrap` (`engine/commands/fixtures/zstd-interop-v1/`,
//! manifest `engine/commands/zstd-interop-vectors.v1.json`). Rust's
//! `zstd::stream::decode_all` (via `DirPersistence::get_blob`, the real
//! production read path) already handles both content-size-header shapes,
//! so this direction is expected to pass without any production change.
//!
//! The other direction — .NET decoding a Rust-produced (header-less) frame
//! — is the currently-broken one; that RED test lives in
//! `hosts/dotnet/tests/NodalMerge.DotNetHost.Tests/ZstdInteropTests.cs` and
//! is skip-gated until slice 3.1 fixes `BlobCompression.TryDecompress`.

use nodalmerge_core::Hash;
use nodalmerge_server::store::{BlobPersistence, DirPersistence};
use serde::Deserialize;
use std::path::{Path, PathBuf};

const VECTORS_JSON: &str = include_str!("../../../engine/commands/zstd-interop-vectors.v1.json");

#[derive(Debug, Deserialize)]
struct VectorsFile {
    fixtures: Vec<FixtureVector>,
}

#[derive(Debug, Deserialize)]
struct FixtureVector {
    id: String,
    producer_runtime: String,
    consumer_runtime: String,
    fixture_path: String,
    decoded_plaintext_len: usize,
    decoded_plaintext_blake3: String,
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

fn tmpdir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("nodalmerge-zstd-interop-{tag}-{nanos}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Places `fixture_bytes` at the canonical encoded-blob location under a
/// fresh `DirPersistence` root (`<store_root>/blobs/blake3/<hash>.zst`) and
/// opens it — the same real production read path
/// `server/server/tests/blob_layout_interop.rs` exercises, not a raw
/// `zstd::stream::decode_all` call.
fn get_blob_via_dir_persistence(hash: &Hash, fixture_bytes: &[u8], tag: &str) -> Option<Vec<u8>> {
    let store_root = tmpdir(tag);
    let blake3_dir = store_root.join("blobs").join("blake3");
    std::fs::create_dir_all(&blake3_dir).unwrap();
    std::fs::write(blake3_dir.join(format!("{}.zst", hash.to_hex())), fixture_bytes).unwrap();

    let persistence =
        DirPersistence::open(&store_root).expect("open over a fresh store root should succeed");
    let result = persistence.get_blob(hash);
    let _ = std::fs::remove_dir_all(&store_root);
    result
}

#[test]
fn rust_decodes_dotnet_produced_zstd_frames() {
    let file: VectorsFile = serde_json::from_str(VECTORS_JSON)
        .expect("engine/commands/zstd-interop-vectors.v1.json must parse");

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let commands_root = Path::new(manifest_dir).join("../../engine/commands");

    let mut failures = Vec::new();
    let mut exercised = 0;

    for vector in &file.fixtures {
        if vector.consumer_runtime != "rust" {
            continue;
        }
        assert_eq!(
            vector.producer_runtime, "dotnet",
            "vector `{}`: rust is only exercised as a consumer of dotnet-produced frames in this test",
            vector.id
        );
        exercised += 1;

        let fixture_path = commands_root.join(&vector.fixture_path);
        let fixture_bytes = match std::fs::read(&fixture_path) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!(
                    "vector `{}`: could not read fixture at {fixture_path:?}: {e}",
                    vector.id
                ));
                continue;
            }
        };

        let expected_hash = parse_hash(&vector.decoded_plaintext_blake3);
        match get_blob_via_dir_persistence(&expected_hash, &fixture_bytes, &vector.id) {
            Some(decoded) => {
                if decoded.len() != vector.decoded_plaintext_len {
                    failures.push(format!(
                        "vector `{}`: decoded length {} != manifest decoded_plaintext_len {}",
                        vector.id,
                        decoded.len(),
                        vector.decoded_plaintext_len
                    ));
                }
                let actual_hash = Hash::of(&decoded).to_hex();
                if actual_hash != vector.decoded_plaintext_blake3 {
                    failures.push(format!(
                        "vector `{}`: decoded blake3 {actual_hash} != manifest decoded_plaintext_blake3 {}",
                        vector.id, vector.decoded_plaintext_blake3
                    ));
                }
            }
            None => {
                failures.push(format!(
                    "vector `{}`: DirPersistence::get_blob returned None — Rust failed to decode a dotnet-produced zstd frame",
                    vector.id
                ));
            }
        }
    }

    assert!(exercised > 0, "no rust-consumer vectors found in the manifest — did the schema change?");
    assert!(
        failures.is_empty(),
        "cross-runtime zstd interop (rust-as-consumer) failed:\n{}",
        failures.join("\n")
    );
}
