//! Slice 0.2 (`nodalmerge-studio/plans/blob-cas-remediation.md`, Phase 0) —
//! a shared, **non-hydrating** `BlobPersistence` fake plus the small
//! JSON/node builders the Phase-2 conformance tests need, so those tests
//! (`server/server/tests/blob_nonhydrating_conformance.rs`, and
//! `blob_relay_nonhydrating_backend.rs`, which used to define its own copy
//! of this fake) don't each hand-roll the same fixture.
//!
//! ## Why this shape
//!
//! `S3BlobStore::get_blob` (`server/s3-blobs/src/lib.rs:471`) always returns
//! `None` — bytes are never hydrated into the server process, by design
//! (`docs/BLOB_STORAGE_LAYOUT.md`). Any production code path that reaches
//! for `BlobPersistence::get_blob` and doesn't treat `None` as "ask the
//! resolve/hydrating path instead" silently breaks against a real S3
//! deployment. [`NonHydratingBackend`] reproduces exactly that shape —
//! in-memory, no network — so those code paths can be exercised without a
//! real bucket. It mirrors `S3BlobStore`'s Direct auth mode specifically:
//! `get_blob` always `None`, `has_blob` a real (here, in-memory) presence
//! check via a real bucket-`HEAD`-equivalent, `verify_uploaded` re-checks
//! that same presence set (mirrors `direct_head`-backed verification), and
//! `supports_presigned_urls` is unconditionally `true` (matching
//! `S3BlobStore`'s answer in *both* of its auth modes — see
//! `BlobPersistence::supports_presigned_urls`'s doc for why that flag can't
//! be inferred from `verify_uploaded`'s return value alone).
//!
//! ## The trap this deliberately avoids
//!
//! `BlobPersistence::persist_blob` (`server/server/src/store.rs`) is the
//! **only** method on the trait without a default body — every other method,
//! including `get_blob` (default `None`) and `has_blob` (default
//! `self.get_blob(hash).is_some()`), already has one. That means the
//! *laziest possible* fake — override nothing but `persist_blob` — silently
//! reproduces the real S3 bug shape: `has_blob` reports **every** blob
//! absent, live or not, because its default falls through to a `get_blob`
//! that never hydrates. See `naive_default_only_backend_reports_everything_absent`
//! in this crate's test suite for a pinned demonstration. `NonHydratingBackend`
//! below overrides `has_blob` on purpose, with a comment saying why, so this
//! crate doesn't repeat that trap by accident.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use ed25519_dalek::SigningKey;
use nodalmerge_core::{Hash, MapOp, Op, StateGraph, SyncNode};
use nodalmerge_server::store::BlobPersistence;

/// A `BlobPersistence` fake shaped exactly like `S3BlobStore`
/// (`server/s3-blobs/src/lib.rs:467`, Direct auth mode). See module docs.
#[derive(Debug, Default)]
pub struct NonHydratingBackend {
    present: Mutex<HashSet<Hash>>,
    /// Slice 1.3 — GC tombstones (hash → when it was first seen
    /// unreferenced), mirroring the tombstone objects the real
    /// `S3BlobStore::blob_gc_sweep` writes under
    /// `<prefix>.tombstones/blake3/`. See [`Self::blob_gc_sweep`].
    tombstones: Mutex<std::collections::HashMap<Hash, std::time::Instant>>,
    get_blob_calls: AtomicUsize,
}

impl NonHydratingBackend {
    /// Record `hash` as present without going through `persist_blob` — e.g.
    /// to simulate a blob that arrived via a presigned PUT this fake never
    /// sees bytes for (exactly how a real upload-confirm marks an S3 object
    /// live without the server ever holding its bytes).
    pub fn mark_present(&self, hash: Hash) {
        self.present.lock().unwrap().insert(hash);
    }

    /// How many times `get_blob` has been called. Lets a test assert a code
    /// path never attempted to hydrate bytes — the entire point of the
    /// HEAD/existence contract (slice 2.1).
    pub fn get_blob_calls(&self) -> usize {
        self.get_blob_calls.load(Ordering::SeqCst)
    }
}

impl BlobPersistence for NonHydratingBackend {
    /// Never hydrates — see module docs. Counts calls so tests can prove a
    /// production path avoided (or didn't avoid) reaching for bytes.
    fn get_blob(&self, _hash: &Hash) -> Option<Vec<u8>> {
        self.get_blob_calls.fetch_add(1, Ordering::SeqCst);
        None
    }

    /// Deliberately overridden — see module docs' "trap" section. A real
    /// bucket-existence check, independent of `get_blob`.
    fn has_blob(&self, hash: &Hash) -> bool {
        self.present.lock().unwrap().contains(hash)
    }

    fn persist_blob(&self, hash: &Hash, _bytes: &[u8]) {
        self.present.lock().unwrap().insert(*hash);
    }

    /// Mirrors `S3BlobStore`'s Direct-mode `verify_uploaded`: a real
    /// existence re-check, `Err` if the object genuinely isn't there.
    fn verify_uploaded(&self, _room_id: &str, hash: &Hash) -> Result<(), String> {
        if self.present.lock().unwrap().contains(hash) {
            Ok(())
        } else {
            Err("object missing after upload".into())
        }
    }

    /// `S3BlobStore` answers `true` in both its auth modes — see module docs.
    fn supports_presigned_urls(&self) -> bool {
        true
    }

    fn blobs_durable(&self) -> bool {
        true
    }

    /// A port of `S3BlobStore::blob_gc_sweep`'s **two-phase grace**
    /// (`server/s3-blobs/src/lib.rs`), in-memory: an unreferenced hash is
    /// tombstoned on the first sweep that sees it and deleted only once its
    /// tombstone is at least `grace` old; a hash that becomes live again has
    /// its tombstone cleared; `grace == 0` collapses the two phases in the
    /// same call. The real backend keeps its tombstones as bucket objects
    /// (`<prefix>.tombstones/blake3/<hex>.<unix_millis>`); this fake keeps
    /// them in a map. Both age against this process's clock.
    ///
    /// ## Read this before "fixing" a test against this method
    ///
    /// Until slice 1.3 (`blob-cas-remediation.md`, finding #3) this was a
    /// byte-for-byte port of the real backend's **bug**: `grace` bound and
    /// never read, a single-pass immediate delete. It exists so the GC sweep
    /// path can be exercised without a live S3/MinIO endpoint — which means
    /// it can only ever gate a **copy** of the backend's behavior, never the
    /// backend. A green test here is evidence about this file and nothing
    /// else. The real gates for the S3 two-phase grace are the
    /// bucket-touching ones in `server/s3-blobs/tests/minio_round_trip.rs`
    /// (`s3_blob_gc_two_phase_honors_grace_and_tombstones_first`,
    /// `s3_blob_gc_clears_tombstone_when_object_becomes_live_again`,
    /// `s3_min_physical_grace_floor_is_effective_end_to_end`); they run in
    /// CI via `.github/workflows/blob-s3-gc-minio.yml`. **If this port and
    /// `S3BlobStore` ever disagree, the MinIO tests are right and this file
    /// is wrong.**
    fn blob_gc_sweep(&self, live: &HashSet<Hash>, grace: std::time::Duration) -> usize {
        let now = std::time::Instant::now();
        let mut present = self.present.lock().unwrap();
        let mut tombs = self.tombstones.lock().unwrap();
        let mut deleted = 0usize;
        for hash in present.iter().copied().collect::<Vec<_>>() {
            if live.contains(&hash) {
                // Live: clear any leftover tombstone so a brief
                // unreference-then-rereference doesn't doom it next round.
                tombs.remove(&hash);
                continue;
            }
            match tombs.get(&hash) {
                Some(at) => {
                    if now.duration_since(*at) >= grace {
                        present.remove(&hash);
                        tombs.remove(&hash);
                        deleted += 1;
                    }
                }
                None => {
                    tombs.insert(hash, now);
                    if grace.is_zero() {
                        present.remove(&hash);
                        tombs.remove(&hash);
                        deleted += 1;
                    }
                }
            }
        }
        deleted
    }
}

// ─── Shared builders ────────────────────────────────────────────────────────
//
// Previously copy-pasted per test file (`tmpdir`/`make_setblob_node` in
// `server/server/tests/blob_gc.rs:27,40`; a larger set in
// `server/server/tests/studio_gc.rs:30-122`). There is no `tests/common/` in
// this workspace, so these live here instead of adding a fourth copy.

/// A fresh, uniquely-named temp directory under the OS temp root, created
/// and ready to use. Callers are expected to `std::fs::remove_dir_all` it
/// when done (best-effort; tests don't currently assert on cleanup).
pub fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("nodalmerge-blobstore-conformance-{tag}-{nanos}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Build a `TREE_OBJECT_FORMAT.md` v2 tree-object blob's bytes from an
/// `entries` array (each `{"n":..,"k":"f"|"d","h":..}`), returning its hash
/// alongside the bytes so the caller can `persist_blob`/`mark_present` it.
pub fn tree_v2_blob(entries: serde_json::Value) -> (Hash, Vec<u8>) {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "nodalmerge": "tree", "version": 2, "entries": entries
    }))
    .unwrap();
    (Hash::of(&bytes), bytes)
}

/// Build a `studio/repository-snapshot/v1` envelope's bytes, PascalCase per
/// `studio_live_hashes.rs`'s module docs (bare `JsonSerializer.Serialize`,
/// no naming policy).
#[allow(clippy::too_many_arguments)]
pub fn snapshot_envelope(
    snapshot_id: &str,
    repository_id: &str,
    generation: i64,
    created_at: &str,
    tree_hash: &str,
    tree_entries: Option<serde_json::Value>,
    source: Option<&str>,
    work_unit_id: Option<&str>,
) -> Vec<u8> {
    let payload = serde_json::json!({
        "SnapshotId": snapshot_id,
        "RepositoryId": repository_id,
        "TreeHash": tree_hash,
        "Generation": generation,
        "CreatedAt": created_at,
        "WorkUnitId": work_unit_id,
        "Source": source,
        "TreeEntries": tree_entries,
    });
    serde_json::to_vec(&serde_json::json!({
        "v": 1, "kind": "studio/repository-snapshot/v1", "payload": payload
    }))
    .unwrap()
}

/// Build a `studio/work-unit/v1` envelope's bytes (same casing rationale as
/// [`snapshot_envelope`]).
pub fn work_unit_envelope(
    work_unit_id: &str,
    status: i64,
    created_at: &str,
    updated_at: &str,
    repository_id: Option<&str>,
    metadata: Option<serde_json::Value>,
) -> Vec<u8> {
    let payload = serde_json::json!({
        "WorkUnitId": work_unit_id,
        "Status": status,
        "CreatedAt": created_at,
        "UpdatedAt": updated_at,
        "RepositoryId": repository_id,
        "Metadata": metadata,
    });
    serde_json::to_vec(&serde_json::json!({
        "v": 1, "kind": "studio/work-unit/v1", "payload": payload
    }))
    .unwrap()
}

/// Build a signed node carrying a single `SetBlob` op pointing at
/// `blob_hash` — the same shape a real client's blob upload produces.
/// Throwaway graph; the caller owns only the resulting node.
pub fn make_setblob_node(sk: &SigningKey, key: &str, blob_hash: Hash) -> SyncNode {
    let mut g = StateGraph::new();
    let id = g
        .apply_local(
            sk,
            0,
            vec![Op::Map(MapOp::SetBlob {
                key: key.into(),
                blob_hash,
            })],
        )
        .unwrap();
    g.get_nodes(&[id]).into_iter().next().unwrap().clone()
}

/// Build a signed node carrying a single `MapOp::Set{key, value}` — the
/// shape a promoted studio `MapSet` command produces. Throwaway graph.
pub fn make_mapset_node(sk: &SigningKey, key: &str, value: Vec<u8>) -> SyncNode {
    let mut g = StateGraph::new();
    let id = g
        .apply_local(sk, 0, vec![Op::Map(MapOp::Set { key: key.into(), value })])
        .unwrap();
    g.get_nodes(&[id]).into_iter().next().unwrap().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_hydrating_backend_reports_presence_via_has_blob_not_get_blob() {
        let backend = NonHydratingBackend::default();
        let hash = Hash::of(b"uploaded via a presigned PUT, never seen by this process");
        backend.mark_present(hash);

        assert!(backend.has_blob(&hash), "has_blob must answer via the real presence check");
        assert_eq!(backend.get_blob_calls(), 0, "has_blob must not fall through to get_blob");
        assert_eq!(backend.get_blob(&hash), None, "get_blob must never hydrate, even for a present hash");
        assert_eq!(backend.get_blob_calls(), 1);
    }

    #[test]
    fn non_hydrating_backend_verify_uploaded_mirrors_s3_direct_mode() {
        let backend = NonHydratingBackend::default();
        let hash = Hash::of(b"confirmed upload");
        assert!(
            backend.verify_uploaded("room-a", &hash).is_err(),
            "an object that was never marked present must fail verification"
        );
        backend.mark_present(hash);
        assert!(
            backend.verify_uploaded("room-a", &hash).is_ok(),
            "once present, verification must succeed"
        );
        assert!(backend.supports_presigned_urls());
    }

    /// Deliberate, documented reproduction of the trap described in this
    /// crate's module docs — NOT a regression this crate gates on (the
    /// trait's conservative defaults are correct behavior, not a bug): a
    /// fake (or a careless real backend) that overrides only `persist_blob`
    /// inherits `get_blob`'s `None` default and, through that,
    /// `has_blob`'s `self.get_blob(hash).is_some()` default — so it reports
    /// **every** blob absent, even ones it just "persisted". This is
    /// exactly the real S3 bug shape (findings #7 / #10 / the 2.1 gap): the
    /// difference between this naive fake and `NonHydratingBackend` above
    /// is a single deliberate `has_blob` override.
    #[test]
    fn naive_default_only_backend_reports_everything_absent() {
        #[derive(Debug, Default)]
        struct DefaultOnlyBackend {
            seen: Mutex<HashSet<Hash>>,
        }
        impl BlobPersistence for DefaultOnlyBackend {
            fn persist_blob(&self, hash: &Hash, _bytes: &[u8]) {
                self.seen.lock().unwrap().insert(*hash);
            }
            // get_blob / has_blob: intentionally NOT overridden.
        }

        let backend = DefaultOnlyBackend::default();
        let hash = Hash::of(b"genuinely stored");
        backend.persist_blob(&hash, b"genuinely stored");

        assert!(
            backend.seen.lock().unwrap().contains(&hash),
            "sanity: persist_blob really did record it"
        );
        assert!(
            !backend.has_blob(&hash),
            "the trap: has_blob()'s default reports this blob absent even though it was \
             just persisted, because it falls through to get_blob()'s default None"
        );
        assert_eq!(backend.get_blob(&hash), None);
    }
}
