//! Cross-runtime interop test — Rust reads a golden blob-store fixture
//! (`engine/commands/fixtures/blob-layout-v1/`) that the .NET mirror
//! (`hosts/dotnet/tests/NodalMerge.DotNetHost.Tests/BlobLayoutInteropTests.cs`)
//! reads too. Neither runtime writes the fixture; each just proves it can
//! interpret the other's canonical on-disk shape. See
//! docs/BLOB_STORAGE_LAYOUT.md.
//!
//! The fixture represents a bare `<blob_root>` (`blake3/`, `.tombstones/`,
//! `.layout-v2`); `DirPersistence` additionally expects that root nested
//! one level under `<store_root>/blobs/`, so this test copies the fixture
//! into a fresh temp dir under a `blobs/` subdirectory before opening it.

use nodalmerge_core::Hash;
use nodalmerge_server::store::{BlobPersistence, DirPersistence};
use std::path::{Path, PathBuf};

const FIXTURE_ROOT: &str = "../../engine/commands/fixtures/blob-layout-v1";

const BLOB_ONE_HASH: &str = "30db28cf6e7c0df6cf6226f68f364649236cd3a6470823ea78d759f95da49da4";
const BLOB_ONE_CONTENT: &[u8] = b"fixture blob one";
const BLOB_TWO_HASH: &str = "70dd491e791ac7b073c5ac5e343b9bc803f3d1e1b7038c1233531070a07b0b31";
const BLOB_TWO_CONTENT: &[u8] = b"fixture blob two";

fn parse_hash(hex: &str) -> Hash {
    let mut out = [0u8; 32];
    let bytes = hex.as_bytes();
    for i in 0..32 {
        let hi = (bytes[2 * i] as char).to_digit(16).unwrap() as u8;
        let lo = (bytes[2 * i + 1] as char).to_digit(16).unwrap() as u8;
        out[i] = (hi << 4) | lo;
    }
    Hash(out)
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let path = entry.path();
        let dest = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &dest);
        } else {
            std::fs::copy(&path, &dest).unwrap();
        }
    }
}

fn tmpdir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("nodalmerge-blob-interop-{tag}-{nanos}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn rust_reads_the_shared_golden_fixture() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = Path::new(manifest_dir).join(FIXTURE_ROOT);
    assert!(
        fixture.is_dir(),
        "fixture not found at {fixture:?} — did engine/commands/fixtures/blob-layout-v1/ move?"
    );

    let store_root = tmpdir("rust-reads-fixture");
    copy_dir_recursive(&fixture, &store_root.join("blobs"));

    let persistence = DirPersistence::open(&store_root).expect("open over the fixture should succeed");

    let blob_one = persistence
        .get_blob(&parse_hash(BLOB_ONE_HASH))
        .expect("blob one must be readable from the fixture");
    assert_eq!(blob_one, BLOB_ONE_CONTENT);

    let blob_two = persistence
        .get_blob(&parse_hash(BLOB_TWO_HASH))
        .expect("blob two must be readable from the fixture even though it is also tombstoned");
    assert_eq!(blob_two, BLOB_TWO_CONTENT);

    // Migration must be a no-op: the fixture already carries `.layout-v2`.
    assert!(
        store_root.join("blobs").join(".migration-skipped").exists() == false,
        "a canonical fixture must never produce migration quarantine output"
    );

    let _ = std::fs::remove_dir_all(&store_root);
}
