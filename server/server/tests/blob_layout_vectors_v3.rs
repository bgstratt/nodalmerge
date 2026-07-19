//! Asserts the Rust file-store v3 at-rest-encoding layout
//! (`docs/BLOB_STORAGE_LAYOUT.md` §8) against the `encoding_vectors_v3` and
//! `name_conformance_vectors_v3` arrays in
//! `engine/commands/blob-layout-vectors.v1.json`. Sibling to
//! `blob_layout_vectors.rs` (which covers the pre-v3 arrays); split out so
//! a v3-unaware harness can keep ignoring these arrays without touching the
//! original file. The .NET mirror is
//! `hosts/dotnet/tests/NodalMerge.DotNetHost.Tests/BlobLayoutParityTests.cs`.

use std::sync::Arc;

use nodalmerge_core::Hash;
use nodalmerge_server::store::{
    blob_relative_encoded_path, blob_relative_path, is_canonical_blob_name_v3,
    parse_blob_entry_name, tombstone_relative_path, BlobCompressionConfig, BlobEncoding,
    BlobPersistence, DirPersistence, SharedPersistence,
};
use serde::Deserialize;

const VECTORS_JSON: &str = include_str!("../../../engine/commands/blob-layout-vectors.v1.json");

#[derive(Debug, Deserialize)]
struct VectorsFile {
    encoding_vectors_v3: Vec<EncodingVector>,
    name_conformance_vectors_v3: Vec<NameVector>,
}

#[derive(Debug, Deserialize)]
struct EncodingVector {
    id: String,
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    encoded_relative_path: Option<String>,
    #[serde(default)]
    tombstone_relative_path: Option<String>,
    #[serde(default)]
    preferred_relative_path: Option<String>,
    // s3_key vectors are covered by server/s3-blobs's own test; ignored here.
    #[serde(default)]
    #[allow(dead_code)]
    s3_key: Option<String>,
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

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("nodalmerge-blob-v3-{tag}-{nanos}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn encoding_vectors_v3_match_rust_derivation() {
    let file: VectorsFile =
        serde_json::from_str(VECTORS_JSON).expect("engine/commands/blob-layout-vectors.v1.json must parse");

    let mut failures = Vec::new();

    for vector in &file.encoding_vectors_v3 {
        let Some(hash_hex) = &vector.hash else {
            continue; // s3-key-only vector, nothing for this harness to check
        };
        let hash = parse_hash(hash_hex);

        if let Some(expected) = &vector.encoded_relative_path {
            let actual = normalize(blob_relative_encoded_path(&hash));
            if &actual != expected {
                failures.push(format!(
                    "vector `{}`: expected encoded_relative_path {expected}, got {actual}",
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

        if let Some(expected) = &vector.preferred_relative_path {
            // "both files exist, identity wins" — assert against the pure
            // path formula (this is a derivation vector, not a behavior
            // test; the both-exist behavior itself is covered below by
            // `both_exist_prefers_identity_at_runtime`).
            let actual = normalize(blob_relative_path(&hash));
            if &actual != expected {
                failures.push(format!(
                    "vector `{}`: expected preferred_relative_path {expected}, got {actual}",
                    vector.id
                ));
            }
        }
    }

    for vector in &file.name_conformance_vectors_v3 {
        let actual = is_canonical_blob_name_v3(&vector.name);
        if actual != vector.canonical {
            failures.push(format!(
                "vector `{}`: expected canonical={}, got canonical={} (name `{}`)",
                vector.id, vector.canonical, actual, vector.name
            ));
        }

        // Cross-check against the sweep's own recognizer, since that's what
        // GC actually calls — the two must never drift.
        let via_parse = parse_blob_entry_name(&vector.name).is_some();
        if via_parse != vector.canonical {
            failures.push(format!(
                "vector `{}`: parse_blob_entry_name disagrees with is_canonical_blob_name_v3 (name `{}`)",
                vector.id, vector.name
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "v3 encoding/name-conformance vectors drifted from Rust derivation:\n{}",
        failures.join("\n")
    );
}

#[test]
fn parse_blob_entry_name_recognizes_identity_and_zstd() {
    let hex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let hash = parse_hash(hex);

    let (parsed_hash, encoding) = parse_blob_entry_name(hex).expect("bare hex must parse");
    assert_eq!(parsed_hash, hash);
    assert_eq!(encoding, BlobEncoding::Identity);

    let zst_name = format!("{hex}.zst");
    let (parsed_hash, encoding) = parse_blob_entry_name(&zst_name).expect(".zst must parse");
    assert_eq!(parsed_hash, hash);
    assert_eq!(encoding, BlobEncoding::Zstd);

    assert!(parse_blob_entry_name(&format!("{hex}.gz")).is_none());
    assert!(parse_blob_entry_name(&format!("{hex}.zst.zst")).is_none());
}

// ─── Round-trip / heuristic / both-exist runtime behavior ─────────────────

fn compressible_payload() -> Vec<u8> {
    // Well above min_bytes, highly repetitive -> compresses well under any
    // reasonable zstd level.
    b"the quick brown fox jumps over the lazy dog. ".repeat(500)
}

fn incompressible_payload() -> Vec<u8> {
    // Pseudo-random bytes (deterministic, no external RNG dependency):
    // simple xorshift-style generator, well above min_bytes.
    let mut state: u64 = 0x9E3779B97F4A7C15;
    let mut out = Vec::with_capacity(8192);
    for _ in 0..8192 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push((state & 0xff) as u8);
    }
    out
}

#[tokio::test]
async fn compression_on_round_trip_smaller_on_disk_and_byte_identical() {
    let dir = tmpdir("roundtrip");
    let cfg = BlobCompressionConfig {
        enabled: true,
        level: 3,
        min_bytes: 4096,
    };
    let store = DirPersistence::open_with_compression(&dir, cfg).unwrap();

    let payload = compressible_payload();
    let hash = Hash::of(&payload);
    store.persist_blob(&hash, &payload).unwrap();

    let encoded_path = dir.join("blobs").join("blake3").join(format!("{}.zst", hash.to_hex()));
    let identity_path = dir.join("blobs").join("blake3").join(hash.to_hex());
    assert!(encoded_path.is_file(), "compressible payload must be stored as .zst");
    assert!(!identity_path.is_file(), "identity form must not also exist");

    let on_disk_len = std::fs::metadata(&encoded_path).unwrap().len() as usize;
    assert!(
        on_disk_len < payload.len(),
        "compressed form ({on_disk_len} bytes) must be smaller than input ({} bytes)",
        payload.len()
    );

    let loaded = store.get_blob(&hash);
    assert_eq!(loaded, Some(payload), "get_blob must return byte-identical input");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn compression_on_corrupt_zst_on_disk_yields_none() {
    let dir = tmpdir("corrupt");
    let cfg = BlobCompressionConfig {
        enabled: true,
        level: 3,
        min_bytes: 4096,
    };
    let store = DirPersistence::open_with_compression(&dir, cfg).unwrap();

    let payload = compressible_payload();
    let hash = Hash::of(&payload);
    store.persist_blob(&hash, &payload).unwrap();

    let encoded_path = dir.join("blobs").join("blake3").join(format!("{}.zst", hash.to_hex()));
    assert!(encoded_path.is_file());
    std::fs::write(&encoded_path, b"not a zstd frame at all").unwrap();

    assert_eq!(
        store.get_blob(&hash),
        None,
        "a corrupt .zst frame must be treated as missing, never served"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn compression_on_small_blob_stays_identity() {
    let dir = tmpdir("small");
    let cfg = BlobCompressionConfig {
        enabled: true,
        level: 3,
        min_bytes: 4096,
    };
    let store = DirPersistence::open_with_compression(&dir, cfg).unwrap();

    let payload = b"tiny payload, well under min_bytes".to_vec();
    assert!(payload.len() < cfg.min_bytes);
    let hash = Hash::of(&payload);
    store.persist_blob(&hash, &payload).unwrap();

    let identity_path = dir.join("blobs").join("blake3").join(hash.to_hex());
    let encoded_path = dir.join("blobs").join("blake3").join(format!("{}.zst", hash.to_hex()));
    assert!(identity_path.is_file(), "blob under min_bytes must stay identity");
    assert!(!encoded_path.is_file());

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn compression_on_incompressible_blob_stays_identity() {
    let dir = tmpdir("incompressible");
    let cfg = BlobCompressionConfig {
        enabled: true,
        level: 3,
        min_bytes: 4096,
    };
    let store = DirPersistence::open_with_compression(&dir, cfg).unwrap();

    let payload = incompressible_payload();
    assert!(payload.len() >= cfg.min_bytes);
    let hash = Hash::of(&payload);
    store.persist_blob(&hash, &payload).unwrap();

    let identity_path = dir.join("blobs").join("blake3").join(hash.to_hex());
    let encoded_path = dir.join("blobs").join("blake3").join(format!("{}.zst", hash.to_hex()));
    assert!(
        identity_path.is_file(),
        "incompressible payload must skip the .zst form and stay identity"
    );
    assert!(!encoded_path.is_file());

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn both_exist_prefers_identity_at_runtime() {
    // Writers never produce this, but readers must tolerate it: if both
    // forms exist for one hash, get_blob must return the identity bytes.
    let dir = tmpdir("both-exist");
    let store = DirPersistence::open(&dir).unwrap(); // compression off; we hand-write both forms

    let payload = compressible_payload();
    let hash = Hash::of(&payload);
    let blake3_dir = dir.join("blobs").join("blake3");
    std::fs::create_dir_all(&blake3_dir).unwrap();
    std::fs::write(blake3_dir.join(hash.to_hex()), &payload).unwrap();
    let compressed = zstd::stream::encode_all(&payload[..], 3).unwrap();
    std::fs::write(blake3_dir.join(format!("{}.zst", hash.to_hex())), &compressed).unwrap();

    let loaded = store.get_blob(&hash).expect("identity form must be read");
    assert_eq!(loaded, payload);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn compression_off_reader_still_reads_zst_written_by_compression_on_store() {
    // "Reading is always encoding-aware regardless of the write config" —
    // a store opened with compression off must still be able to read a
    // .zst blob that some other (compression-on) writer produced at the
    // same root.
    let dir = tmpdir("interop-write-on-read-off");
    let on_cfg = BlobCompressionConfig {
        enabled: true,
        level: 3,
        min_bytes: 4096,
    };
    let writer = DirPersistence::open_with_compression(&dir, on_cfg).unwrap();
    let payload = compressible_payload();
    let hash = Hash::of(&payload);
    writer.persist_blob(&hash, &payload).unwrap();
    drop(writer);

    let reader: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap()); // compression off
    let loaded = reader.get_blob(&hash);
    assert_eq!(
        loaded,
        Some(payload),
        "compression-off reader must still decode a .zst blob on disk"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
