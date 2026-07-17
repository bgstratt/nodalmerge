//! Slice 1.5 (finding #9, plans/blob-cas-remediation.md) — bounds the tree
//! walk so a pathological chain of single-entry directory tree blobs cannot
//! overflow the worker stack and abort the whole server process.
//!
//! Pre-fix, `tree_walk::walk_one` recursed once per directory level with no
//! depth bound; a chain of N distinct single-entry v2 tree blobs (each
//! trivially uploadable via anonymous blob PUT — see 1.5's note in the plan)
//! blew the stack and aborted the process. Post-fix, the walk is an explicit
//! heap-allocated work-stack with a hard depth cap
//! (`tree_walk::MAX_TREE_DEPTH`), so the same fixture returns a clean `Err`
//! instead of aborting.
//!
//! This test runs the walk on a spawned thread with a deliberately small
//! stack (256 KiB, versus the platform default of several MiB) so a
//! regression back to unbounded per-level recursion reliably reproduces a
//! stack overflow at a depth this test can afford to construct, rather than
//! requiring millions of frames to exhaust a default-size stack. A stack
//! overflow aborts the whole process (SIGSEGV / STATUS_STACK_OVERFLOW is not
//! catchable in safe Rust) — so if this ever regresses, the failure mode is
//! the test binary crashing outright, not a clean assertion failure. See the
//! slice's final report for how the pre-fix abort was independently
//! confirmed (via `git stash` against the original recursive `walk_one`,
//! since a test that reliably aborts the process can't be the thing that
//! ships gated in the suite).

use std::collections::HashMap as StdHashMap;
use std::sync::Mutex;

use nodalmerge_core::Hash;
use nodalmerge_server::store::{BlobPersistence, PersistBlobError};
use nodalmerge_server::tree_walk::{walk_tree, TreeWalkError};

#[derive(Default, Debug)]
struct MemBlobs {
    map: Mutex<StdHashMap<Hash, Vec<u8>>>,
}

impl MemBlobs {
    fn put_json(&self, json: serde_json::Value) -> Hash {
        let bytes = serde_json::to_vec(&json).unwrap();
        let h = Hash::of(&bytes);
        self.map.lock().unwrap().insert(h, bytes);
        h
    }
}

impl BlobPersistence for MemBlobs {
    fn get_blob(&self, hash: &Hash) -> Option<Vec<u8>> {
        self.map.lock().unwrap().get(hash).cloned()
    }
    fn persist_blob(&self, hash: &Hash, bytes: &[u8]) -> Result<(), PersistBlobError> {
        self.map.lock().unwrap().insert(*hash, bytes.to_vec());
        Ok(())
    }
}

/// Build a chain of `depth` single-entry v2 directory tree blobs, each
/// pointing at the next by hash, terminating in a single file entry. Returns
/// the root hash (the outermost directory in the chain).
fn build_deep_chain(store: &MemBlobs, depth: usize) -> Hash {
    let file_hash = Hash::of(b"leaf file");
    let mut next = store.put_json(serde_json::json!({
        "nodalmerge": "tree", "version": 2,
        "entries": [{"n": "leaf.txt", "k": "f", "h": file_hash.to_hex()}]
    }));
    for _ in 0..depth {
        next = store.put_json(serde_json::json!({
            "nodalmerge": "tree", "version": 2,
            "entries": [{"n": "d", "k": "d", "h": next.to_hex()}]
        }));
    }
    next
}

#[test]
fn deep_chain_fails_closed_instead_of_overflowing_the_stack() {
    let store = MemBlobs::default();
    // Comfortably past the walk's MAX_TREE_DEPTH (4096) — a synthetic
    // adversarial chain, not a plausible real repo tree.
    let root = build_deep_chain(&store, 50_000);

    // Run on a thread with a deliberately small stack: if this ever
    // regresses to unbounded per-level recursion, the *test binary itself*
    // aborts (stack overflow) rather than this assertion failing cleanly —
    // that is the point of driving it from a small-stack thread instead of
    // the test harness's own (much larger) thread.
    let handle = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || walk_tree(&store, &root))
        .expect("spawn thread");

    let result = handle.join().expect("walk must not panic/abort the thread");
    let err = result.expect_err("a chain this deep must fail closed, not succeed silently");
    assert!(matches!(err, TreeWalkError::TooDeep { .. }), "expected TooDeep, got {err:?}");
}

#[test]
fn moderate_chain_well_within_the_cap_still_succeeds() {
    // Guards against an overly-aggressive cap breaking legitimate deep-ish
    // repos: a few hundred directory levels — already far more than any real
    // project — must still walk successfully.
    let store = MemBlobs::default();
    let root = build_deep_chain(&store, 300);

    let live = walk_tree(&store, &root).expect("a moderate chain must not be rejected");
    // root + 300 intermediate dirs + 1 leaf file = 302 distinct hashes.
    assert_eq!(live.len(), 302);
}
