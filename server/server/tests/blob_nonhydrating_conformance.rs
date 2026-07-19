//! Slice 0.2 (`nodalmerge-studio/plans/blob-cas-remediation.md`, Phase 0) —
//! the non-hydrating backend conformance harness. `nodalmerge_blobstore_conformance::NonHydratingBackend`
//! reproduces `S3BlobStore`'s real shape (`get_blob` always `None`, `has_blob`
//! a real presence check) without a network. This file exercises three of
//! the four named surfaces against it — GC live-set, archive export,
//! tree-walk — and pins the mechanism finding #10 blames for "GC never runs
//! on S3". The fourth surface (HEAD/existence) is:
//!   - already correct on the Rust side today (see
//!     `blob_relay_nonhydrating_backend.rs`'s green
//!     `head_reports_present_via_has_blob_even_though_get_blob_never_hydrates`,
//!     which now imports the same promoted fake this file uses) — no RED
//!     test needed here;
//!   - broken on the .NET side (finding 2.1's whole point — `IBlobStoreProvider`
//!     has no existence check at all), covered by
//!     `hosts/dotnet/tests/NodalMerge.DotNetHost.Tests/NonHydratingBlobBackendConformanceTests.cs`.
//!
//! ## RED-test convention
//! Gated tests assert the CORRECT (post-fix) behavior and are skipped today
//! via `#[ignore = "RED: ..."]`, naming the slice that removes the gate. See
//! this file's own tests below for the exact wording and the real failure
//! each one produces today (captured in slice 0.2's final report).
//!
//! ## A finding about the plan's own scoping (see slice 0.2's final report)
//! "GC live-set" and "tree-walk" are named as two separate scenarios in the
//! plan's Phase 0 §0.2, but they collapse to the *same* acceptance gate:
//! the only place `BlobPersistence::get_blob` is reachable from the studio
//! GC coordinator is via `tree_walk::walk_tree` inside
//! `studio_live_hashes::collect_studio_live_hashes`. There is no separate
//! "GC live-set" code path that touches blob bytes without going through the
//! tree walk. Both scenarios below therefore gate slice 2.2, not two
//! different slices.

use std::sync::Arc;

use ed25519_dalek::SigningKey;
use nodalmerge_blobstore_conformance::{
    make_mapset_node, make_setblob_node, snapshot_envelope, tmp_dir, tree_v2_blob,
    NonHydratingBackend,
};
use nodalmerge_core::{canonical_hash, ArchiveWsResponse, Hash};
use nodalmerge_s3_blobs::{S3Auth, S3BlobStore, S3BlobStoreConfig};
use nodalmerge_server::archive_adapter::{
    process_archive_describe, process_archive_import, process_archive_validate,
};
use nodalmerge_server::archive_export::build_external_manifest_document;
use nodalmerge_server::room::{import_nodes, Room, Rooms};
use nodalmerge_server::store::{BlobPersistence, Composite, DirPersistence, SharedPersistence};
use nodalmerge_server::studio_live_hashes::collect_studio_live_hashes;
use nodalmerge_server::tree_walk::{walk_tree, TreeWalkError};

/// Pair a real (`DirPersistence`) node half with the fake blob half — the
/// exact composition `Composite<N, B>` exists for (`store.rs` module docs),
/// and the exact shape `S3BlobStore` deployments actually run in production
/// (real Postgres/Mongo/SQLite nodes, S3 blobs).
fn non_hydrating_persistence(tag: &str) -> SharedPersistence {
    let dir = tmp_dir(tag);
    Arc::new(Composite::new(
        DirPersistence::open(&dir).expect("dir persistence should open"),
        NonHydratingBackend::default(),
    ))
}

// ─── tree-walk: documents today's mechanism (green, not gated) ────────────

/// **Rewritten by slice 2.2.** This test used to assert the *bug*: that
/// `walk_tree` over a non-hydrating backend always failed with `MissingBlob`
/// even though `has_blob` said the tree was right there. That was finding
/// #10's mechanism, and 2.2 removed it — the walk now resolves tree objects
/// through `BlobPersistence::hydrate_blob` instead of `get_blob`.
///
/// It keeps its original job (pinning the mechanism the walk uses), so what
/// it pins now is the pair of properties that make the fix legitimate rather
/// than a hole in the offloading policy:
///
/// 1. the walk resolves tree objects on a backend whose `get_blob` never
///    hydrates, and
/// 2. `get_blob`'s non-hydrating contract is **untouched** — the walk must
///    never call it. Asserted via `get_blob_calls()`, not via a return value:
///    a value-only assertion would pass against a walk that called `get_blob`
///    and ignored the answer, proving nothing about the policy.
#[test]
fn tree_walk_resolves_via_hydrate_blob_without_touching_the_get_blob_policy() {
    let backend = NonHydratingBackend::default();
    let file_hash = Hash::of(b"a file the tree names but the walk never fetches");
    let (tree_hash, tree_bytes) =
        tree_v2_blob(serde_json::json!([{"n": "a.txt", "k": "f", "h": file_hash.to_hex()}]));
    backend.persist_blob(&tree_hash, &tree_bytes).unwrap();
    assert!(backend.has_blob(&tree_hash), "sanity: the backend agrees the tree blob exists");

    let live = walk_tree(&backend, &tree_hash).expect(
        "finding #10: the walk must resolve tree objects through the hydrating path even \
         though this backend's get_blob never returns bytes",
    );
    assert!(live.contains(&tree_hash), "the tree object itself is live");
    assert!(live.contains(&file_hash), "the file the tree names is live");
    assert_eq!(live.len(), 2, "root + file, nothing else: {live:?}");

    assert_eq!(
        backend.get_blob_calls(),
        0,
        "slice 2.2 must not have reached for get_blob: its non-hydrating contract is \
         deliberately unchanged, and the tree walk resolves through hydrate_blob instead"
    );
}

/// The `Missing` half of the same seam: a tree object that genuinely isn't
/// there must still fail closed. Without this, "the walk stopped failing"
/// could be satisfied by a walk that never fails at all — which would be a
/// far worse bug than #10 (GC would compute a short live set and delete live
/// blobs). Mirrors `minio_round_trip.rs`'s
/// `s3_direct_tree_walk_reports_a_genuinely_absent_tree_as_missing` against
/// the real backend.
#[test]
fn tree_walk_still_fails_closed_on_a_genuinely_absent_tree_object() {
    let backend = NonHydratingBackend::default();
    let phantom = Hash::of(b"never uploaded anywhere");
    let err = walk_tree(&backend, &phantom).expect_err("an absent root must fail the walk");
    assert!(
        matches!(err, TreeWalkError::MissingBlob(_)),
        "absent is Missing, not Unresolvable: {err:?}"
    );
}

/// Slice 2.2 — the `Composite` forwarding pin.
///
/// `Composite<N, B>` is the production wiring (`Composite<Postgres, S3BlobStore>`),
/// and `hydrate_blob`/`get_blob_hydrates` are **defaulted** trait methods. A
/// missing forward therefore compiles silently and answers *plausibly*: the
/// default would report `get_blob_hydrates() == true` for an S3-backed
/// composite, call `get_blob` (correctly `None`), and hand back `Missing` —
/// reinstating finding #10 with `S3BlobStore::hydrate_blob` sitting unused.
/// This is the exact shape of the `RemoteBlobLinkAggregator` hole that would
/// have voided slice 2.1, so it gets its own pin rather than relying on the
/// GC test above to notice.
#[test]
fn composite_forwards_hydration_seam_to_the_blob_half() {
    let persistence = non_hydrating_persistence("composite-forwarding");
    assert!(
        !persistence.get_blob_hydrates(),
        "Composite must forward get_blob_hydrates to its blob half — inheriting the \
         `true` default here would make the trait's own hydrate_blob default look correct"
    );

    let (tree_hash, tree_bytes) = tree_v2_blob(serde_json::json!([]));
    persistence.persist_blob(&tree_hash, &tree_bytes).unwrap();
    let bytes = persistence
        .hydrate_blob(&tree_hash)
        .expect("Composite must forward hydrate_blob to the blob half's real override");
    assert_eq!(bytes, tree_bytes, "and return that half's bytes, not a default's answer");
}

// ─── GC live-set (finding #10, gates slice 2.2) ────────────────────────────

#[tokio::test]
async fn studio_gc_live_set_over_cas_tree_snapshot_does_not_fail_closed_on_non_hydrating_backend() {
    let persistence = non_hydrating_persistence("gc-live-set");
    let sk = SigningKey::from_bytes(&[0xF1u8; 32]);
    let rooms = Rooms::new(SigningKey::from_bytes(&[0xF2u8; 32]), Arc::clone(&persistence), 512, 0, 0);
    let room = rooms.get_or_create("repo/repo-nonhydrating").await;

    let file_bytes = b"repo file behind a cas tree, on a backend that never hydrates".to_vec();
    let file_hash = Hash::of(&file_bytes);
    persistence.persist_blob(&file_hash, &file_bytes).unwrap(); // marks present; never re-readable
    let (tree_hash, tree_bytes) =
        tree_v2_blob(serde_json::json!([{"n": "a.txt", "k": "f", "h": file_hash.to_hex()}]));
    persistence.persist_blob(&tree_hash, &tree_bytes).unwrap(); // the tree object is *also* a blob

    let node = make_mapset_node(
        &sk,
        "studio/repository-snapshot/v1/gen-0",
        snapshot_envelope(
            "gen-0",
            "repo-nonhydrating",
            0,
            "2026-01-01T00:00:00Z",
            &tree_hash.to_hex(),
            None,
            Some("Bootstrap"),
            None,
        ),
    );
    let (accepted, _, errs) = import_nodes(&room, vec![node]).await;
    assert_eq!(accepted, 1, "expected the snapshot entry node to be accepted: {errs:?}");

    let live = collect_studio_live_hashes(&rooms, 30).await;
    assert!(
        live.is_ok(),
        "finding #10: GC must not fail closed forever just because the tree/file blobs \
         live on a backend that never hydrates bytes into the server process (S3's real \
         shape) — got {live:?}"
    );

    // Slice 2.2 strengthened the gate beyond `is_ok()`. On its own, `is_ok()`
    // is satisfied by a "fix" that resolves nothing and returns an empty live
    // set — which is not GC working, it is GC deleting every blob in the pool
    // on the next sweep. The whole point of resolving the tree is *what comes
    // back*, so assert the contents.
    let live = live.unwrap();
    assert!(
        live.contains(&tree_hash.to_hex()),
        "the tree object must be in the live set (it is itself a CAS blob, and a GC that \
         doesn't protect it deletes the snapshot's index): {live:?}"
    );
    assert!(
        live.contains(&file_hash.to_hex()),
        "the file blob the tree names must be in the live set — this is the hash that only \
         a successful tree *resolution* can produce, so it is what distinguishes a real fix \
         from a walk that silently returned nothing: {live:?}"
    );
}

// ─── Archive export (finding #7, gates slice 2.3) ─────────────────────────
//
// ## What "the same digest" has to mean here, and why `dir == s3` alone does
// ## not gate the slice
//
// `blobs_digest` is `canonical_hash` over a `BTreeMap<hex, bytes>`. The bug
// (finding #7) is that a non-hydrating backend contributes **nothing** to that
// map, so the digest is the digest of the *empty map* — a fixed, well-known
// value meaning "this room references no blobs". Asserting only
// `dir_digest == s3_digest` is therefore satisfied by a "fix" that makes
// **both** sides empty, which is finding #7 spread to Dir as well. Every test
// below pins the digest against `expected_blobs_digest(...)` — computed here,
// from the bytes, independently of the production code — and additionally
// asserts it is not `empty_blobs_digest()`. Agreement is necessary; agreement
// *on the real content* is the actual property.

/// The `blobs_digest` a correct implementation must produce for `blobs`,
/// computed from the bytes rather than by asking the code under test.
/// Mirrors `archive_export.rs`'s formula (`canonical_hash` over a
/// `BTreeMap<hex, bytes>`, `sha256:`-prefixed).
fn expected_blobs_digest(blobs: &[(Hash, &[u8])]) -> String {
    let map: std::collections::BTreeMap<String, Vec<u8>> =
        blobs.iter().map(|(h, b)| (h.to_hex(), b.to_vec())).collect();
    format!("sha256:{}", canonical_hash(&map).to_hex())
}

/// The digest of a room that references **no** blobs at all — i.e. exactly
/// what finding #7 silently produces for an S3-backed room that references
/// plenty. Never equal to a correct digest for a non-empty room; asserted
/// against, not for.
fn empty_blobs_digest() -> String {
    expected_blobs_digest(&[])
}

/// A **Delegate-mode** `S3BlobStore` — the real backend, not a fake.
///
/// The 0.2 fake (`NonHydratingBackend`) deliberately models S3 **Direct** mode
/// only (it holds bytes and can hand them back, mirroring a bucket `GET`).
/// Delegate mode is the structurally-unhydratable case: no bucket credentials,
/// and delegate presign protocol v1 has no bytes-fetch op. It needs no network
/// and no Docker — it never opens a connection — so the *real* backend is used
/// here directly. Same construction as `s3-blobs/tests/delegate_gc.rs`.
fn delegate_persistence(tag: &str) -> SharedPersistence {
    let cfg = S3BlobStoreConfig {
        bucket: "app-owned-bucket".into(),
        auth: S3Auth::delegate("https://app.example.com/presign", None),
        ..Default::default()
    };
    let s3 = S3BlobStore::new(cfg).expect("build Delegate-mode S3BlobStore");
    Arc::new(Composite::new(
        DirPersistence::open(tmp_dir(tag)).expect("dir persistence should open"),
        s3,
    ))
}

async fn room_referencing(
    persistence: &SharedPersistence,
    room_id: &str,
    signer: &SigningKey,
    blob_hash: Hash,
) -> Arc<Room> {
    let room: Arc<Room> = Room::new(room_id.to_string(), Arc::clone(persistence), 64);
    let (accepted, _, errs) =
        import_nodes(&room, vec![make_setblob_node(signer, "asset", blob_hash)]).await;
    assert_eq!(accepted, 1, "setup: the SetBlob node must be accepted: {errs:?}");
    room
}

async fn describe(room: &Arc<Room>, room_id: &str) -> Result<String, String> {
    match process_archive_describe(
        room,
        room_id,
        &serde_json::json!({"archive_ref": format!("room://{room_id}")}),
    )
    .await
    {
        Ok(ArchiveWsResponse::DescribeResult(r)) => Ok(r.payload_digest_set.blobs),
        Ok(other) => panic!("expected archive.describe.result, got {other:?}"),
        Err(rejected) => Err(format!("{rejected:?}")),
    }
}

/// **The slice 2.3 gate.** Ungated by 2.3 — and strengthened by it: see this
/// section's header for why the original `dir == s3` assertion was satisfiable
/// by two empty digests.
#[tokio::test]
async fn archive_describe_blobs_digest_matches_between_hydrating_and_non_hydrating_backends() {
    let signer = SigningKey::from_bytes(&[0x91u8; 32]);
    let blob_bytes = b"a real file the room referenced via SetBlob".to_vec();
    let blob_hash = Hash::of(&blob_bytes);
    let room_id = "archive-digest-room";
    let expected = expected_blobs_digest(&[(blob_hash, &blob_bytes)]);
    assert_ne!(
        expected,
        empty_blobs_digest(),
        "sanity: this room references a blob, so its correct digest cannot be the \
         empty-set digest — if this ever fires, every assertion below is vacuous"
    );

    // Hydrating baseline: DirPersistence actually holds the bytes.
    let dir = tmp_dir("archive-digest-parity-dir");
    let dir_persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&dir).expect("dir persistence should open"));
    dir_persistence.persist_blob(&blob_hash, &blob_bytes).unwrap();
    let dir_room = room_referencing(&dir_persistence, room_id, &signer, blob_hash).await;
    let dir_digest = describe(&dir_room, room_id)
        .await
        .expect("describe over DirPersistence must succeed");

    // Non-hydrating (S3 Direct-shaped): same node, same blob hash present in
    // the "bucket", but `get_blob` is always None.
    let s3_shaped_persistence = non_hydrating_persistence("archive-digest-parity-s3shape");
    s3_shaped_persistence.persist_blob(&blob_hash, &blob_bytes).unwrap();
    let s3_room = room_referencing(&s3_shaped_persistence, room_id, &signer, blob_hash).await;
    let s3_digest = describe(&s3_room, room_id).await.expect(
        "describe over the non-hydrating backend must succeed: the bytes ARE reachable \
         (Direct mode does a real bucket GET), so there is nothing to fail about — the \
         only reason it produced an empty-set digest before 2.3 was that it asked \
         get_blob, which never hydrates by policy",
    );

    assert_eq!(
        dir_digest, expected,
        "sanity: the hydrating baseline must already agree with the independently \
         computed digest — if it doesn't, the comparison below proves nothing"
    );
    assert_eq!(
        s3_digest, expected,
        "finding #7: archive describe must not compute blobs_digest over an empty set \
         when the backend never hydrates bytes via get_blob. Note this is asserted \
         against the digest of the REAL BYTES, not merely against the Dir side — \
         `dir == s3` alone is satisfied by both being empty, which is the same bug"
    );
    assert_ne!(
        s3_digest,
        empty_blobs_digest(),
        "finding #7, stated directly: the empty-set digest is exactly what the bug \
         produced. It must not be what the fix produces"
    );
    assert_eq!(dir_digest, s3_digest, "and, restated: Dir- and S3-backed exports agree");
}

/// The same property on the **other** export entry point.
///
/// `archive_export.rs::build_external_manifest_document` is a *separate*
/// function with its own `filter_map` (`blobs_digest_for_referenced`), and it
/// is what `archive.export` actually signs and writes to disk — the gate above
/// only covers `archive_adapter.rs`'s `load_archive_from_ref`
/// (`archive.describe`). Fixing one and not the other would leave finding #7
/// fully alive on the path that produces the durable artifact while the gate
/// went green.
#[tokio::test]
async fn archive_export_manifest_blobs_digest_matches_between_hydrating_and_non_hydrating_backends()
{
    let signer = SigningKey::from_bytes(&[0x92u8; 32]);
    let blob_bytes = b"a real file that must reach the exported manifest's digest".to_vec();
    let blob_hash = Hash::of(&blob_bytes);
    let room_id = "archive-manifest-digest-room";
    let expected = expected_blobs_digest(&[(blob_hash, &blob_bytes)]);

    let dir = tmp_dir("archive-manifest-digest-dir");
    let dir_persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&dir).expect("dir persistence should open"));
    dir_persistence.persist_blob(&blob_hash, &blob_bytes).unwrap();
    let _dir_room = room_referencing(&dir_persistence, room_id, &signer, blob_hash).await;
    let dir_manifest =
        build_external_manifest_document(&*dir_persistence, room_id, &signer, &"0".repeat(64), 0, &[])
            .expect("manifest build over DirPersistence must succeed");

    let s3_shaped = non_hydrating_persistence("archive-manifest-digest-s3shape");
    s3_shaped.persist_blob(&blob_hash, &blob_bytes).unwrap();
    let _s3_room = room_referencing(&s3_shaped, room_id, &signer, blob_hash).await;
    let s3_manifest =
        build_external_manifest_document(&*s3_shaped, room_id, &signer, &"0".repeat(64), 0, &[])
            .expect("manifest build over the non-hydrating backend must succeed");

    assert_eq!(
        dir_manifest.payload_digest_set.blobs, expected,
        "sanity: the hydrating baseline must match the independently computed digest"
    );
    assert_eq!(
        s3_manifest.payload_digest_set.blobs, expected,
        "finding #7 on the archive.export path: the written manifest's blobs digest must \
         be over the real bytes, not over an empty set"
    );
    assert_ne!(
        s3_manifest.payload_digest_set.blobs,
        empty_blobs_digest(),
        "the empty-set digest is the bug's signature; it must not be the fix's output"
    );
}

// ─── Fail-loud: a backend that structurally cannot hydrate (Delegate) ─────
//
// These use the REAL `S3BlobStore` in Delegate mode, not the 0.2 fake: the
// fake models Direct mode only, and teaching it Delegate would be the 1.3
// trap (proving a property about the fake). Delegate needs no network.

/// An S3 **Delegate**-mode deployment cannot read blob bytes at all — ever.
/// The plan's two options for 2.3 are "fail the export" or "warning + manifest
/// marker"; for this case only the former is honest. A manifest whose blobs
/// digest is the empty-set digest, carrying a marker admitting it, is still a
/// manifest that **mismatches every Dir-backed peer's digest for the same
/// room** — i.e. finding #7's actual damage, merely annotated. So: fail, and
/// say why in terms an operator can act on.
#[tokio::test]
async fn archive_describe_fails_loud_when_the_backend_structurally_cannot_hydrate() {
    let signer = SigningKey::from_bytes(&[0x93u8; 32]);
    let blob_hash = Hash::of(b"a file sitting in the app's own bucket");
    let room_id = "archive-delegate-room";
    let persistence = delegate_persistence("archive-delegate-describe");
    // Deliberately NOT persisted here: in Delegate mode the bytes reached the
    // app's bucket via an app-minted presigned PUT this server never saw. The
    // object exists. This server simply cannot read it.
    let room = room_referencing(&persistence, room_id, &signer, blob_hash).await;

    let err = describe(&room, room_id).await.expect_err(
        "Delegate mode cannot materialize the referenced blob, so describe must fail \
         rather than report a digest computed over an empty set",
    );

    // 1. Not a lost blob. Same reasoning as delegate_gc.rs: reporting an
    //    intact, present object as missing sends an operator hunting for
    //    nothing and hides the real (config) cause.
    let lower = err.to_lowercase();
    assert!(
        !lower.contains("not present") && !lower.contains("missing blob"),
        "an unreadable-but-present blob must not be reported as data loss: {err}"
    );
    // 2. Named as a configuration fact, and actionable.
    assert!(
        lower.contains("deployment configuration"),
        "the rejection must name this as a config problem, not a generic failure: {err}"
    );
    for needle in ["delegate", "direct-mode", "hydrate"] {
        assert!(
            lower.contains(needle),
            "the rejection must be actionable and mention {needle:?}; got: {err}"
        );
    }
    // 3. The hash, so the operator knows *which* object.
    assert!(
        err.contains(&blob_hash.to_hex()),
        "the rejection must name the blob it could not resolve: {err}"
    );
}

/// Same property on the `archive.export` (manifest-document) path — see the
/// describe/export split noted above.
#[tokio::test]
async fn archive_export_manifest_fails_loud_when_the_backend_structurally_cannot_hydrate() {
    let signer = SigningKey::from_bytes(&[0x94u8; 32]);
    let blob_hash = Hash::of(b"another file in the app's own bucket");
    let room_id = "archive-delegate-manifest-room";
    let persistence = delegate_persistence("archive-delegate-export");
    let _room = room_referencing(&persistence, room_id, &signer, blob_hash).await;

    let err = build_external_manifest_document(
        &*persistence,
        room_id,
        &signer,
        &"0".repeat(64),
        0,
        &[],
    )
    .expect_err("Delegate mode must fail the manifest build, not sign an empty-set digest");

    let lower = err.to_lowercase();
    assert!(
        lower.contains("deployment configuration") && lower.contains("delegate"),
        "the error must name the config cause: {err}"
    );
    assert!(
        err.contains(&blob_hash.to_hex()),
        "the error must name the blob it could not resolve: {err}"
    );
}

/// A **transient** read failure is neither a config fact nor data loss — but
/// it must still fail the export rather than silently shrink the digest. This
/// is the case that makes "just drop what you can't read" indefensible: a
/// single 503 on one object would otherwise mint a signed manifest whose
/// digest permanently disagrees with every peer, with no error anywhere.
///
/// Driven through the 0.2 fake's `mark_present` (an object present in the
/// "bucket" whose bytes this process never saw), which its `hydrate_blob`
/// reports as `HydrateError::Backend`.
#[tokio::test]
async fn archive_export_fails_when_a_referenced_blob_read_fails_transiently() {
    let signer = SigningKey::from_bytes(&[0x95u8; 32]);
    let blob_hash = Hash::of(b"present in the bucket, unreadable this tick");
    let room_id = "archive-transient-room";

    let backend = NonHydratingBackend::default();
    backend.mark_present(blob_hash);
    let persistence: SharedPersistence = Arc::new(Composite::new(
        DirPersistence::open(tmp_dir("archive-transient")).expect("dir persistence should open"),
        backend,
    ));
    let room = room_referencing(&persistence, room_id, &signer, blob_hash).await;

    let err = describe(&room, room_id)
        .await
        .expect_err("a transient backend read failure must fail the export, not shrink the digest");
    assert!(
        err.to_lowercase().contains("could not be read"),
        "the rejection must say the read failed (retryable), not that the blob is gone: {err}"
    );
    assert!(err.contains(&blob_hash.to_hex()), "the rejection must name the blob: {err}");
}

// ─── Specificity control: what must still SUCCEED ─────────────────────────

/// **The control that stops "fail loud" from becoming "fail always".**
///
/// A referenced hash whose bytes are genuinely absent (`HydrateError::Missing`)
/// is a fact about the **data**, not about the backend — a Dir-backed peer and
/// an S3-backed peer both see it, and both must therefore produce the *same*
/// digest: the digest over the blobs they do have. Failing here would
/// permanently brick export for any room carrying a dangling SetBlob (a node
/// committed for an upload that never completed), with no operator remedy —
/// and would regress today's Dir behavior, which is not what finding #7 is
/// about.
///
/// So: Missing is tolerated (counted + warned), Unhydratable/Backend are fatal.
/// This test is what proves that distinction is real rather than a story: it
/// fails if 2.3 collapses all three `HydrateError` variants into "fail".
#[tokio::test]
async fn archive_export_tolerates_a_genuinely_absent_referenced_blob_and_keeps_digest_parity() {
    let signer = SigningKey::from_bytes(&[0x96u8; 32]);
    let present_bytes = b"this one was uploaded".to_vec();
    let present_hash = Hash::of(&present_bytes);
    let absent_hash = Hash::of(b"a SetBlob whose upload never completed");
    let room_id = "archive-dangling-room";
    // The correct digest covers the present blob and *excludes* the absent
    // one — the same answer on both backends, which is the whole point.
    let expected = expected_blobs_digest(&[(present_hash, &present_bytes)]);
    assert_ne!(expected, empty_blobs_digest(), "sanity: one blob is present");

    for (tag, persistence) in [
        (
            "dir",
            Arc::new(DirPersistence::open(tmp_dir("archive-dangling-dir")).expect("open"))
                as SharedPersistence,
        ),
        ("s3shape", non_hydrating_persistence("archive-dangling-s3shape")),
    ] {
        persistence.persist_blob(&present_hash, &present_bytes).unwrap();
        let room: Arc<Room> = Room::new(room_id.to_string(), Arc::clone(&persistence), 64);
        let (accepted, _, errs) = import_nodes(
            &room,
            vec![
                make_setblob_node(&signer, "present", present_hash),
                make_setblob_node(&signer, "dangling", absent_hash),
            ],
        )
        .await;
        assert_eq!(accepted, 2, "setup ({tag}): both nodes must be accepted: {errs:?}");

        let digest = describe(&room, room_id).await.unwrap_or_else(|e| {
            panic!(
                "({tag}) a genuinely-absent referenced blob is a data fact both backends \
                 share, not a backend fault — export must still succeed, or a dangling \
                 SetBlob bricks the room's export forever: {e}"
            )
        });
        assert_eq!(
            digest, expected,
            "({tag}) the digest must cover the blob that IS there and exclude the one \
             that isn't — identically on both backends"
        );
    }
}

// ─── room.rs — the third call site, which wants the OPPOSITE policy ───────

/// Room hydration is **not** an export, and 2.3 must not treat it like one.
///
/// `room.rs`'s blob load fills the room's *in-memory* store on open. On an S3
/// backend the right answer is to load **nothing**: pulling every file payload
/// a room references into server memory on every room open is exactly the cost
/// offloading exists to avoid, and `hydrate_blob`'s own contract says callers
/// wanting file bytes should mint a URL instead (clients do; see 2.4). So this
/// call site keeps its behavior and gains only *visibility*.
///
/// Asserted via `get_blob_calls()` rather than a return value: 2.3 makes the
/// decision by asking `get_blob_hydrates()` up front, so a non-hydrating
/// backend is never asked for bytes it will never give. A value-based
/// assertion ("no blobs loaded") would pass against the old code too, and
/// would also pass against a future change that hydrated everything and threw
/// it away.
#[tokio::test]
async fn room_hydrate_does_not_pull_file_payloads_through_a_non_hydrating_backend() {
    let sk = SigningKey::from_bytes(&[0x97u8; 32]);
    let dir = tmp_dir("room-hydrate-nonhydrating");
    let blob_bytes = b"a large file payload that must never be pulled into server memory".to_vec();
    let blob_hash = Hash::of(&blob_bytes);
    let room_id = "room-hydrate-nonhydrating";

    // Persist the node through a first Rooms instance so a second one has
    // something to hydrate from disk.
    {
        let persistence: SharedPersistence = Arc::new(Composite::new(
            DirPersistence::open(&dir).expect("open"),
            NonHydratingBackend::default(),
        ));
        let rooms = Rooms::new(
            SigningKey::from_bytes(&[0x98u8; 32]),
            Arc::clone(&persistence),
            512,
            0,
            0,
        );
        let room = rooms.get_or_create(room_id).await;
        let (accepted, _, errs) =
            import_nodes(&room, vec![make_setblob_node(&sk, "asset", blob_hash)]).await;
        assert_eq!(accepted, 1, "setup: node must be accepted: {errs:?}");
    }

    let backend = Arc::new(NonHydratingBackend::default());
    backend.persist_blob(&blob_hash, &blob_bytes).unwrap();
    let persistence: SharedPersistence = Arc::new(Composite::new(
        DirPersistence::open(&dir).expect("reopen"),
        CountingBlobHalf(Arc::clone(&backend)),
    ));
    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0x99u8; 32]),
        Arc::clone(&persistence),
        512,
        0,
        0,
    );
    let room = rooms.get_or_create(room_id).await;

    // Room hydration is spawned; wait for the nodes to land so we know the
    // blob-load stage ran (it is sequenced after apply_remote_batch).
    for _ in 0..200 {
        if !room.graph.read().await.all_nodes().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        !room.graph.read().await.all_nodes().is_empty(),
        "setup: the room must have hydrated its nodes, or this test proves nothing"
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(
        backend.get_blob_calls(),
        0,
        "room open must not reach for blob bytes on a backend that declares itself \
         non-hydrating: on S3 that is a per-room-open pull of every referenced file \
         payload into server memory. 2.3 fixes the ARCHIVE call sites; this one is \
         deliberately left non-hydrating and must stay that way"
    );
}

/// Thin `BlobPersistence` forwarder so the test above can hold an `Arc` to the
/// fake (to read its counter) while `Composite` takes its blob half by value.
#[derive(Debug)]
struct CountingBlobHalf(Arc<NonHydratingBackend>);

impl BlobPersistence for CountingBlobHalf {
    fn get_blob(&self, hash: &Hash) -> Option<Vec<u8>> {
        self.0.get_blob(hash)
    }
    fn persist_blob(&self, hash: &Hash, bytes: &[u8]) -> Result<(), nodalmerge_server::store::PersistBlobError> {
        self.0.persist_blob(hash, bytes)
    }
    fn has_blob(&self, hash: &Hash) -> bool {
        self.0.has_blob(hash)
    }
    fn get_blob_hydrates(&self) -> bool {
        self.0.get_blob_hydrates()
    }
    fn hydrate_blob(&self, hash: &Hash) -> Result<Vec<u8>, nodalmerge_server::store::HydrateError> {
        self.0.hydrate_blob(hash)
    }
    fn supports_presigned_urls(&self) -> bool {
        self.0.supports_presigned_urls()
    }
    fn blobs_durable(&self) -> bool {
        self.0.blobs_durable()
    }
}

// ─── 2.3's own blast radius: paths that must NOT start hydrating ──────────
//
// Found while implementing 2.3, not predicted by the plan. `load_archive_from_ref`
// is shared by FOUR entry points, and only some of them want blob bytes:
//
//   archive.describe  — needs the bytes (the blobs digest is over them)
//   archive.export    — needs the bytes (same)
//   archive.validate  — **never reads `loaded.blobs` at all**
//   archive.import    — needs them only for `full_apply`; `metadata_only`
//                       discards them
//
// Resolving unconditionally (which is what a naive 2.3 does, and what the
// first cut of this slice did) is invisible on `DirPersistence` — a wasted
// local read — but on S3 it downloads every file payload the room references
// in order to throw it away, and on **Delegate** mode it turns a working
// `archive.validate` / `metadata_only` import into a hard failure. That is a
// functional regression *introduced by the fix*, in the exact "silently
// hydrating file payloads" direction the offloading policy exists to prevent.
//
// These two tests pin the boundary. They fail against the naive fix.

/// `archive.validate` never touches `loaded.blobs`, so it must not resolve
/// them — on Delegate mode, resolving would fail an operation that has no
/// business needing bytes.
#[tokio::test]
async fn archive_validate_does_not_hydrate_blobs_and_works_on_a_delegate_backend() {
    let signer = SigningKey::from_bytes(&[0x9Au8; 32]);
    let blob_hash = Hash::of(b"a file this validate has no business reading");
    let room_id = "archive-validate-delegate";
    let persistence = delegate_persistence("archive-validate-delegate");
    let room = room_referencing(&persistence, room_id, &signer, blob_hash).await;

    let response = process_archive_validate(
        &room,
        room_id,
        &serde_json::json!({"archive_ref": format!("room://{room_id}"), "mode": "metadata_only"}),
    )
    .await;

    assert!(
        matches!(response, ArchiveWsResponse::ValidateResult(_)),
        "archive.validate reads only nodes — it must not resolve blob bytes, and so must \
         keep working on a backend that can never hand them over. Slice 2.3 must not \
         widen its fail-loud policy onto a path that does not need the bytes: {response:?}"
    );
}

/// The same boundary on `archive.import`: `metadata_only` discards the blobs,
/// so it must not pull them. Asserted on the S3-shaped fake via
/// `get_blob_calls`-style evidence (here: the Delegate backend simply cannot
/// serve them, so success *is* the evidence that nothing was pulled).
#[tokio::test]
async fn archive_metadata_only_import_does_not_hydrate_blobs_on_a_delegate_backend() {
    let signer = SigningKey::from_bytes(&[0x9Bu8; 32]);
    let blob_hash = Hash::of(b"a file a metadata_only import discards anyway");
    let room_id = "archive-import-delegate";
    let persistence = delegate_persistence("archive-import-delegate");
    let room = room_referencing(&persistence, room_id, &signer, blob_hash).await;

    let response = process_archive_import(
        &room,
        room_id,
        // NB: the field is `import_mode`, not `mode` (see
        // `parse_archive_import_request`, which silently defaults to
        // "full_apply"). Spelling it `mode` here made this test exercise
        // full_apply and "prove" the opposite of its name.
        &serde_json::json!({
            "archive_ref": format!("room://{room_id}"),
            "import_mode": "metadata_only",
        }),
    )
    .await;

    match response {
        ArchiveWsResponse::ImportCompleted(imported) => {
            assert_eq!(
                imported.imported_blobs, 0,
                "metadata_only imports no blobs by definition — if this is nonzero the \
                 test is not exercising the path it claims to"
            );
        }
        other => panic!(
            "a metadata_only import discards blob bytes, so it must not resolve them and \
             must keep working on a backend that can never hand them over. 2.3's \
             fail-loud policy belongs on the paths that NEED the bytes, not on every \
             caller of load_archive_from_ref: {other:?}"
        ),
    }
}
