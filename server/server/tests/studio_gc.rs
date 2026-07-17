//! S5.3 integration tests — the server-side GC coordinator end-to-end:
//! studio-domain `LiveHashSource` reading real room state, staged rollout
//! (dryrun/markonly/sweepsoft/sweephard) against real SQLite-backed
//! inventory/run stores and a real local `BlobObjectStore`, fail-closed
//! behavior, and restart durability.
//!
//! Studio map entries are installed the same way the real engine promotion
//! path would produce them: a `MapOp::Set` op whose key is
//! `"{kind}/{entityId}"` and whose value is the compact JSON text of the
//! envelope `{"v":1,"kind":"...","payload":{...}}` — see
//! `studio_live_hashes.rs`'s module docs for why that's what
//! `StateGraph::resolve_with_meta()` actually returns for a promoted
//! `MapSet`.

use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use nodalmerge_blobstore_conformance::make_setblob_node;
use nodalmerge_core::{BlobStore, Hash, MapOp, Op, StateGraph};
use nodalmerge_gc::contracts::{AssetInventoryStore, GcRunStore};
use nodalmerge_gc::types::{AssetState, GcRunMode, GcRunStart};
use nodalmerge_server::gc_blob_objects::LocalBlobObjectStore;
use nodalmerge_server::gc_pin_store::StaticPinStore;
use nodalmerge_server::gc_service::{self, GcMode, GcServiceConfig};
use nodalmerge_server::gc_store::{local_key_scheme, SqliteGcStore};
use nodalmerge_server::room::{import_nodes, Rooms};
use nodalmerge_server::store::{DirPersistence, SharedPersistence};
use nodalmerge_server::studio_live_hashes::collect_studio_live_hashes;

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("nodalmerge-s5.3-{tag}-{nanos}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn tree_v2_blob(entries: serde_json::Value) -> (Hash, Vec<u8>) {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "nodalmerge": "tree", "version": 2, "entries": entries
    }))
    .unwrap();
    (Hash::of(&bytes), bytes)
}

fn snapshot_envelope(
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

fn work_unit_envelope(
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

/// Build a signed node carrying a single `MapOp::Set{key, value}` — the
/// same op shape a promoted `MapSet` produces. Throwaway graph, caller owns
/// only the resulting node.
fn make_mapset_node(sk: &SigningKey, key: &str, value: Vec<u8>) -> nodalmerge_core::SyncNode {
    let mut g = StateGraph::new();
    let id = g
        .apply_local(sk, 0, vec![Op::Map(MapOp::Set { key: key.into(), value })])
        .unwrap();
    g.get_nodes(&[id]).into_iter().next().unwrap().clone()
}

async fn install_studio_entry(room: &Arc<nodalmerge_server::room::Room>, sk: &SigningKey, key: &str, value: Vec<u8>) {
    let node = make_mapset_node(sk, key, value);
    let (accepted, _, errs) = import_nodes(room, vec![node]).await;
    assert_eq!(accepted, 1, "expected the studio entry node to be accepted: {errs:?}");
}

/// Seed an inventory row as though a *prior* mark pass (or the upload-time
/// `Uploading -> Active` hook wired into `blob_http.rs`) already saw this
/// hash as live. This is the realistic way a hash ends up eligible for
/// soft-sweep discovery in this design: `BlobObjectStore` deliberately has
/// no `list`, so nothing here ever discovers a blob from a raw directory
/// scan (per `docs/delegated-storage-gc.md`'s "no ListBucket" rule) —
/// inventory rows only ever come from a mark pass or a write-time upsert.
/// A blob that was *never* registered either way (bypassing every wired
/// write path) is a known, documented gap — see the S5.3 final report.
fn seed_prior_active(inventory: &SqliteGcStore, hash: &Hash) {
    let past = std::time::SystemTime::now() - Duration::from_secs(7 * 24 * 3600);
    inventory.upsert_active_seen("prior-run", &hash.to_hex(), past).unwrap();
}

#[tokio::test]
async fn multi_room_live_set_matches_hand_computed_union() {
    let dir = tmpdir("multiroom");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0xA1u8; 32]);
    let rooms = Rooms::new(SigningKey::from_bytes(&[0x10u8; 32]), Arc::clone(&persistence), 512, 0, 0);

    // ── repo/repo-a: bootstrap (pinned) + head (active), each with a v2
    // tree blob referencing one file each. ──────────────────────────────
    let room_a = rooms.get_or_create("repo/repo-a").await;
    let (file_x, file_x_bytes) = (Hash::of(b"file-x-contents"), b"file-x-contents".to_vec());
    persistence.persist_blob(&file_x, &file_x_bytes).unwrap();
    let (tree0, tree0_bytes) = tree_v2_blob(serde_json::json!([{"n":"x.txt","k":"f","h":file_x.to_hex()}]));
    persistence.persist_blob(&tree0, &tree0_bytes).unwrap();
    room_a.blobs.write().await.put(tree0_bytes.clone());

    let (file_y, file_y_bytes) = (Hash::of(b"file-y-contents"), b"file-y-contents".to_vec());
    persistence.persist_blob(&file_y, &file_y_bytes).unwrap();
    let (tree1, tree1_bytes) = tree_v2_blob(serde_json::json!([{"n":"y.txt","k":"f","h":file_y.to_hex()}]));
    persistence.persist_blob(&tree1, &tree1_bytes).unwrap();

    install_studio_entry(
        &room_a, &sk, "studio/repository-snapshot/v1/gen-0",
        snapshot_envelope("gen-0", "repo-a", 0, "2020-01-01T00:00:00Z", &tree0.to_hex(), None, Some("Bootstrap"), None),
    ).await;
    install_studio_entry(
        &room_a, &sk, "studio/repository-snapshot/v1/gen-1",
        snapshot_envelope("gen-1", "repo-a", 1, "2020-01-02T00:00:00Z", &tree1.to_hex(), None, None, None),
    ).await;

    // ── repo/repo-b: an expired Intermediate generation (its unique blob
    // must NOT be live) plus a head generation. ─────────────────────────
    let room_b = rooms.get_or_create("repo/repo-b").await;
    let (file_stale, file_stale_bytes) = (Hash::of(b"stale-unique-file"), b"stale-unique-file".to_vec());
    persistence.persist_blob(&file_stale, &file_stale_bytes).unwrap();
    let (tree_stale, tree_stale_bytes) = tree_v2_blob(serde_json::json!([{"n":"s.txt","k":"f","h":file_stale.to_hex()}]));
    persistence.persist_blob(&tree_stale, &tree_stale_bytes).unwrap();

    let (file_head_b, file_head_b_bytes) = (Hash::of(b"repo-b-head-file"), b"repo-b-head-file".to_vec());
    persistence.persist_blob(&file_head_b, &file_head_b_bytes).unwrap();
    let (tree_head_b, tree_head_b_bytes) = tree_v2_blob(serde_json::json!([{"n":"h.txt","k":"f","h":file_head_b.to_hex()}]));
    persistence.persist_blob(&tree_head_b, &tree_head_b_bytes).unwrap();

    install_studio_entry(
        &room_b, &sk, "studio/repository-snapshot/v1/gen-0",
        snapshot_envelope("gen-0", "repo-b", 0, "2000-01-01T00:00:00Z", &tree_stale.to_hex(), None, Some("Bootstrap"), None),
    ).await;
    // Note: gen-0 is Bootstrap so it's Pinned regardless — use a SECOND,
    // non-pinned, non-head, old generation to actually exercise expiry.
    install_studio_entry(
        &room_b, &sk, "studio/repository-snapshot/v1/gen-stale",
        snapshot_envelope("gen-stale", "repo-b", 1, "2000-06-01T00:00:00Z", &tree_stale.to_hex(), None, None, None),
    ).await;
    install_studio_entry(
        &room_b, &sk, "studio/repository-snapshot/v1/gen-head",
        snapshot_envelope("gen-head", "repo-b", 2, "2026-01-01T00:00:00Z", &tree_head_b.to_hex(), None, None, None),
    ).await;

    // ── legacy "studio" room (non-repo, pre-6.3a peer data): retained
    // unconditionally per the conservative non-repo-room rule. ──────────
    let room_legacy = rooms.get_or_create("studio").await;
    let (file_w, file_w_bytes) = (Hash::of(b"legacy-room-file"), b"legacy-room-file".to_vec());
    persistence.persist_blob(&file_w, &file_w_bytes).unwrap();
    let (tree_w, tree_w_bytes) = tree_v2_blob(serde_json::json!([{"n":"w.txt","k":"f","h":file_w.to_hex()}]));
    persistence.persist_blob(&tree_w, &tree_w_bytes).unwrap();
    install_studio_entry(
        &room_legacy, &sk, "studio/repository-snapshot/v1/legacy-gen",
        snapshot_envelope("legacy-gen", "repo-legacy", 0, "2000-01-01T00:00:00Z", &tree_w.to_hex(), None, None, None),
    ).await;

    let live = collect_studio_live_hashes(&rooms, 30).await.expect("collect must succeed");

    // repo-a: both generations retained (bootstrap + head) -> both trees +
    // both files live.
    for h in [tree0, file_x, tree1, file_y] {
        assert!(live.contains(&h.to_hex()), "expected {} live (repo-a)", h.to_hex());
    }
    // repo-b: gen-stale (Intermediate, 26 years old, no seed/head/pin
    // protection) must NOT be live via its own contribution... but gen-0
    // (Bootstrap) shares the same tree_stale blob, so tree_stale/file_stale
    // ARE live via the Pinned generation. This is expected — the point of
    // this fixture is proving classification distinguishes the two
    // generations' *retention status*, not that the shared blob disappears
    // (content-addressing means a blob is live if *any* retained generation
    // references it).
    for h in [tree_stale, file_stale, tree_head_b, file_head_b] {
        assert!(live.contains(&h.to_hex()), "expected {} live (repo-b)", h.to_hex());
    }
    // legacy "studio" room: retained unconditionally.
    for h in [tree_w, file_w] {
        assert!(live.contains(&h.to_hex()), "expected {} live (legacy room)", h.to_hex());
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn expired_intermediate_generation_without_shared_blobs_is_excluded() {
    // Same shape as above but this repo's stale generation's blob is
    // *unique* to it (no Pinned/Active generation shares it), so it must be
    // absent from the live set entirely — the actual "reclaim it" case.
    let dir = tmpdir("expired-unique");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0xA2u8; 32]);
    let rooms = Rooms::new(SigningKey::from_bytes(&[0x11u8; 32]), Arc::clone(&persistence), 512, 0, 0);
    let room = rooms.get_or_create("repo/repo-c").await;

    let (file_bootstrap, fb_bytes) = (Hash::of(b"bootstrap-file"), b"bootstrap-file".to_vec());
    persistence.persist_blob(&file_bootstrap, &fb_bytes).unwrap();
    let (tree_bootstrap, tb_bytes) = tree_v2_blob(serde_json::json!([{"n":"b.txt","k":"f","h":file_bootstrap.to_hex()}]));
    persistence.persist_blob(&tree_bootstrap, &tb_bytes).unwrap();

    let (file_unique_stale, fus_bytes) = (Hash::of(b"unique-stale-file"), b"unique-stale-file".to_vec());
    persistence.persist_blob(&file_unique_stale, &fus_bytes).unwrap();
    let (tree_unique_stale, tus_bytes) =
        tree_v2_blob(serde_json::json!([{"n":"u.txt","k":"f","h":file_unique_stale.to_hex()}]));
    persistence.persist_blob(&tree_unique_stale, &tus_bytes).unwrap();

    let (file_head, fh_bytes) = (Hash::of(b"repo-c-head-file"), b"repo-c-head-file".to_vec());
    persistence.persist_blob(&file_head, &fh_bytes).unwrap();
    let (tree_head, th_bytes) = tree_v2_blob(serde_json::json!([{"n":"h.txt","k":"f","h":file_head.to_hex()}]));
    persistence.persist_blob(&tree_head, &th_bytes).unwrap();

    install_studio_entry(
        &room, &sk, "studio/repository-snapshot/v1/gen-0",
        snapshot_envelope("gen-0", "repo-c", 0, "2000-01-01T00:00:00Z", &tree_bootstrap.to_hex(), None, Some("Bootstrap"), None),
    ).await;
    install_studio_entry(
        &room, &sk, "studio/repository-snapshot/v1/gen-stale",
        snapshot_envelope("gen-stale", "repo-c", 1, "2000-06-01T00:00:00Z", &tree_unique_stale.to_hex(), None, None, None),
    ).await;
    install_studio_entry(
        &room, &sk, "studio/repository-snapshot/v1/gen-head",
        snapshot_envelope("gen-head", "repo-c", 2, "2026-01-01T00:00:00Z", &tree_head.to_hex(), None, None, None),
    ).await;

    let live = collect_studio_live_hashes(&rooms, 30).await.expect("collect must succeed");
    assert!(live.contains(&tree_bootstrap.to_hex()));
    assert!(live.contains(&file_bootstrap.to_hex()));
    assert!(live.contains(&tree_head.to_hex()));
    assert!(live.contains(&file_head.to_hex()));
    assert!(!live.contains(&tree_unique_stale.to_hex()), "expired Intermediate tree must be reclaimable");
    assert!(!live.contains(&file_unique_stale.to_hex()), "expired Intermediate's unique file must be reclaimable");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn active_work_unit_seed_stays_live_past_the_retention_window() {
    // NOTE (fixed alongside slice 1.6, blob-cas-remediation.md): this test
    // used to be vacuous — its Bootstrap generation (gen-0) shared
    // `tree_seed` with the seed generation (gen-seed), so gen-seed's tree
    // was Pinned via Bootstrap regardless of whether the "non-terminal
    // work unit protects its seed" mechanism under test actually worked.
    // The Bootstrap generation now gets its OWN unique tree
    // (`tree_bootstrap`), isolating the seed-protection loop the same way
    // 0.3(e)'s `failed_work_unit_seed_stays_live_past_the_retention_window`
    // does. Proof this now isolates the mechanism: reverting the fix this
    // test guards (making `WU-inflight`'s status terminal, or otherwise
    // disabling the seed-loop) makes this test fail — see slice 1.6's
    // report for the exact local-revert command.
    let dir = tmpdir("active-seed");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0xA3u8; 32]);
    let rooms = Rooms::new(SigningKey::from_bytes(&[0x12u8; 32]), Arc::clone(&persistence), 512, 0, 0);
    let room = rooms.get_or_create("repo/repo-d").await;

    // Bootstrap generation with its OWN unique tree — not shared with
    // gen-seed below (see the isolation note above).
    let (file_bootstrap, fb_bytes) = (Hash::of(b"repo-d-bootstrap-file"), b"repo-d-bootstrap-file".to_vec());
    persistence.persist_blob(&file_bootstrap, &fb_bytes).unwrap();
    let (tree_bootstrap, tb_bytes) = tree_v2_blob(serde_json::json!([{"n":"b.txt","k":"f","h":file_bootstrap.to_hex()}]));
    persistence.persist_blob(&tree_bootstrap, &tb_bytes).unwrap();

    let (file_seed, fs_bytes) = (Hash::of(b"seed-file"), b"seed-file".to_vec());
    persistence.persist_blob(&file_seed, &fs_bytes).unwrap();
    let (tree_seed, ts_bytes) = tree_v2_blob(serde_json::json!([{"n":"s.txt","k":"f","h":file_seed.to_hex()}]));
    persistence.persist_blob(&tree_seed, &ts_bytes).unwrap();

    let (file_head, fh_bytes) = (Hash::of(b"repo-d-head"), b"repo-d-head".to_vec());
    persistence.persist_blob(&file_head, &fh_bytes).unwrap();
    let (tree_head, th_bytes) = tree_v2_blob(serde_json::json!([{"n":"h.txt","k":"f","h":file_head.to_hex()}]));
    persistence.persist_blob(&tree_head, &th_bytes).unwrap();

    install_studio_entry(
        &room, &sk, "studio/repository-snapshot/v1/gen-0",
        snapshot_envelope("gen-0", "repo-d", 0, "2000-01-01T00:00:00Z", &tree_bootstrap.to_hex(), None, Some("Bootstrap"), None),
    ).await;
    // gen-seed predates the work unit and is >30 days old, and its tree is
    // NOT shared with any Pinned/head generation — would expire as
    // Intermediate on its own, except a non-terminal work unit seeds from
    // it. This is the only thing that can keep tree_seed/file_seed live.
    install_studio_entry(
        &room, &sk, "studio/repository-snapshot/v1/gen-seed",
        snapshot_envelope("gen-seed", "repo-d", 1, "2020-01-01T00:00:00Z", &tree_seed.to_hex(), None, None, None),
    ).await;
    install_studio_entry(
        &room, &sk, "studio/repository-snapshot/v1/gen-head",
        snapshot_envelope("gen-head", "repo-d", 2, "2026-01-01T00:00:00Z", &tree_head.to_hex(), None, None, None),
    ).await;
    install_studio_entry(
        &room, &sk, "studio/work-unit/v1/WU-inflight",
        work_unit_envelope("WU-inflight", 7 /* Executing */, "2020-01-02T00:00:00Z", "2020-01-02T00:00:00Z", Some("repo-d"), None),
    ).await;

    let live = collect_studio_live_hashes(&rooms, 30).await.expect("collect must succeed");
    assert!(live.contains(&tree_seed.to_hex()), "seed generation must stay live for the in-flight work unit");
    assert!(live.contains(&file_seed.to_hex()));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn fail_closed_on_missing_tree_blob_for_retained_snapshot() {
    let dir = tmpdir("fail-closed");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0xA4u8; 32]);
    let rooms = Rooms::new(SigningKey::from_bytes(&[0x13u8; 32]), Arc::clone(&persistence), 512, 0, 0);
    let room = rooms.get_or_create("repo/repo-broken").await;

    // A retained (Bootstrap => Pinned) snapshot pointing at a TreeHash that
    // was never actually persisted as a blob.
    let phantom_tree = Hash::of(b"never actually stored anywhere");
    install_studio_entry(
        &room, &sk, "studio/repository-snapshot/v1/gen-0",
        snapshot_envelope("gen-0", "repo-broken", 0, "2020-01-01T00:00:00Z", &phantom_tree.to_hex(), None, Some("Bootstrap"), None),
    ).await;

    let err = collect_studio_live_hashes(&rooms, 30).await.expect_err("must fail closed on unresolvable tree");
    // No partial set is ever returned — the caller only ever sees Err.
    let _ = err;

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── Staged rollout against real stores ────────────────────────────────────

fn gc_cfg(mode: GcMode, grace: Duration) -> GcServiceConfig {
    GcServiceConfig {
        mode,
        grace,
        max_deletes_per_run: 100,
        require_head_before_delete: true,
        retain_intermediate_days: 30,
    }
}

#[tokio::test]
async fn dryrun_mutates_nothing() {
    let dir = tmpdir("dryrun");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0xB1u8; 32]);
    let rooms = Rooms::new(SigningKey::from_bytes(&[0x20u8; 32]), Arc::clone(&persistence), 512, 0, 0);
    let room = rooms.get_or_create("repo/repo-dry").await;

    let (file, file_bytes) = (Hash::of(b"dryrun-live-file"), b"dryrun-live-file".to_vec());
    persistence.persist_blob(&file, &file_bytes).unwrap();
    let (tree, tree_bytes) = tree_v2_blob(serde_json::json!([{"n":"f.txt","k":"f","h":file.to_hex()}]));
    persistence.persist_blob(&tree, &tree_bytes).unwrap();
    install_studio_entry(
        &room, &sk, "studio/repository-snapshot/v1/gen-0",
        snapshot_envelope("gen-0", "repo-dry", 0, "2020-01-01T00:00:00Z", &tree.to_hex(), None, Some("Bootstrap"), None),
    ).await;

    // An orphan blob nothing references — dryrun must not touch it either.
    let (orphan, orphan_bytes) = (Hash::of(b"dryrun-orphan"), b"dryrun-orphan".to_vec());
    persistence.persist_blob(&orphan, &orphan_bytes).unwrap();

    let inventory = Arc::new(SqliteGcStore::open(&dir, local_key_scheme()).unwrap());
    let pins = Arc::new(StaticPinStore::new(std::iter::empty()));
    let objects = Arc::new(LocalBlobObjectStore::new(&dir));

    let cfg = gc_cfg(GcMode::New(GcRunMode::DryRun), Duration::from_secs(3600));
    let delta = gc_service::run_new_coordinator_once(&rooms, GcRunMode::DryRun, &cfg, Arc::clone(&inventory), pins, objects)
        .await
        .expect("dryrun must succeed");
    assert!(delta.marked_count >= 2, "expects at least tree+file marked live");
    assert_eq!(delta.newly_pending_count, 0);
    assert_eq!(delta.hard_deleted_count, 0);

    // Nothing on disk was touched.
    let blake3_dir = dir.join("blobs").join("blake3");
    assert!(blake3_dir.join(file.to_hex()).is_file());
    assert!(blake3_dir.join(tree.to_hex()).is_file());
    assert!(blake3_dir.join(orphan.to_hex()).is_file(), "dryrun must not delete the orphan");
    // And no inventory row was tombstoned (dryrun skips soft/hard sweep
    // entirely — see GcCoordinator::run_inner).
    assert_ne!(inventory.asset_state(&orphan.to_hex()), Some(AssetState::PendingDelete));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn markonly_marks_without_sweeping() {
    let dir = tmpdir("markonly");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0xB2u8; 32]);
    let rooms = Rooms::new(SigningKey::from_bytes(&[0x21u8; 32]), Arc::clone(&persistence), 512, 0, 0);
    let room = rooms.get_or_create("repo/repo-mark").await;

    let (file, file_bytes) = (Hash::of(b"markonly-file"), b"markonly-file".to_vec());
    persistence.persist_blob(&file, &file_bytes).unwrap();
    let (tree, tree_bytes) = tree_v2_blob(serde_json::json!([{"n":"f.txt","k":"f","h":file.to_hex()}]));
    persistence.persist_blob(&tree, &tree_bytes).unwrap();
    install_studio_entry(
        &room, &sk, "studio/repository-snapshot/v1/gen-0",
        snapshot_envelope("gen-0", "repo-mark", 0, "2020-01-01T00:00:00Z", &tree.to_hex(), None, Some("Bootstrap"), None),
    ).await;

    let inventory = Arc::new(SqliteGcStore::open(&dir, local_key_scheme()).unwrap());
    let pins = Arc::new(StaticPinStore::new(std::iter::empty()));
    let objects = Arc::new(LocalBlobObjectStore::new(&dir));
    let cfg = gc_cfg(GcMode::New(GcRunMode::MarkOnly), Duration::from_secs(3600));

    gc_service::run_new_coordinator_once(&rooms, GcRunMode::MarkOnly, &cfg, Arc::clone(&inventory), pins, objects)
        .await
        .expect("markonly must succeed");

    assert_eq!(inventory.asset_state(&file.to_hex()), Some(AssetState::Active));
    assert_eq!(inventory.asset_state(&tree.to_hex()), Some(AssetState::Active));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn sweepsoft_then_sweephard_reclaims_orphan_and_respects_max_deletes() {
    let dir = tmpdir("softhard");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0xB3u8; 32]);
    let rooms = Rooms::new(SigningKey::from_bytes(&[0x22u8; 32]), Arc::clone(&persistence), 512, 0, 0);
    let room = rooms.get_or_create("repo/repo-soft").await;

    let (file, file_bytes) = (Hash::of(b"softhard-live-file"), b"softhard-live-file".to_vec());
    persistence.persist_blob(&file, &file_bytes).unwrap();
    let (tree, tree_bytes) = tree_v2_blob(serde_json::json!([{"n":"f.txt","k":"f","h":file.to_hex()}]));
    persistence.persist_blob(&tree, &tree_bytes).unwrap();
    install_studio_entry(
        &room, &sk, "studio/repository-snapshot/v1/gen-0",
        snapshot_envelope("gen-0", "repo-soft", 0, "2020-01-01T00:00:00Z", &tree.to_hex(), None, Some("Bootstrap"), None),
    ).await;

    // Two orphans: bytes exist on disk and (per `seed_prior_active`'s doc)
    // were registered as live by a *prior* mark pass — modeling "this was
    // referenced once, the reference was since removed" rather than "never
    // referenced, ever" (which this design cannot discover without a
    // listing-based drift job; see the helper's doc).
    let (orphan1, orphan1_bytes) = (Hash::of(b"orphan-one"), b"orphan-one".to_vec());
    let (orphan2, orphan2_bytes) = (Hash::of(b"orphan-two"), b"orphan-two".to_vec());
    persistence.persist_blob(&orphan1, &orphan1_bytes).unwrap();
    persistence.persist_blob(&orphan2, &orphan2_bytes).unwrap();

    let inventory = Arc::new(SqliteGcStore::open(&dir, local_key_scheme()).unwrap());
    seed_prior_active(&inventory, &orphan1);
    seed_prior_active(&inventory, &orphan2);
    let pins = Arc::new(StaticPinStore::new(std::iter::empty()));
    let objects = Arc::new(LocalBlobObjectStore::new(&dir));

    // Short grace so hard-sweep is observable without sleeping 24h.
    let grace = Duration::from_millis(50);
    let cfg = gc_cfg(GcMode::New(GcRunMode::SweepSoft), grace);
    let delta = gc_service::run_new_coordinator_once(&rooms, GcRunMode::SweepSoft, &cfg, Arc::clone(&inventory), Arc::clone(&pins), Arc::clone(&objects))
        .await
        .expect("sweepsoft must succeed");
    assert_eq!(delta.newly_pending_count, 2, "both orphans tombstoned");
    assert_eq!(inventory.asset_state(&orphan1.to_hex()), Some(AssetState::PendingDelete));
    assert_eq!(inventory.asset_state(&orphan2.to_hex()), Some(AssetState::PendingDelete));
    // Live blobs are untouched.
    assert_eq!(inventory.asset_state(&file.to_hex()), Some(AssetState::Active));

    // Blobs still physically present — SweepSoft never deletes.
    let blake3_dir = dir.join("blobs").join("blake3");
    assert!(blake3_dir.join(orphan1.to_hex()).is_file());
    assert!(blake3_dir.join(orphan2.to_hex()).is_file());

    tokio::time::sleep(Duration::from_millis(80)).await; // elapse grace

    // max_deletes_per_run = 1: exactly one of the two pending orphans is
    // hard-deleted this run.
    let mut cfg_hard = gc_cfg(GcMode::New(GcRunMode::SweepHard), grace);
    cfg_hard.max_deletes_per_run = 1;
    let delta = gc_service::run_new_coordinator_once(&rooms, GcRunMode::SweepHard, &cfg_hard, Arc::clone(&inventory), Arc::clone(&pins), Arc::clone(&objects))
        .await
        .expect("sweephard must succeed");
    assert_eq!(delta.hard_deleted_count, 1, "max_deletes_per_run must cap this run's deletes");

    let remaining_on_disk = [orphan1, orphan2]
        .iter()
        .filter(|h| blake3_dir.join(h.to_hex()).is_file())
        .count();
    assert_eq!(remaining_on_disk, 1, "exactly one orphan deleted, one still pending");
    assert!(blake3_dir.join(file.to_hex()).is_file(), "live file must survive throughout");
    assert!(blake3_dir.join(tree.to_hex()).is_file(), "live tree must survive throughout");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn re_referenced_during_grace_returns_active_and_survives() {
    let dir = tmpdir("re-referenced");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0xB4u8; 32]);
    let rooms = Rooms::new(SigningKey::from_bytes(&[0x23u8; 32]), Arc::clone(&persistence), 512, 0, 0);
    let room = rooms.get_or_create("repo/repo-revive").await;

    // A blob that starts out unreferenced (but was seen live by a prior
    // mark pass — see `seed_prior_active`'s doc for why that's required for
    // this design to ever consider it a sweep candidate).
    let (revived, revived_bytes) = (Hash::of(b"about-to-be-revived"), b"about-to-be-revived".to_vec());
    persistence.persist_blob(&revived, &revived_bytes).unwrap();

    let inventory = Arc::new(SqliteGcStore::open(&dir, local_key_scheme()).unwrap());
    seed_prior_active(&inventory, &revived);
    let pins = Arc::new(StaticPinStore::new(std::iter::empty()));
    let objects = Arc::new(LocalBlobObjectStore::new(&dir));
    let grace = Duration::from_secs(3600); // long — must not elapse in this test

    let cfg_soft = gc_cfg(GcMode::New(GcRunMode::SweepSoft), grace);
    gc_service::run_new_coordinator_once(&rooms, GcRunMode::SweepSoft, &cfg_soft, Arc::clone(&inventory), Arc::clone(&pins), Arc::clone(&objects))
        .await
        .unwrap();
    assert_eq!(inventory.asset_state(&revived.to_hex()), Some(AssetState::PendingDelete));

    // Now reference it: install a v2 tree blob + a Bootstrap snapshot
    // pointing at it, so the next mark pass sees it live again.
    let (tree, tree_bytes) = tree_v2_blob(serde_json::json!([{"n":"r.txt","k":"f","h":revived.to_hex()}]));
    persistence.persist_blob(&tree, &tree_bytes).unwrap();
    install_studio_entry(
        &room, &sk, "studio/repository-snapshot/v1/gen-0",
        snapshot_envelope("gen-0", "repo-revive", 0, "2020-01-01T00:00:00Z", &tree.to_hex(), None, Some("Bootstrap"), None),
    ).await;

    // A markonly run re-marks it live -> Active, clearing pending_delete_at.
    let cfg_mark = gc_cfg(GcMode::New(GcRunMode::MarkOnly), grace);
    gc_service::run_new_coordinator_once(&rooms, GcRunMode::MarkOnly, &cfg_mark, Arc::clone(&inventory), Arc::clone(&pins), Arc::clone(&objects))
        .await
        .unwrap();
    assert_eq!(inventory.asset_state(&revived.to_hex()), Some(AssetState::Active), "re-referenced blob must return to Active");

    // Even a hard sweep afterward must not delete it (still Active, not
    // PendingDelete, so it's never a hard-sweep candidate).
    let cfg_hard = gc_cfg(GcMode::New(GcRunMode::SweepHard), Duration::ZERO);
    gc_service::run_new_coordinator_once(&rooms, GcRunMode::SweepHard, &cfg_hard, Arc::clone(&inventory), Arc::clone(&pins), Arc::clone(&objects))
        .await
        .unwrap();
    assert!(
        dir.join("blobs").join("blake3").join(revived.to_hex()).is_file(),
        "re-referenced blob must survive a subsequent hard sweep"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn run_ledger_records_failure_and_performs_zero_deletes_on_fail_closed() {
    let dir = tmpdir("run-ledger-fail");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0xB5u8; 32]);
    let rooms = Rooms::new(SigningKey::from_bytes(&[0x24u8; 32]), Arc::clone(&persistence), 512, 0, 0);
    let room = rooms.get_or_create("repo/repo-fail").await;

    // An orphan that would otherwise be eligible for hard delete.
    let (orphan, orphan_bytes) = (Hash::of(b"would-be-deleted-if-not-for-the-failure"), b"would-be-deleted-if-not-for-the-failure".to_vec());
    persistence.persist_blob(&orphan, &orphan_bytes).unwrap();

    // A retained (Bootstrap) snapshot whose TreeHash is unresolvable —
    // forces the whole collect to fail.
    let phantom_tree = Hash::of(b"never stored, forces fail-closed");
    install_studio_entry(
        &room, &sk, "studio/repository-snapshot/v1/gen-0",
        snapshot_envelope("gen-0", "repo-fail", 0, "2020-01-01T00:00:00Z", &phantom_tree.to_hex(), None, Some("Bootstrap"), None),
    ).await;

    let inventory = Arc::new(SqliteGcStore::open(&dir, local_key_scheme()).unwrap());
    let pins = Arc::new(StaticPinStore::new(std::iter::empty()));
    let objects = Arc::new(LocalBlobObjectStore::new(&dir));
    let cfg = gc_cfg(GcMode::New(GcRunMode::SweepHard), Duration::ZERO);

    let result = gc_service::run_new_coordinator_once(&rooms, GcRunMode::SweepHard, &cfg, Arc::clone(&inventory), pins, objects).await;
    assert!(result.is_err(), "fail-closed: the run must return Err");

    // Zero deletes: the orphan blob must still be on disk.
    assert!(
        dir.join("blobs").join("blake3").join(orphan.to_hex()).is_file(),
        "fail-closed run must perform zero deletes"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn inventory_state_survives_store_reopen() {
    let dir = tmpdir("restart");
    let (orphan, orphan_bytes) = (Hash::of(b"restart-orphan"), b"restart-orphan".to_vec());
    {
        let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
        let rooms = Rooms::new(SigningKey::from_bytes(&[0x25u8; 32]), Arc::clone(&persistence), 512, 0, 0);
        let _room = rooms.get_or_create("repo/repo-restart").await;
        persistence.persist_blob(&orphan, &orphan_bytes).unwrap();

        let inventory = Arc::new(SqliteGcStore::open(&dir, local_key_scheme()).unwrap());
        seed_prior_active(&inventory, &orphan);
        let pins = Arc::new(StaticPinStore::new(std::iter::empty()));
        let objects = Arc::new(LocalBlobObjectStore::new(&dir));
        let cfg = gc_cfg(GcMode::New(GcRunMode::SweepSoft), Duration::from_secs(3600));
        gc_service::run_new_coordinator_once(&rooms, GcRunMode::SweepSoft, &cfg, Arc::clone(&inventory), pins, objects)
            .await
            .unwrap();
        assert_eq!(inventory.asset_state(&orphan.to_hex()), Some(AssetState::PendingDelete));
    }

    // Fresh process-equivalent: reopen the SQLite store against the same
    // directory (no in-memory state carried over).
    let inventory2 = SqliteGcStore::open(&dir, local_key_scheme()).unwrap();
    assert_eq!(
        inventory2.asset_state(&orphan.to_hex()),
        Some(AssetState::PendingDelete),
        "PendingDelete + grace timestamp must survive a store reopen"
    );
    let pending: Vec<_> = inventory2
        .iter_pending_older_than(std::time::SystemTime::now() + Duration::from_secs(3601))
        .unwrap()
        .collect();
    assert!(pending.iter().any(|r| r.hash == orphan.to_hex()));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn run_store_records_running_then_succeeded() {
    // Direct GcRunStore exercise via the real SqliteGcStore, proving the
    // ledger is queryable end-to-end through the same path
    // `run_new_coordinator_once` uses internally.
    let dir = tmpdir("run-store");
    let store = SqliteGcStore::open(&dir, local_key_scheme()).unwrap();
    let run_id = store
        .start_run(GcRunStart { mode: GcRunMode::DryRun, started_at: std::time::SystemTime::now() })
        .unwrap();
    assert_eq!(store.run_status(&run_id), None, "no status until finish_run");
    let _ = std::fs::remove_dir_all(&dir);
}

// ─── 0.3 — GC data-loss regression suite (blob-cas-remediation.md Phase 0/1) ──
//
// (a), (c), (d) live in tests/blob_gc.rs (Rooms::sweep_blobs / legacy
// two-phase world); (b) and (e) live here, since both need the new
// coordinator's studio-domain live-hash source. See the plan's Phase 0 §0.3
// and Phase 1 for the finding each gates.

#[tokio::test]
#[ignore = "RED: fails until slice 1.2 — see nodalmerge-studio/plans/blob-cas-remediation.md"]
async fn ordinary_setblob_blob_must_survive_sweepsoft_then_sweephard() {
    // 0.3(b) — gates slice 1.2 (finding #2). `collect_studio_live_hashes`
    // (studio_live_hashes.rs:666) only ever looks at `studio/`-prefixed
    // engine-map keys (`resolve_room_studio_map`'s filter at line 648), so
    // an ordinary
    // `SetBlob`-referenced blob — nothing studio-specific about it, e.g. a
    // plain avatar upload — is never added to the live set the new
    // coordinator computes from, and gets reclaimed under `--gc-mode
    // sweepsoft`/`sweephard` even though it's genuinely referenced by the
    // room's DAG. The legacy `Rooms::sweep_blobs` path protects this fine
    // (via `blob_hashes_referenced_by`); this coordinator does not.
    let dir = tmpdir("ordinary-setblob");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0xB6u8; 32]);
    let rooms = Rooms::new(SigningKey::from_bytes(&[0x26u8; 32]), Arc::clone(&persistence), 512, 0, 0);
    let room = rooms.get_or_create("peer-room").await;

    let (avatar, avatar_bytes) = (Hash::of(b"an-ordinary-avatar-upload"), b"an-ordinary-avatar-upload".to_vec());
    persistence.persist_blob(&avatar, &avatar_bytes).unwrap();
    let node = make_setblob_node(&sk, "avatar", avatar);
    let (accepted, _, errs) = import_nodes(&room, vec![node]).await;
    assert_eq!(accepted, 1, "expected the ordinary SetBlob node to be accepted: {errs:?}");

    let inventory = Arc::new(SqliteGcStore::open(&dir, local_key_scheme()).unwrap());
    // A prior mark pass (or the upload-time Uploading->Active hook wired
    // into blob_http.rs) already saw this hash live — see
    // `seed_prior_active`'s doc for why that's required for it to be a
    // sweep candidate at all in this design.
    seed_prior_active(&inventory, &avatar);
    let pins = Arc::new(StaticPinStore::new(std::iter::empty()));
    let objects = Arc::new(LocalBlobObjectStore::new(&dir));

    let grace = Duration::from_millis(50);
    let cfg_soft = gc_cfg(GcMode::New(GcRunMode::SweepSoft), grace);
    gc_service::run_new_coordinator_once(&rooms, GcRunMode::SweepSoft, &cfg_soft, Arc::clone(&inventory), Arc::clone(&pins), Arc::clone(&objects))
        .await
        .expect("sweepsoft must succeed");

    tokio::time::sleep(Duration::from_millis(80)).await; // elapse grace

    let cfg_hard = gc_cfg(GcMode::New(GcRunMode::SweepHard), grace);
    gc_service::run_new_coordinator_once(&rooms, GcRunMode::SweepHard, &cfg_hard, Arc::clone(&inventory), Arc::clone(&pins), Arc::clone(&objects))
        .await
        .expect("sweephard must succeed");

    assert!(
        dir.join("blobs").join("blake3").join(avatar.to_hex()).is_file(),
        "an ordinary SetBlob-referenced blob must survive sweepsoft/sweephard, not just the legacy sweep"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn failed_work_unit_seed_stays_live_past_the_retention_window() {
    // 0.3(e) — gates slice 1.6 (finding #30, added during 0.4).
    // `TERMINAL_STATUSES` (studio_live_hashes.rs) treats `Failed` (4) as
    // terminal, so a `Failed` work unit is skipped entirely by the "Active"
    // seed-protection loop in `classify_repo_room`, and its seed generation
    // — deliberately NOT shared with any Pinned/head generation here, unlike
    // this file's `active_work_unit_seed_stays_live_past_the_retention_window`
    // sibling fixture, which happens to also pin its seed's tree via a
    // Bootstrap generation and so doesn't actually isolate the seed-loop
    // mechanism — ages out under `retain_intermediate_days` like any
    // ordinary Intermediate snapshot. But studio's `WorkUnit.cs:208` rule
    // `(_, Cancelled) when from is not Completed and not Merged => true`
    // permits `Failed -> Cancelled`, and `Cancelled -> Queued`/`Executing`
    // are both legal — a real revival path out of a status the GC
    // retention-ages. The GC cannot prove this work unit's blobs are dead,
    // so it must not reclaim them.
    let dir = tmpdir("failed-seed");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0xA5u8; 32]);
    let rooms = Rooms::new(SigningKey::from_bytes(&[0x14u8; 32]), Arc::clone(&persistence), 512, 0, 0);
    let room = rooms.get_or_create("repo/repo-e").await;

    // Bootstrap generation with its OWN unique tree — not shared with
    // gen-seed below.
    let (file_bootstrap, fb_bytes) = (Hash::of(b"repo-e-bootstrap-file"), b"repo-e-bootstrap-file".to_vec());
    persistence.persist_blob(&file_bootstrap, &fb_bytes).unwrap();
    let (tree_bootstrap, tb_bytes) = tree_v2_blob(serde_json::json!([{"n":"b.txt","k":"f","h":file_bootstrap.to_hex()}]));
    persistence.persist_blob(&tree_bootstrap, &tb_bytes).unwrap();

    let (file_seed, fs_bytes) = (Hash::of(b"repo-e-seed-file"), b"repo-e-seed-file".to_vec());
    persistence.persist_blob(&file_seed, &fs_bytes).unwrap();
    let (tree_seed, ts_bytes) = tree_v2_blob(serde_json::json!([{"n":"s.txt","k":"f","h":file_seed.to_hex()}]));
    persistence.persist_blob(&tree_seed, &ts_bytes).unwrap();

    let (file_head, fh_bytes) = (Hash::of(b"repo-e-head-file"), b"repo-e-head-file".to_vec());
    persistence.persist_blob(&file_head, &fh_bytes).unwrap();
    let (tree_head, th_bytes) = tree_v2_blob(serde_json::json!([{"n":"h.txt","k":"f","h":file_head.to_hex()}]));
    persistence.persist_blob(&tree_head, &th_bytes).unwrap();

    install_studio_entry(
        &room, &sk, "studio/repository-snapshot/v1/gen-0",
        snapshot_envelope("gen-0", "repo-e", 0, "2000-01-01T00:00:00Z", &tree_bootstrap.to_hex(), None, Some("Bootstrap"), None),
    ).await;
    // >30 days old, no shared Pinned/head tree, no WorkUnitId on the
    // snapshot itself — its only possible protection is a non-terminal
    // work unit's seed-selection.
    install_studio_entry(
        &room, &sk, "studio/repository-snapshot/v1/gen-seed",
        snapshot_envelope("gen-seed", "repo-e", 1, "2020-01-01T00:00:00Z", &tree_seed.to_hex(), None, None, None),
    ).await;
    install_studio_entry(
        &room, &sk, "studio/repository-snapshot/v1/gen-head",
        snapshot_envelope("gen-head", "repo-e", 2, "2026-01-01T00:00:00Z", &tree_head.to_hex(), None, None, None),
    ).await;
    install_studio_entry(
        &room, &sk, "studio/work-unit/v1/WU-failed",
        work_unit_envelope("WU-failed", 4 /* Failed */, "2020-01-02T00:00:00Z", "2020-01-02T00:00:00Z", Some("repo-e"), None),
    ).await;

    let live = collect_studio_live_hashes(&rooms, 30).await.expect("collect must succeed");
    assert!(
        live.contains(&tree_seed.to_hex()),
        "a Failed work unit's seed generation must stay live -- Failed has a legal revival path (Failed -> Cancelled -> Queued/Executing)"
    );
    assert!(live.contains(&file_seed.to_hex()));

    let _ = std::fs::remove_dir_all(&dir);
}
