//! blob-cas-remediation.md slice 2.2 (finding #10) — the **fail-loud** half,
//! end-to-end through the real studio GC entry point.
//!
//! Direct mode hydrates tree objects with a real bucket `GET`, gated against a
//! real bucket in `minio_round_trip.rs`. **Delegate mode genuinely cannot**:
//! it holds no bucket credentials (`store.s3` is `None`) and delegate presign
//! protocol v1 has only `get`/`put` URL-minting ops — no bytes op. The plan's
//! requirement for that case is a *distinct, actionable* error "rather than a
//! generic per-run Failure, so 'GC never runs on S3' is impossible to miss".
//!
//! `s3-blobs`' unit tests already pin the error at the `hydrate_blob` and
//! `walk_tree` seams. This file pins the property one level up, where it
//! actually matters — at `collect_studio_live_hashes`, the GC coordinator's
//! entry point — because that is the layer the plan's claim is about, and the
//! layer where "distinct" could silently be lost by an error-mapping that
//! flattens everything into one string.
//!
//! No Docker: Delegate mode never opens a connection, so there is nothing to
//! containerize. That is exactly why this is a separate file from
//! `minio_round_trip.rs` (whose workflow sets `NODALMERGE_REQUIRE_DOCKER=1`).

use std::sync::Arc;

use ed25519_dalek::SigningKey;
use nodalmerge_blobstore_conformance::{make_mapset_node, snapshot_envelope, tmp_dir, tree_v2_blob};
use nodalmerge_core::Hash;
use nodalmerge_s3_blobs::{S3Auth, S3BlobStore, S3BlobStoreConfig};
use nodalmerge_server::room::{import_nodes, Rooms};
use nodalmerge_server::store::{BlobPersistence, Composite, DirPersistence, SharedPersistence};
use nodalmerge_server::studio_live_hashes::collect_studio_live_hashes;

/// The production shape of a Delegate-mode deployment: real nodes on disk,
/// blobs in the app's own bucket that this server has no credentials for.
fn delegate_persistence(tag: &str) -> SharedPersistence {
    let cfg = S3BlobStoreConfig {
        bucket: "app-owned-bucket".into(),
        auth: S3Auth::delegate("https://app.example.com/presign", None),
        ..Default::default()
    };
    let s3 = S3BlobStore::new(cfg).expect("build Delegate-mode S3BlobStore");
    assert!(
        !s3.get_blob_hydrates(),
        "sanity: S3BlobStore must declare its get_blob non-hydrating"
    );
    Arc::new(Composite::new(
        DirPersistence::open(tmp_dir(tag)).expect("dir persistence should open"),
        s3,
    ))
}

#[tokio::test]
async fn delegate_mode_studio_gc_fails_with_a_distinct_actionable_error_not_a_generic_failure() {
    let persistence = delegate_persistence("delegate-gc");
    let sk = SigningKey::from_bytes(&[0xD1u8; 32]);
    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0xD2u8; 32]),
        Arc::clone(&persistence),
        512,
        0,
        0,
    );
    let room = rooms.get_or_create("repo/repo-delegate").await;

    let file_hash = Hash::of(b"a repo file living in the app's own bucket");
    let (tree_hash, _tree_bytes) =
        tree_v2_blob(serde_json::json!([{"n": "a.txt", "k": "f", "h": file_hash.to_hex()}]));
    // Deliberately NOT persisted: in Delegate mode the bytes reached the
    // bucket via an app-minted presigned PUT that this server never saw. The
    // object exists; this server just cannot read it. That is the whole point.

    let node = make_mapset_node(
        &sk,
        "studio/repository-snapshot/v1/gen-0",
        snapshot_envelope(
            "gen-0",
            "repo-delegate",
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

    let err = collect_studio_live_hashes(&rooms, 30)
        .await
        .expect_err("Delegate mode cannot resolve the tree, so GC must fail — loudly");
    let msg = err.to_string();

    // 1. NOT a lost blob. This is the assertion that matters: before 2.2 this
    //    read "missing tree/blob object <hex>", which sends an operator
    //    hunting for an object that is sitting intact in the app's bucket.
    assert!(
        !msg.contains("missing tree/blob object"),
        "finding #10: Delegate mode must not report an unreadable-but-present tree object \
         as a missing one — that is indistinguishable from real data loss. Got: {msg}"
    );

    // 2. Distinct at the run level, not merely inside walk_tree: the GC
    //    coordinator's own error must say this is a configuration problem.
    assert!(
        msg.contains("DEPLOYMENT CONFIGURATION"),
        "the run-level GC error must name this as a config problem, not a generic \
         backend failure: {msg}"
    );

    // 3. Actionable: an operator reading only this line must learn which
    //    backend/mode is at fault and what to change.
    let lower = msg.to_lowercase();
    for needle in ["delegate", "direct-mode", "hydrate", "studio gc cannot run"] {
        assert!(
            lower.contains(needle),
            "the error must be actionable and mention {needle:?}; got: {msg}"
        );
    }
}

/// The companion property, and the reason the test above cannot pass by
/// accident: Delegate mode must fail loud **only** because it cannot hydrate,
/// not because it fails at everything. A snapshot carrying a legacy inline
/// `TreeEntries` map needs no tree resolution at all, so GC must succeed even
/// in Delegate mode — proving the error above is specific to the resolution
/// path rather than a Delegate-mode server simply erroring on every input.
#[tokio::test]
async fn delegate_mode_studio_gc_still_succeeds_for_snapshots_needing_no_tree_resolution() {
    let persistence = delegate_persistence("delegate-gc-inline");
    let sk = SigningKey::from_bytes(&[0xD3u8; 32]);
    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0xD4u8; 32]),
        Arc::clone(&persistence),
        512,
        0,
        0,
    );
    let room = rooms.get_or_create("repo/repo-delegate-inline").await;

    let file_hash = Hash::of(b"a file named inline, no tree blob to resolve");
    let node = make_mapset_node(
        &sk,
        "studio/repository-snapshot/v1/gen-0",
        snapshot_envelope(
            "gen-0",
            "repo-delegate-inline",
            0,
            "2026-01-01T00:00:00Z",
            &Hash::of(b"unused tree hash").to_hex(),
            Some(serde_json::json!({ "a.txt": file_hash.to_hex() })),
            Some("Bootstrap"),
            None,
        ),
    );
    let (accepted, _, errs) = import_nodes(&room, vec![node]).await;
    assert_eq!(accepted, 1, "expected the snapshot entry node to be accepted: {errs:?}");

    let live = collect_studio_live_hashes(&rooms, 30)
        .await
        .expect("an inline-entries snapshot needs no tree resolution, so Delegate GC works");
    assert!(
        live.contains(&file_hash.to_hex()),
        "the inline-named file blob must be live: {live:?}"
    );
}
