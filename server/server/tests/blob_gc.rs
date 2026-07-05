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
//! We drive `Rooms::sweep_blobs` directly instead of spawning the task
//! so the test is synchronous, deterministic, and doesn't race `tokio::
//! time::interval`. Grace is set to 50 ms so the two-phase behavior is
//! observable without sleeping for the production 24 h default.

use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use nodalmerge_core::{BlobStore, Hash, MapOp, Op, StateGraph};
use nodalmerge_server::room::{import_nodes, Rooms};
use nodalmerge_server::store::{DirPersistence, SharedPersistence};

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
    persistence.persist_blob(&room_id, &live_hash, &live_bytes);
    persistence.persist_blob(&room_id, &orphan_hash, &orphan_bytes);
    room.blobs.write().await.put(live_bytes.clone());
    room.blobs.write().await.put(orphan_bytes.clone());

    // --- install a SetBlob node that references ONLY the live hash --------
    let node = make_setblob_node(&sk, "avatar", live_hash);
    let (accepted, _, errs) = import_nodes(&room, vec![node]).await;
    assert_eq!(accepted, 1);
    assert!(errs.is_empty());

    // Short grace so we can observe the two phases in-test.
    let grace = Duration::from_millis(50);

    // --- Phase 1: tombstones the orphan, deletes nothing. -----------------
    let deleted = rooms.sweep_blobs(grace).await;
    assert_eq!(deleted, 0, "first sweep must only tombstone, not delete");
    let live_path = dir.join("blobs").join(&room_id).join(live_hash.to_hex());
    let orphan_path = dir.join("blobs").join(&room_id).join(orphan_hash.to_hex());
    let orphan_tomb = dir
        .join("blob-tombstones")
        .join(&room_id)
        .join(orphan_hash.to_hex());
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
    persistence.persist_blob(&room_id, &hash, &bytes);
    room.blobs.write().await.put(bytes.clone());

    // No SetBlob yet → sweep with grace=1h tombstones the blob.
    let _ = rooms.sweep_blobs(Duration::from_secs(3600)).await;
    let tomb = dir
        .join("blob-tombstones")
        .join(&room_id)
        .join(hash.to_hex());
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
    let blob_path = dir.join("blobs").join(&room_id).join(hash.to_hex());
    assert!(blob_path.exists(), "blob must survive once it's live again");
    assert!(
        !tomb.exists(),
        "tombstone must be cleared once blob is live"
    );

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
