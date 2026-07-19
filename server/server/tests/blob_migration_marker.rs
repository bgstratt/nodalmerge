//! Slice 5.1 (nodalmerge-studio/plans/blob-cas-remediation.md) —
//! `migrate_legacy_blob_layout` must write the `.layout-v2` marker only on
//! a fully clean pass.
//!
//! Pre-5.1 the marker was written unconditionally, even when room dirs or
//! individual entries were skipped (read-dir error, non-Unicode name,
//! mkdir failure, copy-fallback failure, blocked quarantine). Because
//! readers only consult `blobs/blake3/` once the marker exists, every
//! skipped entry became a *permanently* orphaned legacy blob: still on
//! disk, invisible to `get_blob`, and never looked at again.
//!
//! Post-5.1 contract, pinned here end-to-end through `DirPersistence::open`
//! (the real startup path — `main.rs` opens the store exactly once per
//! process, so "marker absent" means "the next server start retries"):
//!   * any deferred entry ⇒ no marker, a warn per skipped entry, and the
//!     next open re-runs the whole scan;
//!   * the re-run is idempotent over already-migrated entries (the
//!     dest-exists collision is the dedup, never an error);
//!   * once the previously skipped entries migrate cleanly, THAT run
//!     writes the marker — the second-run-completes assertion is the
//!     point of the slice;
//!   * a partial destination file (crashed or failed copy fallback on an
//!     earlier incomplete run) is repaired from the legacy source, never
//!     "deduped" against — deleting the only good copy on the strength of
//!     a partial dest would be data loss.

use nodalmerge_core::Hash;
use nodalmerge_server::store::{BlobPersistence, DirPersistence};
use std::path::PathBuf;

fn tmpdir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("nodalmerge-migmarker-{tag}-{nanos}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// A directory name that is valid for the local filesystem but is NOT
/// valid Unicode, so the migration's `file_name().to_str()` sees `None`
/// and cannot process the room dir. This is the portable stand-in for
/// "a room dir the scan cannot read": constructible on both Unix (raw
/// non-UTF-8 bytes) and Windows/NTFS (an unpaired UTF-16 surrogate).
#[cfg(unix)]
fn non_unicode_dir_name() -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    // "room" + an invalid UTF-8 continuation byte.
    std::ffi::OsString::from_vec(vec![b'r', b'o', b'o', b'm', 0xFF])
}

#[cfg(windows)]
fn non_unicode_dir_name() -> std::ffi::OsString {
    use std::os::windows::ffi::OsStringExt;
    // "room" + an unpaired high surrogate — legal in NTFS names, not in
    // Unicode, so `to_str()` returns `None`.
    std::ffi::OsString::from_wide(&[0x72, 0x6F, 0x6F, 0x6D, 0xD800])
}

/// The headline 5.1 scenario: one healthy legacy room and one room dir the
/// scan cannot process. Run 1 must migrate the healthy room but LEAVE THE
/// MARKER ABSENT; after the operator fixes the bad dir (here: renames it),
/// run 2 migrates the remainder and only then writes the marker.
#[test]
fn skipped_room_dir_defers_marker_and_second_run_migrates_remainder() {
    let root = tmpdir("skipped-room");
    let blobs_root = root.join("blobs");

    // Healthy legacy room — must migrate on run 1 regardless of the skip.
    let ok_bytes = b"healthy legacy blob".to_vec();
    let ok_hash = Hash::of(&ok_bytes);
    let ok_room = blobs_root.join("room-ok");
    std::fs::create_dir_all(&ok_room).unwrap();
    std::fs::write(ok_room.join(ok_hash.to_hex()), &ok_bytes).unwrap();

    // A room dir with a non-Unicode name: the scan cannot process it, so
    // its blob must be treated as deferred, not silently orphaned.
    let orphan_bytes = b"blob at risk of being orphaned".to_vec();
    let orphan_hash = Hash::of(&orphan_bytes);
    let bad_room = blobs_root.join(non_unicode_dir_name());
    std::fs::create_dir_all(&bad_room).expect("filesystem must accept the non-Unicode dir name");
    std::fs::write(bad_room.join(orphan_hash.to_hex()), &orphan_bytes).unwrap();

    let marker = blobs_root.join(".layout-v2");

    // Run 1: the healthy room migrates, the bad room defers the marker.
    {
        let store = DirPersistence::open(&root).unwrap();
        assert_eq!(
            store.get_blob(&ok_hash),
            Some(ok_bytes.clone()),
            "healthy legacy blob must be readable after run 1"
        );
        assert_eq!(
            store.get_blob(&orphan_hash),
            None,
            "the skipped room's blob is not yet migrated on run 1"
        );
        assert!(
            !marker.exists(),
            "RED (pre-5.1): .layout-v2 was written despite a skipped room dir — \
             the legacy blob in the non-Unicode dir is now permanently orphaned"
        );
    }

    // Operator remediation between restarts: give the dir a readable name.
    std::fs::rename(&bad_room, blobs_root.join("room-fixed")).unwrap();

    // Run 2 (next startup): the remainder migrates, and only now the marker.
    {
        let store = DirPersistence::open(&root).unwrap();
        assert_eq!(
            store.get_blob(&orphan_hash),
            Some(orphan_bytes.clone()),
            "run 2 must migrate the previously skipped room's blob"
        );
        assert_eq!(store.get_blob(&ok_hash), Some(ok_bytes.clone()));
        assert!(
            marker.is_file(),
            "a fully clean second pass must write the marker"
        );
    }

    // Run 3: marker present ⇒ no rescan, everything still readable.
    let store = DirPersistence::open(&root).unwrap();
    assert_eq!(store.get_blob(&orphan_hash), Some(orphan_bytes));
    assert_eq!(store.get_blob(&ok_hash), Some(ok_bytes));
}

/// A failed quarantine is a skip too: if `.migration-skipped/` cannot be
/// created (here: the path is occupied by a file), the non-conforming
/// entry stays in the legacy room dir — so the marker must stay absent
/// until a later run parks it properly. A *successful* quarantine remains
/// a terminal disposition and does NOT defer the marker (pinned by the
/// existing inline `migrate_legacy_blob_layout_moves_and_dedupes` test).
#[test]
fn blocked_quarantine_defers_marker_and_second_run_completes() {
    let root = tmpdir("blocked-quarantine");
    let blobs_root = root.join("blobs");

    let bytes = b"legacy blob next to junk".to_vec();
    let hash = Hash::of(&bytes);
    let room = blobs_root.join("room-a");
    std::fs::create_dir_all(&room).unwrap();
    std::fs::write(room.join(hash.to_hex()), &bytes).unwrap();
    // Non-conforming entry that must be quarantined…
    std::fs::write(room.join("not-a-hash"), b"junk").unwrap();
    // …except quarantine is blocked: `.migration-skipped` is a FILE, so
    // create_dir_all fails on both Unix and Windows.
    std::fs::write(blobs_root.join(".migration-skipped"), b"in the way").unwrap();

    let marker = blobs_root.join(".layout-v2");

    // Run 1: the valid blob migrates; the blocked quarantine defers the marker.
    {
        let store = DirPersistence::open(&root).unwrap();
        assert_eq!(store.get_blob(&hash), Some(bytes.clone()));
        assert!(
            room.join("not-a-hash").is_file(),
            "the non-conforming entry must stay in place when quarantine is blocked — never deleted"
        );
        assert!(
            !marker.exists(),
            "RED (pre-5.1): .layout-v2 was written despite a blocked quarantine — \
             the entry is stranded in the legacy dir forever"
        );
    }

    // Unblock quarantine between restarts.
    std::fs::remove_file(blobs_root.join(".migration-skipped")).unwrap();

    // Run 2: the leftover entry is quarantined, and only now the marker.
    {
        let store = DirPersistence::open(&root).unwrap();
        assert!(
            blobs_root
                .join(".migration-skipped")
                .join("room-a__not-a-hash")
                .is_file(),
            "run 2 must park the leftover entry in quarantine"
        );
        assert!(marker.is_file(), "clean second pass must write the marker");
        assert_eq!(store.get_blob(&hash), Some(bytes));
    }
}

/// Idempotency hazard of the retry loop this slice introduces: a crashed or
/// failed copy fallback on an earlier (marker-less) run can leave a PARTIAL
/// file at the canonical destination. The re-run's dest-exists collision
/// branch used to treat any existing dest as "already migrated" and delete
/// the legacy source — destroying the only good copy. The re-run must
/// instead verify the dest and repair it from the legacy bytes.
#[test]
fn partial_destination_is_repaired_not_adopted() {
    let root = tmpdir("partial-dest");
    let blobs_root = root.join("blobs");

    let bytes = b"the only good copy of this blob".to_vec();
    let hash = Hash::of(&bytes);

    // Legacy source with the real bytes…
    let room = blobs_root.join("room-b");
    std::fs::create_dir_all(&room).unwrap();
    std::fs::write(room.join(hash.to_hex()), &bytes).unwrap();
    // …and a partial/corrupt file already at the canonical destination,
    // with NO marker — exactly what an interrupted copy fallback leaves.
    let blake3_dir = blobs_root.join("blake3");
    std::fs::create_dir_all(&blake3_dir).unwrap();
    let dest = blake3_dir.join(hash.to_hex());
    std::fs::write(&dest, b"the only goo").unwrap();

    let store = DirPersistence::open(&root).unwrap();

    assert_eq!(
        std::fs::read(&dest).ok(),
        Some(bytes.clone()),
        "RED (pre-5.1): the partial destination was adopted as-is and the legacy source deleted — data loss"
    );
    assert_eq!(
        store.get_blob(&hash),
        Some(bytes),
        "the repaired blob must be readable (get_blob verifies BLAKE3 on read)"
    );
    // The repair is not a skip: this pass was clean, so the marker lands.
    assert!(blobs_root.join(".layout-v2").is_file());
}
