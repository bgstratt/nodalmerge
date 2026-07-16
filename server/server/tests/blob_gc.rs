//! G4 integration test — two-phase blob GC sweep.
//!
//! Scenario: a room holds two durable blobs — one referenced by a
//! `SetBlob` op in the DAG (live), one that was uploaded but never
//! referenced (orphan). The sweeper should:
//!   - leave the live blob untouched,
//!   - tombstone the orphan on the first sweep,
//!   - delete the orphan on the second sweep once the grace window has
//!     elapsed.
//!
//! Blobs are a single global CAS pool, not room-scoped (see
//! docs/BLOB_STORAGE_LAYOUT.md) — paths below have no room segment.
//!
//! We drive `Rooms::sweep_blobs` directly instead of spawning the task
//! so the test is synchronous, deterministic, and doesn't race `tokio::
//! time::interval`. Grace is set to 50 ms so the two-phase behavior is
//! observable without sleeping for the production 24 h default.

use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use nodalmerge_blobstore_conformance::NonHydratingBackend;
use nodalmerge_core::{BlobStore, Hash, MapOp, Op, StateGraph};
use nodalmerge_server::room::{import_nodes, Rooms};
use nodalmerge_server::store::{Composite, DirPersistence, NodePersistence, SharedPersistence};

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("nodalmerge-g4-{tag}-{nanos}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Build a signed node that carries a single `SetBlob` op pointing at
/// `blob_hash`. Uses a throwaway graph so the caller owns nothing but
/// the node.
fn make_setblob_node(sk: &SigningKey, key: &str, blob_hash: Hash) -> nodalmerge_core::SyncNode {
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

#[tokio::test]
async fn blob_gc_two_phase_deletes_orphans_only() {
    let dir = tmpdir("two-phase");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0xAAu8; 32]);
    let room_id = "gc-room".to_string();

    // Rooms with rate limits disabled; default broadcast capacity.
    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0x01u8; 32]),
        Arc::clone(&persistence),
        512,
        0,
        0,
    );
    let room = rooms.get_or_create(&room_id).await;

    // --- persist two blobs on disk ----------------------------------------
    let live_bytes = b"referenced-by-setblob".to_vec();
    let live_hash = Hash::of(&live_bytes);
    let orphan_bytes = b"never-referenced".to_vec();
    let orphan_hash = Hash::of(&orphan_bytes);
    persistence.persist_blob(&live_hash, &live_bytes);
    persistence.persist_blob(&orphan_hash, &orphan_bytes);
    room.blobs.write().await.put(live_bytes.clone());
    room.blobs.write().await.put(orphan_bytes.clone());

    // --- install a SetBlob node that references ONLY the live hash --------
    let node = make_setblob_node(&sk, "avatar", live_hash);
    let (accepted, _, errs) = import_nodes(&room, vec![node]).await;
    assert_eq!(accepted, 1);
    assert!(errs.is_empty());

    // Short grace so we can observe the two phases in-test.
    let grace = Duration::from_millis(50);

    let blake3_dir = dir.join("blobs").join("blake3");
    let tombs_dir = dir.join("blobs").join(".tombstones").join("blake3");

    // --- Phase 1: tombstones the orphan, deletes nothing. -----------------
    let deleted = rooms.sweep_blobs(grace).await;
    assert_eq!(deleted, 0, "first sweep must only tombstone, not delete");
    let live_path = blake3_dir.join(live_hash.to_hex());
    let orphan_path = blake3_dir.join(orphan_hash.to_hex());
    let orphan_tomb = tombs_dir.join(orphan_hash.to_hex());
    assert!(live_path.exists(), "live blob must survive phase 1");
    assert!(
        orphan_path.exists(),
        "orphan still on disk before grace elapses"
    );
    assert!(orphan_tomb.exists(), "orphan must be tombstoned in phase 1");

    // Wait for grace to elapse.
    tokio::time::sleep(Duration::from_millis(80)).await;

    // --- Phase 2: orphan is now older than grace, must be deleted. --------
    let deleted = rooms.sweep_blobs(grace).await;
    assert_eq!(deleted, 1, "second sweep must delete exactly the orphan");
    assert!(live_path.exists(), "live blob must still survive phase 2");
    assert!(!orphan_path.exists(), "orphan must be gone after phase 2");
    assert!(
        !orphan_tomb.exists(),
        "tombstone is cleaned up after delete"
    );

    // --- Phase 3: idempotent — another sweep changes nothing. -------------
    let deleted = rooms.sweep_blobs(grace).await;
    assert_eq!(deleted, 0, "third sweep is a no-op");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn blob_gc_clears_tombstone_when_blob_becomes_live_again() {
    // A peer that went offline briefly and re-uploaded the same blob should
    // not lose it. We simulate that by: (a) tombstoning a blob via one
    // sweep, (b) installing a SetBlob node that references it, (c)
    // sweeping again with grace=0 — the tombstone must be cleared rather
    // than the blob deleted.
    let dir = tmpdir("rebirth");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0xBBu8; 32]);
    let room_id = "rebirth-room".to_string();

    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0x02u8; 32]),
        Arc::clone(&persistence),
        512,
        0,
        0,
    );
    let room = rooms.get_or_create(&room_id).await;

    let bytes = b"refound".to_vec();
    let hash = Hash::of(&bytes);
    persistence.persist_blob(&hash, &bytes);
    room.blobs.write().await.put(bytes.clone());

    let blake3_dir = dir.join("blobs").join("blake3");
    let tombs_dir = dir.join("blobs").join(".tombstones").join("blake3");

    // No SetBlob yet → sweep with grace=1h tombstones the blob.
    let _ = rooms.sweep_blobs(Duration::from_secs(3600)).await;
    let tomb = tombs_dir.join(hash.to_hex());
    assert!(
        tomb.exists(),
        "blob should be tombstoned while unreferenced"
    );

    // Now install a SetBlob op referencing it.
    let node = make_setblob_node(&sk, "k", hash);
    let (accepted, _, _) = import_nodes(&room, vec![node]).await;
    assert_eq!(accepted, 1);

    // Even with grace=0, the sweep must clear the tombstone rather than
    // delete: the live set wins.
    let deleted = rooms.sweep_blobs(Duration::ZERO).await;
    assert_eq!(deleted, 0);
    let blob_path = blake3_dir.join(hash.to_hex());
    assert!(blob_path.exists(), "blob must survive once it's live again");
    assert!(
        !tomb.exists(),
        "tombstone must be cleared once blob is live"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn blob_gc_protects_blobs_in_cold_non_resident_rooms() {
    // The correctness landmine of moving to a global CAS pool: a sweep
    // driven only by resident rooms would incorrectly tombstone (and
    // eventually delete) blobs belonging to rooms nobody has loaded since
    // the server started. `Rooms::sweep_blobs` must union in every room
    // known to persistence, not just resident ones.
    let dir = tmpdir("cold-room");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0xCCu8; 32]);

    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0x04u8; 32]),
        Arc::clone(&persistence),
        512,
        0,
        0,
    );

    // Room A gets a blob referenced by a SetBlob node, then is dropped —
    // simulating a room that hydrated once and is no longer resident.
    let bytes = b"owned-by-a-cold-room".to_vec();
    let hash = Hash::of(&bytes);
    {
        let room_a = rooms.get_or_create("room-a").await;
        persistence.persist_blob(&hash, &bytes);
        room_a.blobs.write().await.put(bytes.clone());
        let node = make_setblob_node(&sk, "k", hash);
        let (accepted, _, _) = import_nodes(&room_a, vec![node]).await;
        assert_eq!(accepted, 1);
    }
    // Room A has no connected peers and is idle from the moment it's
    // created (see Room::new), so any non-negative timeout evicts it —
    // once the background hydrate task spawned by get_or_create releases
    // its temporary Arc handle (see idle_eviction.rs for the same race).
    let mut evicted = Vec::new();
    for _ in 0..20 {
        evicted = rooms
            .sweep_idle(Duration::ZERO, std::time::Instant::now())
            .await;
        if evicted.contains(&"room-a".to_string()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        evicted.contains(&"room-a".to_string()),
        "room-a must be evicted (non-resident) before the sweep below"
    );

    // Only room B is resident when the sweep runs.
    let _room_b = rooms.get_or_create("room-b").await;

    let deleted = rooms.sweep_blobs(Duration::ZERO).await;
    assert_eq!(
        deleted, 0,
        "sweep must not delete a blob owned by a non-resident room"
    );
    let blob_path = dir.join("blobs").join("blake3").join(hash.to_hex());
    assert!(
        blob_path.exists(),
        "cold room's blob must survive a sweep driven by a different resident room"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn blob_gc_two_phase_handles_zstd_encoded_blobs_and_ignores_foreign_files() {
    // S3.1b: the sweep's name recognizer (`parse_blob_entry_name`) must
    // treat `<hex>.zst` exactly like `<hex>` for tombstone/delete purposes
    // (tombstone keyed by bare hex, live-set check by bare hash), while a
    // foreign `.gz` file must be left alone entirely — never tombstoned,
    // never deleted.
    use nodalmerge_server::store::BlobCompressionConfig;

    let dir = tmpdir("zstd-encoded");
    let cfg = BlobCompressionConfig {
        enabled: true,
        level: 3,
        min_bytes: 16, // low threshold so small test payloads still compress
    };
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open_with_compression(&dir, cfg).unwrap());
    let sk = SigningKey::from_bytes(&[0xDDu8; 32]);
    let room_id = "gc-zstd-room".to_string();

    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0x05u8; 32]),
        Arc::clone(&persistence),
        512,
        0,
        0,
    );
    let room = rooms.get_or_create(&room_id).await;

    // Two highly compressible blobs — persist_blob's heuristic should store
    // both as `<hex>.zst`. One is genuinely abandoned (deleted after grace);
    // the other is re-referenced mid-grace (tombstone cleared).
    let deleted_bytes = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_vec();
    let deleted_hash = Hash::of(&deleted_bytes);
    let revived_bytes = b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_vec();
    let revived_hash = Hash::of(&revived_bytes);
    persistence.persist_blob(&deleted_hash, &deleted_bytes);
    persistence.persist_blob(&revived_hash, &revived_bytes);
    room.blobs.write().await.put(deleted_bytes.clone());
    room.blobs.write().await.put(revived_bytes.clone());

    let blake3_dir = dir.join("blobs").join("blake3");
    let tombs_dir = dir.join("blobs").join(".tombstones").join("blake3");
    let deleted_encoded_path = blake3_dir.join(format!("{}.zst", deleted_hash.to_hex()));
    let revived_encoded_path = blake3_dir.join(format!("{}.zst", revived_hash.to_hex()));
    assert!(
        deleted_encoded_path.is_file(),
        "expected the blob to be stored zstd-encoded at {deleted_encoded_path:?}"
    );
    assert!(revived_encoded_path.is_file());

    // A foreign file that must never be touched by GC, regardless of grace.
    let foreign_path = blake3_dir.join("not-a-real-hash.gz");
    std::fs::write(&foreign_path, b"unrelated file").unwrap();

    let grace = Duration::from_millis(50);
    let deleted_tomb = tombs_dir.join(deleted_hash.to_hex());
    let revived_tomb = tombs_dir.join(revived_hash.to_hex());

    // Phase 1: both are orphans (no SetBlob references either yet) — both
    // get tombstoned, keyed by bare hex (not `<hex>.zst`).
    let deleted_count = rooms.sweep_blobs(grace).await;
    assert_eq!(deleted_count, 0, "first sweep only tombstones");
    assert!(deleted_encoded_path.exists());
    assert!(revived_encoded_path.exists());
    assert!(deleted_tomb.exists(), "must be tombstoned under its bare hex");
    assert!(revived_tomb.exists(), "must be tombstoned under its bare hex");
    assert!(foreign_path.exists(), "foreign .gz must never be touched");

    // Re-reference `revived_hash` before grace elapses.
    let node = make_setblob_node(&sk, "avatar", revived_hash);
    let (accepted, _, errs) = import_nodes(&room, vec![node]).await;
    assert_eq!(accepted, 1);
    assert!(errs.is_empty());

    // Second sweep, still within grace for the tombstones created above:
    // revived is now live -> tombstone cleared, nothing deleted yet.
    let deleted_count = rooms.sweep_blobs(grace).await;
    assert_eq!(deleted_count, 0, "still within grace for `deleted_hash`");
    assert!(revived_encoded_path.exists(), "revived .zst blob must survive");
    assert!(!revived_tomb.exists(), "tombstone cleared once live again");
    assert!(foreign_path.exists());

    // Let the grace window elapse, then sweep again: `deleted_hash` is
    // still unreferenced and its tombstone is now old enough to delete.
    tokio::time::sleep(Duration::from_millis(80)).await;
    let deleted_count = rooms.sweep_blobs(grace).await;
    assert_eq!(deleted_count, 1, "exactly the still-orphaned .zst blob");
    assert!(!deleted_encoded_path.exists(), "orphaned .zst blob must be gone");
    assert!(!deleted_tomb.exists(), "its tombstone is cleaned up with it");
    assert!(revived_encoded_path.exists(), "revived blob still safe");
    assert!(foreign_path.exists(), "foreign .gz untouched throughout");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn blob_gc_is_noop_on_in_memory_persistence() {
    use nodalmerge_server::store::NoPersistence;
    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0x03u8; 32]),
        Arc::new(NoPersistence),
        512,
        0,
        0,
    );
    let _ = rooms.get_or_create("x").await;
    let deleted = rooms.sweep_blobs(Duration::ZERO).await;
    assert_eq!(deleted, 0, "NoPersistence must never report deletions");
}

// ─── 0.3 — GC data-loss regression suite (blob-cas-remediation.md Phase 0/1) ──
//
// Scenarios (a), (c), (d) live here (Rooms::sweep_blobs / legacy two-phase
// world); (b) and (e) live in tests/studio_gc.rs (the new coordinator's
// world). See the plan's Phase 0 §0.3 and Phase 1 for the finding each
// gates.

#[tokio::test]
#[ignore = "RED: fails until slice 1.1 — see nodalmerge-studio/plans/blob-cas-remediation.md"]
async fn blob_gc_composite_must_forward_known_room_ids_for_cold_rooms() {
    // 0.3(a) — gates slice 1.1 (finding #1). `Composite<N, B>`'s
    // `impl NodePersistence` (server/server/src/store.rs:291) never
    // overrides `known_room_ids`, so it always falls through to the
    // trait's empty-`Vec` default (store.rs:90) — even when the wrapped
    // node store can genuinely enumerate every room with persisted nodes.
    // `Composite` is exactly how the production `server-s3` binary wires a
    // real node store to a blob backend (see nodalmerge-s3-blobs's module
    // doc), so this is the production wiring shape, not a synthetic one —
    // this test is `blob_gc_protects_blobs_in_cold_non_resident_rooms`
    // above, with the persistence swapped from a bare `DirPersistence` to
    // `Composite<DirPersistence, DirPersistence>`.
    let dir = tmpdir("composite-cold-room");
    let node_side = DirPersistence::open(&dir).unwrap();
    let blob_side = DirPersistence::open(&dir).unwrap();
    let sk = SigningKey::from_bytes(&[0xC1u8; 32]);

    let persistence: SharedPersistence = Arc::new(Composite::new(node_side, blob_side));
    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0x07u8; 32]),
        Arc::clone(&persistence),
        512,
        0,
        0,
    );

    let bytes = b"owned-by-a-cold-room-via-composite".to_vec();
    let hash = Hash::of(&bytes);
    {
        let room_a = rooms.get_or_create("room-a").await;
        persistence.persist_blob(&hash, &bytes);
        room_a.blobs.write().await.put(bytes.clone());
        let node = make_setblob_node(&sk, "k", hash);
        let (accepted, _, _) = import_nodes(&room_a, vec![node]).await;
        assert_eq!(accepted, 1);
    }
    // Evict room-a exactly like the plain-DirPersistence cold-room test
    // above.
    let mut evicted = Vec::new();
    for _ in 0..20 {
        evicted = rooms
            .sweep_idle(Duration::ZERO, std::time::Instant::now())
            .await;
        if evicted.contains(&"room-a".to_string()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        evicted.contains(&"room-a".to_string()),
        "room-a must be evicted (non-resident) before the sweep below"
    );

    // Sanity: the underlying node store genuinely CAN enumerate this room —
    // proves the gap is Composite's forwarding, not DirPersistence's
    // ability. A fresh handle on the same directory, called directly
    // (bypassing Composite).
    let probe = DirPersistence::open(&dir).unwrap();
    assert!(
        probe.known_room_ids().iter().any(|id| id == "room-a"),
        "sanity: the real node store can enumerate room-a directly"
    );

    // Only room-b is resident when the sweep runs.
    let _room_b = rooms.get_or_create("room-b").await;

    let deleted = rooms.sweep_blobs(Duration::ZERO).await;
    assert_eq!(
        deleted, 0,
        "Composite must forward known_room_ids so cold room-a's blob survives"
    );
    let blob_path = dir.join("blobs").join("blake3").join(hash.to_hex());
    assert!(
        blob_path.exists(),
        "cold room's blob (owned via Composite) must survive a sweep driven by a different resident room"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
#[ignore = "RED: fails until slice 1.4 — see nodalmerge-studio/plans/blob-cas-remediation.md"]
async fn blob_gc_survives_write_through_during_still_hydrating_room() {
    // 0.3(d) — gates slice 1.4 (finding #5). `Rooms::get_or_create`
    // (server/server/src/room.rs:427) inserts a new `Room` into the
    // resident map and spawns its persistence hydration in the background
    // *before* that hydration has loaded anything into the room's
    // in-memory graph. `sweep_blobs` (room.rs:644) treats residency alone
    // as "covered" and skips the cold-room persisted-node scan for that
    // room id — so a blob referenced only by nodes this room persisted
    // *before this process started* (i.e. nothing racy about the write
    // itself — the race is purely "hydration hasn't run yet") is invisible
    // to the live set and gets swept. `--blob-gc-grace 0` (the plan's
    // stated repro condition) makes the deletion immediate, observable in
    // a single sweep call.
    //
    // Determinism note: `#[tokio::test]` defaults to the current-thread
    // flavor, and nothing between `get_or_create` and `sweep_blobs` below
    // is a genuine suspend point (uncontended `tokio::sync::RwLock`
    // acquisitions resolve without yielding), so the background hydration
    // task spawned by `get_or_create` cannot run before `sweep_blobs`
    // executes.
    let dir = tmpdir("hydration-race");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0xC2u8; 32]);
    let room_id = "hydrate-race-room".to_string();

    // A node + its referenced blob, durably persisted as though by a prior
    // server process — nothing yet loaded into any in-memory `Room`.
    let bytes = b"referenced-by-a-not-yet-hydrated-room".to_vec();
    let hash = Hash::of(&bytes);
    persistence.persist_blob(&hash, &bytes);
    let node = make_setblob_node(&sk, "avatar", hash);
    persistence.persist_node(&room_id, &node);

    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0x08u8; 32]),
        Arc::clone(&persistence),
        512,
        0,
        0,
    );

    // Creates the room and spawns its background hydration task, which has
    // not yet had a chance to run — see the determinism note above.
    let _room = rooms.get_or_create(&room_id).await;

    let deleted = rooms.sweep_blobs(Duration::ZERO).await;
    assert_eq!(
        deleted, 0,
        "a still-hydrating resident room's persisted blob must survive the sweep"
    );
    let blob_path = dir.join("blobs").join("blake3").join(hash.to_hex());
    assert!(
        blob_path.exists(),
        "blob referenced by a not-yet-hydrated room must survive"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
#[ignore = "RED: fails until slice 1.3 — see nodalmerge-studio/plans/blob-cas-remediation.md"]
async fn blob_gc_s3_path_honors_nonzero_grace() {
    // 0.3(c) — gates slice 1.3 (finding #3). `S3BlobStore::blob_gc_sweep`
    // (server/s3-blobs/src/lib.rs:524) binds `_grace` and never reads it —
    // a single-pass immediate delete regardless of the caller's grace
    // window. `NonHydratingBackend::blob_gc_sweep`
    // (server/stores/blob-conformance/src/lib.rs) is a byte-for-byte port
    // of that exact (buggy) logic, so this test exercises the real bug
    // shape through `Rooms::sweep_blobs`/`Composite` without needing a live
    // S3/MinIO endpoint (server/s3-blobs/tests/minio_round_trip.rs covers
    // the real backend end-to-end but requires Docker).
    let dir = tmpdir("s3-path-grace");
    let nodes = DirPersistence::open(&dir).unwrap();
    let blobs = NonHydratingBackend::default();

    let orphan_bytes = b"orphan-on-the-s3-path".to_vec();
    let orphan_hash = Hash::of(&orphan_bytes);
    // Simulates a completed presigned upload the backend never saw bytes
    // for — exactly how a real upload-confirm marks an S3 object live.
    blobs.mark_present(orphan_hash);

    let persistence: SharedPersistence = Arc::new(Composite::new(nodes, blobs));
    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0x09u8; 32]),
        Arc::clone(&persistence),
        512,
        0,
        0,
    );
    let _room = rooms.get_or_create("gc-s3-room").await;

    // Long grace: a correct two-phase S3 sweep must NOT delete on the very
    // first pass.
    let deleted = rooms.sweep_blobs(Duration::from_secs(3600)).await;
    assert_eq!(
        deleted, 0,
        "the S3 path must honor a non-zero grace, not delete on the first pass"
    );
}
