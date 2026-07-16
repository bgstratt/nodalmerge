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

#[test]
fn tree_walk_fails_closed_when_backend_never_hydrates_bytes() {
    // Not gated: `walk_tree`'s own contract is fail-closed by design
    // (tree_walk.rs module docs), and slice 2.2 may legitimately choose to
    // resolve trees through a *different*, hydrating-capable path before
    // ever reaching this function rather than changing `walk_tree` itself.
    // This test only pins the mechanism finding #10 describes; the RED
    // assertion that actually gates 2.2 is
    // `studio_gc_live_set_over_cas_tree_snapshot_does_not_fail_closed_on_non_hydrating_backend`
    // below, which exercises the real GC entry point and requires the
    // *overall* collection to stop failing closed, without presuming how.
    let backend = NonHydratingBackend::default();
    let (tree_hash, tree_bytes) = tree_v2_blob(serde_json::json!([]));
    backend.persist_blob(&tree_hash, &tree_bytes); // marks present; bytes still unreadable
    assert!(backend.has_blob(&tree_hash), "sanity: the backend agrees the tree blob exists");

    let err = walk_tree(&backend, &tree_hash)
        .expect_err("today, get_blob=None always fails the walk even though has_blob is true");
    assert!(matches!(err, TreeWalkError::MissingBlob(_)));
}

// ─── GC live-set (finding #10, gates slice 2.2) ────────────────────────────

#[tokio::test]
#[ignore = "RED: fails until slice 2.2 — see nodalmerge-studio/plans/blob-cas-remediation.md"]
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
