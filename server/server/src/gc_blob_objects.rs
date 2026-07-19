//! S5.3 — local-filesystem `BlobObjectStore`.
//!
//! Backs the GC coordinator's hard-sweep HEAD/DELETE against the same
//! on-disk layout `store::DirPersistence` already uses
//! (`docs/BLOB_STORAGE_LAYOUT.md`), honoring both the identity (`<hex>`) and
//! v3 zstd (`<hex>.zst`) on-disk forms — at most one exists per hash, but
//! this checks/removes whichever is actually present rather than assuming.
//!
//! Object identity convention for this backend: `bucket = "local"` (a fixed
//! sentinel — there is no real bucket concept for on-disk storage) and
//! `object_key = <bare 64-lowercase-hex hash>` (not a path — this store
//! derives the actual on-disk path itself, the same way `DirPersistence`
//! does, so it can find the blob regardless of which encoding was on disk
//! when the row was inserted vs. now).

use std::path::{Path, PathBuf};

use nodalmerge_gc::contracts::BlobObjectStore;
use nodalmerge_gc::{GcError, GcResult};

use crate::store::{blob_relative_encoded_path, blob_relative_path, hash_from_hex};

/// Bucket sentinel this store expects in `AssetRecord::bucket` — see
/// [`gc_store::local_key_scheme`](crate::gc_store::local_key_scheme).
pub const LOCAL_BUCKET: &str = "local";

#[derive(Debug)]
pub struct LocalBlobObjectStore {
    /// The `DirPersistence` store root (parent of `blobs/`), i.e. the same
    /// path passed to `DirPersistence::open`.
    root: PathBuf,
}

impl LocalBlobObjectStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self { root: root.as_ref().to_path_buf() }
    }

    fn paths_for(&self, key: &str) -> GcResult<(PathBuf, PathBuf)> {
        let hash = hash_from_hex(key).ok_or_else(|| {
            GcError::Backend(format!("object key {key:?} is not a canonical 64-lowercase-hex hash"))
        })?;
        let blobs_root = self.root.join("blobs");
        Ok((
            blobs_root.join(blob_relative_path(&hash)),
            blobs_root.join(blob_relative_encoded_path(&hash)),
        ))
    }
}

impl BlobObjectStore for LocalBlobObjectStore {
    fn head(&self, _bucket: &str, key: &str) -> GcResult<bool> {
        let (identity, encoded) = self.paths_for(key)?;
        Ok(identity.is_file() || encoded.is_file())
    }

    fn delete(&self, _bucket: &str, key: &str) -> GcResult<()> {
        let (identity, encoded) = self.paths_for(key)?;
        if identity.is_file() {
            std::fs::remove_file(&identity).map_err(|e| GcError::Backend(e.to_string()))?;
        }
        if encoded.is_file() {
            std::fs::remove_file(&encoded).map_err(|e| GcError::Backend(e.to_string()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodalmerge_core::Hash;

    fn tmpdir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("nodalmerge-blobobjstore-test-{nanos}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn head_and_delete_identity_form() {
        let dir = tmpdir();
        let blake3_dir = dir.join("blobs").join("blake3");
        std::fs::create_dir_all(&blake3_dir).unwrap();
        let hash = Hash::of(b"hello");
        std::fs::write(blake3_dir.join(hash.to_hex()), b"hello").unwrap();

        let store = LocalBlobObjectStore::new(&dir);
        assert!(store.head(LOCAL_BUCKET, &hash.to_hex()).unwrap());
        store.delete(LOCAL_BUCKET, &hash.to_hex()).unwrap();
        assert!(!store.head(LOCAL_BUCKET, &hash.to_hex()).unwrap());
    }

    #[test]
    fn head_and_delete_zstd_form() {
        let dir = tmpdir();
        let blake3_dir = dir.join("blobs").join("blake3");
        std::fs::create_dir_all(&blake3_dir).unwrap();
        let hash = Hash::of(b"hello-zstd");
        std::fs::write(blake3_dir.join(format!("{}.zst", hash.to_hex())), b"compressed-bytes").unwrap();

        let store = LocalBlobObjectStore::new(&dir);
        assert!(store.head(LOCAL_BUCKET, &hash.to_hex()).unwrap());
        store.delete(LOCAL_BUCKET, &hash.to_hex()).unwrap();
        assert!(!store.head(LOCAL_BUCKET, &hash.to_hex()).unwrap());
    }

    #[test]
    fn missing_blob_head_is_false_and_delete_is_ok() {
        let dir = tmpdir();
        let store = LocalBlobObjectStore::new(&dir);
        let hash = Hash::of(b"never stored");
        assert!(!store.head(LOCAL_BUCKET, &hash.to_hex()).unwrap());
        store.delete(LOCAL_BUCKET, &hash.to_hex()).unwrap(); // no-op, not an error
    }

    #[test]
    fn non_canonical_key_is_an_error() {
        let dir = tmpdir();
        let store = LocalBlobObjectStore::new(&dir);
        assert!(store.head(LOCAL_BUCKET, "not-a-hash").is_err());
    }
}
