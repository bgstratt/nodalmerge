//! Slice 0.2 (`nodalmerge-studio/plans/blob-cas-remediation.md`, Phase 0) —
//! a shared, **non-hydrating** `BlobPersistence` fake plus the small
//! JSON/node builders the Phase-2 conformance tests need, so those tests
//! (`server/server/tests/blob_nonhydrating_conformance.rs`, and
//! `blob_relay_nonhydrating_backend.rs`, which used to define its own copy
//! of this fake) don't each hand-roll the same fixture.
//!
//! ## Why this shape
//!
//! `S3BlobStore::get_blob` always returns `None` — bytes are never hydrated
//! into the server process, by design (`docs/BLOB_STORAGE_LAYOUT.md`). Any
//! production code path that reaches for `BlobPersistence::get_blob` and
//! doesn't treat `None` as "ask the resolve/hydrating path instead" silently
//! breaks against a real S3 deployment. [`NonHydratingBackend`] reproduces
//! exactly that shape — in-memory, no network — so those code paths can be
//! exercised without a real bucket. It mirrors `S3BlobStore`'s Direct auth
//! mode specifically: `get_blob` always `None`, `has_blob` a real (here,
//! in-memory) presence check via a real bucket-`HEAD`-equivalent,
//! `verify_uploaded` re-checks that same presence set (mirrors
//! `direct_head`-backed verification), and `supports_presigned_urls` is
//! unconditionally `true` (matching `S3BlobStore`'s answer in *both* of its
//! auth modes — see `BlobPersistence::supports_presigned_urls`'s doc for why
//! that flag can't be inferred from `verify_uploaded`'s return value alone).
//!
//! ## "Non-hydrating" is about `get_blob`, not about bytes (slice 2.2)
//!
//! Read this before concluding the `hydrate_blob` override below contradicts
//! the name of this type. It does not, and neither does the real backend.
//! `S3BlobStore` has a live S3 client — `get_blob`'s `None` is **policy**
//! (keep large *file* payloads out of the server process), not incapacity.
//! Slice 2.2 added `BlobPersistence::hydrate_blob` for the narrow class of
//! objects the server must parse to do its own job — today only **tree
//! objects**, a few hundred bytes of JSON, fetched by `tree_walk::walk_tree`,
//! which never fetches a file blob at all (v1 entries and v2 `"f"` entries are
//! terminal; only `"d"` entries are read). So Direct mode hydrates those with
//! a real bucket `GET`, `get_blob` stays `None` forever, and this fake mirrors
//! both. Delegate mode genuinely cannot, and is not modeled here — it needs no
//! network to test, so it is gated in `s3-blobs`' own unit tests.
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
use nodalmerge_server::store::{BlobPersistence, HydrateError, PersistBlobError};

/// A `BlobPersistence` fake shaped exactly like `S3BlobStore`
/// (`server/s3-blobs/src/lib.rs:467`, Direct auth mode). See module docs.
#[derive(Debug, Default)]
pub struct NonHydratingBackend {
    present: Mutex<HashSet<Hash>>,
    /// Slice 2.2 — the bytes a real bucket would hold for a hash, so this
    /// fake can port `S3BlobStore`'s Direct-mode [`BlobPersistence::hydrate_blob`]
    /// (a real bucket `GET`). `present` stays the source of truth for
    /// existence: a hash can be present *without* bytes here (see
    /// [`Self::mark_present`]), which is a limitation of the fake, not a
    /// state a real bucket can be in.
    bytes: Mutex<std::collections::HashMap<Hash, Vec<u8>>>,
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

    /// Ports `S3BlobStore::get_blob_hydrates` — the declaration that makes
    /// the *reason* `get_blob` is `None` a fact the trait can act on
    /// (policy, not corruption). Without this the trait's default
    /// `hydrate_blob` would report every blob this fake holds as `Missing`.
    fn get_blob_hydrates(&self) -> bool {
        false
    }

    /// Slice 2.2 — ports `S3BlobStore`'s **Direct-mode** `hydrate_blob` (a
    /// real bucket `GET`). Note the asymmetry with `get_blob` directly above,
    /// and that it is the *real* backend's asymmetry, not an invention of
    /// this fake: S3 refuses to hydrate *file payloads* into the server
    /// process by policy, but it has a live client and can always fetch the
    /// small tree objects the server must parse itself. See
    /// `S3BlobStore::hydrate_blob`'s docs.
    ///
    /// **This is a port and only a port.** The real gate for the S3 hydrating
    /// read is `server/s3-blobs/tests/minio_round_trip.rs`
    /// (`s3_direct_tree_walk_resolves_tree_objects_from_the_real_bucket`,
    /// `s3_direct_tree_walk_recurses_into_nested_directories_from_the_real_bucket`,
    /// `s3_direct_tree_walk_reports_a_genuinely_absent_tree_as_missing`),
    /// which runs against a real bucket in CI. Per this file's standing rule:
    /// **if this port and `S3BlobStore` ever disagree, the MinIO tests are
    /// right and this file is wrong.**
    ///
    /// Delegate mode is deliberately *not* modeled here — this fake mirrors
    /// Direct mode only (see module docs). Delegate's `Unhydratable` is gated
    /// in `s3-blobs`' own unit tests, where it needs no network.
    fn hydrate_blob(&self, hash: &Hash) -> Result<Vec<u8>, HydrateError> {
        if !self.present.lock().unwrap().contains(hash) {
            return Err(HydrateError::Missing);
        }
        match self.bytes.lock().unwrap().get(hash) {
            Some(b) => Ok(b.clone()),
            // Present but byte-less: only reachable via `mark_present`, which
            // models an object this process never saw the bytes of. A real
            // bucket would simply return them; this fake cannot invent them.
            // Reported as a loud Backend error rather than Missing so a test
            // that lands here fails with the reason instead of quietly
            // exercising the not-found path and proving the wrong thing.
            None => Err(HydrateError::Backend(format!(
                "NonHydratingBackend: {} was marked present via mark_present() without bytes, \
                 so this fake cannot hydrate it. A real bucket WOULD return the object here — \
                 this is a limitation of the fake. Use persist_blob() if the test needs the \
                 bytes back.",
                hash.to_hex()
            ))),
        }
    }

    /// Ports Direct-mode `persist_blob` (a real bucket `PUT`): the bytes are
    /// now in the "bucket", so a later `hydrate_blob` returns them —
    /// exactly as MinIO does. (Before slice 2.2 this fake discarded them,
    /// which was fine only because nothing could read them back.)
    ///
    /// Slice 4.2 made the port **fallible** to match the real backend's new
    /// signature; an in-memory insert cannot fail, so this port always
    /// returns `Ok` — the real backend's failure classes
    /// (`Backend`/`Unavailable`, and the success/failure↔presence coupling)
    /// are gated by `server/s3-blobs/tests/put_durability.rs` (stalled
    /// listener, Docker-free) and the MinIO suites. Per this file's
    /// standing rule: **if this port and `S3BlobStore` ever disagree, the
    /// MinIO tests are right and this file is wrong.** The alignment this
    /// crate CAN pin — a persist that does not succeed must not leave a
    /// phantom presence — is `persist_failure_must_not_leave_a_phantom_presence`
    /// below.
    fn persist_blob(&self, hash: &Hash, bytes: &[u8]) -> Result<(), PersistBlobError> {
        self.present.lock().unwrap().insert(*hash);
        self.bytes.lock().unwrap().insert(*hash, bytes.to_vec());
        Ok(())
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
        // Slice 2.2 — a deleted object's bytes go with it, so a post-sweep
        // `hydrate_blob` reports Missing exactly as a real bucket would
        // (`minio_round_trip.rs` asserts the real backend's equivalent).
        let mut bytes = self.bytes.lock().unwrap();
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
                        bytes.remove(&hash);
                        tombs.remove(&hash);
                        deleted += 1;
                    }
                }
                None => {
                    tombs.insert(hash, now);
                    if grace.is_zero() {
                        present.remove(&hash);
                        bytes.remove(&hash);
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
            fn persist_blob(&self, hash: &Hash, _bytes: &[u8]) -> Result<(), PersistBlobError> {
                self.seen.lock().unwrap().insert(*hash);
                Ok(())
            }
            // get_blob / has_blob: intentionally NOT overridden.
        }

        let backend = DefaultOnlyBackend::default();
        let hash = Hash::of(b"genuinely stored");
        backend.persist_blob(&hash, b"genuinely stored").unwrap();

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

    /// Slice 2.2 — pins the reason `hydrate_blob`'s **default** is safe to
    /// have at all, which is the one question the slice's trait shape had to
    /// answer (a default that is "correct for hydrating backends and silently
    /// wrong for non-hydrating ones" is exactly the hole that nearly voided
    /// slice 2.1).
    ///
    /// The defence is `get_blob_hydrates()`: a **declaration**, not an
    /// inference. It could not have been inferred from `get_blob() == None &&
    /// has_blob() == true` — on `DirPersistence` that same signature means a
    /// **corrupt** blob (its `get_blob` verifies BLAKE3 on read and returns
    /// `None` on mismatch, while `has_blob` is a plain `is_file()`), so
    /// guessing would have relabelled every corrupt file blob as "this
    /// backend cannot hydrate".
    ///
    /// So the residual trap is narrower than 2.1's, and this test pins both
    /// halves of it:
    #[test]
    fn hydrate_blob_default_is_loud_for_a_backend_that_declares_non_hydration() {
        /// Declares the policy, doesn't implement a read — the plausible
        /// mistake for a future non-hydrating backend.
        #[derive(Debug, Default)]
        struct DeclaresButDoesntOverride;
        impl BlobPersistence for DeclaresButDoesntOverride {
            fn persist_blob(&self, _hash: &Hash, _bytes: &[u8]) -> Result<(), PersistBlobError> {
                Ok(())
            }
            fn get_blob(&self, _hash: &Hash) -> Option<Vec<u8>> {
                None
            }
            fn get_blob_hydrates(&self) -> bool {
                false
            }
        }

        let err = DeclaresButDoesntOverride
            .hydrate_blob(&Hash::of(b"x"))
            .expect_err("must not claim to have hydrated anything");
        assert!(
            matches!(err, HydrateError::Unhydratable { .. }),
            "the default must fail LOUD for a declared non-hydrating backend — reporting \
             Missing here is finding #10 reintroduced for the next backend: {err:?}"
        );

        // The other half: a backend that declares nothing keeps the plain,
        // correct behavior. If this ever became Unhydratable, every hydrating
        // backend's genuinely-absent blob would be misreported as a config
        // problem — the mirror-image false alarm.
        #[derive(Debug, Default)]
        struct OrdinaryHydratingBackend;
        impl BlobPersistence for OrdinaryHydratingBackend {
            fn persist_blob(&self, _hash: &Hash, _bytes: &[u8]) -> Result<(), PersistBlobError> {
                Ok(())
            }
        }
        assert_eq!(
            OrdinaryHydratingBackend.hydrate_blob(&Hash::of(b"x")),
            Err(HydrateError::Missing),
            "absent on a hydrating backend is Missing, not Unhydratable"
        );
    }

    /// The residual trap, stated honestly rather than fixed: a backend that
    /// overrides `get_blob` to `None` as policy and forgets **both**
    /// `get_blob_hydrates` and `hydrate_blob` still reports `Missing`. The
    /// type system cannot catch this (every method but `persist_blob` is
    /// defaulted, and 2.2 deliberately did not change that — it would break
    /// every external implementor). This test exists so the gap is a *known,
    /// pinned* one rather than a surprise for whoever writes the next
    /// non-hydrating backend; the mitigation is `get_blob`'s and
    /// `get_blob_hydrates`' docs, which tell you to flip the flag.
    #[test]
    fn hydrate_blob_default_cannot_save_a_backend_that_declares_nothing() {
        #[derive(Debug, Default)]
        struct SilentlyNonHydrating {
            present: Mutex<HashSet<Hash>>,
        }
        impl BlobPersistence for SilentlyNonHydrating {
            fn persist_blob(&self, hash: &Hash, _bytes: &[u8]) -> Result<(), PersistBlobError> {
                self.present.lock().unwrap().insert(*hash);
                Ok(())
            }
            fn get_blob(&self, _hash: &Hash) -> Option<Vec<u8>> {
                None // policy — but never declared via get_blob_hydrates()
            }
            fn has_blob(&self, hash: &Hash) -> bool {
                self.present.lock().unwrap().contains(hash)
            }
        }

        let backend = SilentlyNonHydrating::default();
        let hash = Hash::of(b"in the bucket, unreadable here");
        backend.persist_blob(&hash, b"in the bucket, unreadable here").unwrap();

        assert!(backend.has_blob(&hash), "sanity: the backend knows it holds this");
        assert_eq!(
            backend.hydrate_blob(&hash),
            Err(HydrateError::Missing),
            "KNOWN GAP, pinned deliberately: without the get_blob_hydrates() declaration the \
             default cannot tell this backend's policy-None apart from DirPersistence's \
             corruption-None, so it answers Missing. Overriding get_blob to None without \
             also overriding get_blob_hydrates is the one remaining way to reintroduce \
             finding #10 in a new backend."
        );
    }

    /// Slice 4.2 (finding #8) — the fallible-persist scenario, the half of
    /// the new `persist_blob -> Result` contract a network-free port CAN
    /// gate: **failure and presence must agree.** A backend whose
    /// `persist_blob` returns `Err` must not afterwards report the hash via
    /// `has_blob` — a phantom presence is finding #8 one layer down (the
    /// PUT dedupe branch would answer `200 OK` for bytes nobody holds, and
    /// GC would protect/track an object that never existed).
    ///
    /// The real backends are proven first, per 1.3's ordering rule:
    /// `DirPersistence`'s fs-failure→`Err`→`has_blob == false` is pinned
    /// against a real filesystem in
    /// `server/server/tests/blob_put_durability.rs`, and `S3BlobStore`'s
    /// timeout→`Unavailable` in `server/s3-blobs/tests/put_durability.rs`
    /// (stalled listener; `has_blob` is a real bucket HEAD there, so a
    /// failed PUT leaves nothing to find). This test pins the same
    /// invariant in trait-shape form so the next backend/fake author trips
    /// over it here instead of in production. And the mirror half:
    /// `NonHydratingBackend`'s own persist is `Ok` and DOES record
    /// presence + bytes, exactly as a bucket PUT that returned 200 would.
    #[test]
    fn persist_failure_must_not_leave_a_phantom_presence() {
        /// The wrong shape on purpose: marks present BEFORE the write can
        /// fail — the same mark-then-write ordering the pre-4.2 PUT handler
        /// had with its inventory row.
        #[derive(Debug, Default)]
        struct MarksBeforeFailing {
            present: Mutex<HashSet<Hash>>,
        }
        impl BlobPersistence for MarksBeforeFailing {
            fn persist_blob(&self, hash: &Hash, _bytes: &[u8]) -> Result<(), PersistBlobError> {
                self.present.lock().unwrap().insert(*hash);
                Err(PersistBlobError::Backend("disk full (simulated)".into()))
            }
            fn has_blob(&self, hash: &Hash) -> bool {
                self.present.lock().unwrap().contains(hash)
            }
        }

        let broken = MarksBeforeFailing::default();
        let hash = Hash::of(b"never actually stored");
        let err = broken
            .persist_blob(&hash, b"never actually stored")
            .expect_err("this backend always fails");
        assert!(matches!(err, PersistBlobError::Backend(_)));
        assert!(
            broken.has_blob(&hash),
            "sanity: this deliberately-wrong backend exhibits the phantom — a correct \
             backend must NOT (the assertion that matters is the one below, on the port)"
        );

        // The port itself: success records presence AND readable bytes;
        // nothing is ever present that persist didn't succeed for.
        let backend = NonHydratingBackend::default();
        let hash = Hash::of(b"a real 200 from the bucket");
        assert!(!backend.has_blob(&hash), "nothing present before persist");
        backend
            .persist_blob(&hash, b"a real 200 from the bucket")
            .expect("in-memory persist cannot fail");
        assert!(backend.has_blob(&hash), "Ok(()) means present");
        assert_eq!(
            backend.hydrate_blob(&hash).expect("bytes must be readable back"),
            b"a real 200 from the bucket".to_vec(),
        );
    }
}
