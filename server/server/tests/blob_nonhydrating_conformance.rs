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
use nodalmerge_core::{ArchiveWsResponse, Hash};
use nodalmerge_server::archive_adapter::process_archive_describe;
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
    backend.persist_blob(&tree_hash, &tree_bytes);
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
    persistence.persist_blob(&tree_hash, &tree_bytes);
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
    persistence.persist_blob(&file_hash, &file_bytes); // marks present; never re-readable
    let (tree_hash, tree_bytes) =
        tree_v2_blob(serde_json::json!([{"n": "a.txt", "k": "f", "h": file_hash.to_hex()}]));
    persistence.persist_blob(&tree_hash, &tree_bytes); // the tree object is *also* a blob

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

#[tokio::test]
#[ignore = "RED: fails until slice 2.3 — see nodalmerge-studio/plans/blob-cas-remediation.md"]
async fn archive_describe_blobs_digest_matches_between_hydrating_and_non_hydrating_backends() {
    let signer = SigningKey::from_bytes(&[0x91u8; 32]);
    let blob_bytes = b"a real file the room referenced via SetBlob".to_vec();
    let blob_hash = Hash::of(&blob_bytes);
    let room_id = "archive-digest-room";

    // Hydrating baseline: DirPersistence actually holds the bytes.
    let dir = tmp_dir("archive-digest-parity-dir");
    let dir_persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&dir).expect("dir persistence should open"));
    dir_persistence.persist_blob(&blob_hash, &blob_bytes);
    let dir_room: Arc<Room> = Room::new(room_id.to_string(), Arc::clone(&dir_persistence), 64);
    let (accepted, _, errs) =
        import_nodes(&dir_room, vec![make_setblob_node(&signer, "asset", blob_hash)]).await;
    assert_eq!(accepted, 1, "hydrating room: node must be accepted: {errs:?}");

    let dir_describe = process_archive_describe(
        &dir_room,
        room_id,
        &serde_json::json!({"archive_ref": format!("room://{room_id}")}),
    )
    .await
    .expect("describe over DirPersistence must succeed");
    let ArchiveWsResponse::DescribeResult(dir_result) = dir_describe else {
        panic!("expected archive.describe.result")
    };

    // Non-hydrating (S3-shaped): same node, same blob hash *marked present*
    // (as a real upload-confirm would), but get_blob is always None.
    let s3_shaped_persistence = non_hydrating_persistence("archive-digest-parity-s3shape");
    s3_shaped_persistence.persist_blob(&blob_hash, &blob_bytes); // marks present; bytes discarded
    let s3_room: Arc<Room> = Room::new(room_id.to_string(), Arc::clone(&s3_shaped_persistence), 64);
    let (accepted, _, errs) =
        import_nodes(&s3_room, vec![make_setblob_node(&signer, "asset", blob_hash)]).await;
    assert_eq!(accepted, 1, "non-hydrating room: node must be accepted: {errs:?}");

    let s3_describe = process_archive_describe(
        &s3_room,
        room_id,
        &serde_json::json!({"archive_ref": format!("room://{room_id}")}),
    )
    .await
    .expect(
        "describe over the non-hydrating backend must succeed (fail-loud is also an \
         acceptable 2.3 outcome per the plan; this harness pins whichever the fix \
         chooses — today it silently succeeds with an empty-set digest, which is \
         finding #7)",
    );
    let ArchiveWsResponse::DescribeResult(s3_result) = s3_describe else {
        panic!("expected archive.describe.result")
    };

    assert_eq!(
        dir_result.payload_digest_set.blobs, s3_result.payload_digest_set.blobs,
        "finding #7: archive describe must not silently compute blobs_digest over an \
         empty set when the backend never hydrates bytes — Dir- and S3-backed exports \
         of the identical room must produce the same blobs digest"
    );
}
