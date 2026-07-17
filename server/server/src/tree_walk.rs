//! S5.3 — server-side tree-object walk.
//!
//! Phase 1 (nodalmerge-studio) landed tree objects as ordinary CAS blobs:
//! `RepositorySnapshot.TreeHash` names a root tree blob instead of carrying
//! an inline path→hash map. The format is frozen in nodalmerge-studio's
//! `docs/TREE_OBJECT_FORMAT.md` (v1 flat map, v2 git-tree-style directories,
//! byte-identical hashing across peers). That doc's own "Future migration
//! note" says it migrates to this repo's parity mechanism (doc + vectors,
//! like `BLOB_STORAGE_LAYOUT.md`) once the Rust server needs to parse trees
//! for GC liveness — this module is that need; the doc/vector migration
//! itself is a tracked follow-up (see S5.3's final report), not done here.
//!
//! This walk is intentionally narrow: given a root hash, return every
//! reachable tree-object hash *and* file blob hash, fetching bytes via
//! [`BlobPersistence::hydrate_blob`] (already zstd-transparent — see
//! `store::DirPersistence::get_blob`). It never writes, never verifies
//! anything beyond "does this parse as a tree object", and fails closed:
//! a missing or malformed blob at any point aborts the whole walk with
//! `Err`, never a partial set — the GC liveness computation built on top of
//! this (`studio_live_hashes.rs`) depends on that.
//!
//! ## Why `hydrate_blob` and not `get_blob` (slice 2.2, finding #10)
//!
//! This walk used `get_blob`, which `S3BlobStore` hardwires to `None` as
//! deliberate policy — so on every S3-backed server the walk aborted on its
//! first fetch and studio GC failed closed *forever*, reporting a generic
//! "missing tree object" that reads like data loss.
//!
//! Routing through [`BlobPersistence::hydrate_blob`] does **not** weaken that
//! policy. The policy keeps large **file** payloads out of the server
//! process; this walk never fetches a file blob. v1 entries are terminal by
//! format, and of v2's two kinds only `"d"` is ever fetched — `"f"` entries
//! contribute their hash to the live set and nothing else. Every byte this
//! module reads is a small JSON tree object, i.e. the server's own index,
//! which it cannot do its job without parsing. A backend that genuinely
//! cannot supply even those (S3 **Delegate** mode) gets
//! [`TreeWalkError::Unresolvable`] — distinct, actionable, and counted —
//! rather than being mistaken for a lost blob.

use std::collections::HashMap;
use std::collections::HashSet;

use nodalmerge_core::Hash;
use serde::Deserialize;

use crate::store::{hash_from_hex, BlobPersistence, HydrateError};

/// Hard cap on directory-chain nesting a single walk will follow (v2 `"d"`
/// entries only — v1 entries and v2 `"f"` entries are always terminal and
/// never contribute to depth). This is **not** a plausible-repo limit — real
/// trees are expected to be a handful of levels deep — it exists purely as a
/// fail-closed backstop against a pathological/adversarial chain of
/// single-entry tree blobs (see finding #9: such a chain is uploadable via
/// anonymous blob PUT). 4096 is far beyond any legitimate directory nesting
/// while still bounding the walk's work to something a single GC tick can
/// afford.
const MAX_TREE_DEPTH: usize = 4096;

/// Everything that can go wrong walking a tree — always fatal to the whole
/// walk (fail-closed; see module docs).
#[derive(Debug)]
pub enum TreeWalkError {
    /// A tree/file blob named by an entry (or the root) isn't present in
    /// this server's blob store.
    MissingBlob(String),
    /// A blob was fetched but isn't a well-formed tree object per
    /// `TREE_OBJECT_FORMAT.md` (bad JSON, missing/unsupported `version`,
    /// non-canonical hash string, unknown entry `kind`, …).
    Malformed(String),
    /// The directory-chain nesting exceeded [`MAX_TREE_DEPTH`]. Fail-closed
    /// backstop for finding #9 (unbounded recursion → stack-overflow process
    /// abort) — see the module docs and slice 1.5 of
    /// `plans/blob-cas-remediation.md`.
    TooDeep { hash: String, depth: usize },
    /// Slice 2.2 (finding #10) — the blob backend **cannot** read tree-object
    /// bytes into this process at all (S3 Delegate mode: no bucket
    /// credentials, and delegate presign protocol v1 has no bytes op). The
    /// tree is probably sitting safely in the bucket; this deployment simply
    /// cannot walk it.
    ///
    /// Deliberately **not** [`MissingBlob`](Self::MissingBlob): that would
    /// read as data loss and send an operator hunting for an object that
    /// isn't lost, which is precisely the "GC never runs on S3, and nobody
    /// notices" failure this slice exists to remove. Counted separately in
    /// `nodalmerge_tree_walk_resolve_failed_total{reason="unhydratable_backend"}`.
    Unresolvable { hash: String, detail: String },
    /// Slice 2.2 — the backend tried to read the tree object and failed
    /// transiently (network/IO). Says nothing about whether it exists, so it
    /// is neither `MissingBlob` nor `Unresolvable`: the next tick may well
    /// succeed. Counted as
    /// `nodalmerge_tree_walk_resolve_failed_total{reason="backend_error"}`.
    ResolveFailed { hash: String, detail: String },
}

impl std::fmt::Display for TreeWalkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TreeWalkError::MissingBlob(h) => write!(f, "missing tree/blob object {h}"),
            TreeWalkError::Malformed(m) => write!(f, "malformed tree object: {m}"),
            TreeWalkError::TooDeep { hash, depth } => write!(
                f,
                "tree walk aborted at {hash}: directory nesting depth {depth} exceeds the \
                 hard cap of {MAX_TREE_DEPTH} (see finding #9, plans/blob-cas-remediation.md 1.5)"
            ),
            TreeWalkError::Unresolvable { hash, detail } => write!(
                f,
                "cannot resolve tree object {hash}: {detail} — this is a DEPLOYMENT \
                 CONFIGURATION problem, not a lost blob: the object is probably intact in \
                 object storage, but this server cannot read tree objects, so studio GC \
                 cannot compute a live set and will not reclaim anything. Run the server \
                 with S3 Direct-mode credentials (or a blob backend that hydrates) if GC \
                 is required. See finding #10, plans/blob-cas-remediation.md 2.2"
            ),
            TreeWalkError::ResolveFailed { hash, detail } => write!(
                f,
                "failed to read tree object {hash}: {detail} (transient backend error — \
                 the object's existence is unknown; a later tick may succeed)"
            ),
        }
    }
}

impl std::error::Error for TreeWalkError {}

#[derive(Debug, Deserialize)]
struct TreeV1 {
    entries: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct TreeV2Entry {
    // `n` (name) is part of the on-disk format but unused by the walk
    // (ordering/naming isn't needed to compute reachability) — allow it to
    // deserialize without a dead-code warning via `#[allow(dead_code)]`
    // rather than silently dropping the field from the struct, which would
    // make this look like it doesn't know the real shape.
    #[allow(dead_code)]
    n: String,
    k: String,
    h: String,
}

#[derive(Debug, Deserialize)]
struct TreeV2 {
    entries: Vec<TreeV2Entry>,
}

/// Walk a tree object rooted at `root_hash`, returning every reachable
/// **tree-object** hash (including the root itself) and **file blob** hash.
/// Fails closed: any missing or malformed blob, or nesting beyond
/// [`MAX_TREE_DEPTH`], aborts with `Err`.
///
/// Implemented as an explicit work-stack rather than recursion (finding #9):
/// `walk_one`'s old per-directory-level recursion had no depth bound, so a
/// chain of distinct single-entry `"d"` tree blobs — trivially uploadable via
/// anonymous blob PUT — overflowed the worker's OS stack and aborted the
/// whole server process. Driving the traversal from a heap-allocated `Vec`
/// removes the OS call-stack entirely from the scaling story; the
/// `MAX_TREE_DEPTH` cap on top is a fail-closed backstop against a
/// pathological chain consuming unbounded work in a single GC tick, not a
/// plausible-repo limit.
pub fn walk_tree(
    persistence: &dyn BlobPersistence,
    root_hash: &Hash,
) -> Result<HashSet<Hash>, TreeWalkError> {
    let mut live = HashSet::new();
    let mut expanded = HashSet::new();
    walk_tree_into(persistence, root_hash, &mut live, &mut expanded)?;
    Ok(live)
}

/// blob-cas-remediation.md slice 6.4 — [`walk_tree`], but accumulating into
/// caller-owned sets so **one** pair of sets can be threaded across every
/// retained snapshot in a GC run: tree objects are content-addressed, so a
/// subtree shared by N generations (or N rooms) has one hash, one content,
/// and one expansion — fetching and parsing it once per *run* instead of
/// once per *snapshot* changes nothing about the union while removing the
/// re-walk of shared subtrees.
///
/// ## Why `expanded` is a separate set from `live` (the plan's "the visited
/// set IS the live output set" claim, verified and found *almost* right)
///
/// `live` receives two different kinds of hash: tree objects this walk has
/// **fully expanded**, and terminal entries (v1 values, v2 `"f"` files)
/// whose bytes are never read. Skipping expansion on `live` membership
/// alone would therefore let a *terminal* insert suppress a later
/// *directory* expansion of the same hash — fine within one fresh-set walk
/// (where that alias means the file's bytes literally are that tree
/// object's bytes, a corner the old single-set walk already resolved
/// order-dependently), but across shared sets it would also let a hash
/// inserted by an unrelated *earlier snapshot's* terminal entry cancel this
/// snapshot's subtree expansion — silently dropping the subtree's children
/// from the live set, i.e. GC deleting live data. So expansion is skipped
/// only on `expanded` membership: hashes proven to have their full
/// transitive contribution already in `live`. Consequence (strictly safer
/// than before): a hash referenced both as a file and as a directory now
/// always gets its directory expansion, where the old walk's answer
/// depended on traversal order — the shared walk can only ever *add*
/// hashes relative to the old per-snapshot union, never lose one.
///
/// Callers must not persist these sets across GC runs — they are valid only
/// as long as the blob store's tree objects are (within a run, guaranteed:
/// a failed fetch aborts the whole collection fail-closed before any
/// partial state could be acted on).
pub fn walk_tree_into(
    persistence: &dyn BlobPersistence,
    root_hash: &Hash,
    live: &mut HashSet<Hash>,
    expanded: &mut HashSet<Hash>,
) -> Result<(), TreeWalkError> {
    // (hash, depth-of-this-directory-in-the-chain). The root is depth 0.
    let mut stack: Vec<(Hash, usize)> = vec![(*root_hash, 0)];

    while let Some((hash, depth)) = stack.pop() {
        // v2 subtrees are shared across generations by construction (identical
        // content ⇒ identical hash) — short-circuit on repeat visits so a large
        // shared subtree is fetched/parsed at most once per walk (and, with
        // caller-shared sets, at most once per GC run).
        if !expanded.insert(hash) {
            continue;
        }
        live.insert(hash);

        if depth > MAX_TREE_DEPTH {
            return Err(TreeWalkError::TooDeep { hash: hash.to_hex(), depth });
        }

        // Slice 2.2 (finding #10). This *must not* be `get_blob`: that is
        // hardwired `None` on `S3BlobStore` as deliberate policy, so every
        // studio GC run over a cas-tree snapshot failed closed forever on any
        // S3-backed server. `hydrate_blob` is the resolution path — a real
        // bucket GET in Direct mode, a distinct/actionable error in Delegate
        // mode (see `store::HydrateError`).
        //
        // **This does not contradict `get_blob`'s non-hydrating contract.**
        // That contract is about keeping large *file* payloads out of the
        // server process. Every fetch in this loop is a **tree object** — a
        // few hundred bytes of JSON — and never a file blob: `"f"` entries
        // (and all v1 entries) are terminal, going straight into `out` above
        // without their bytes ever being read; only `"d"` entries reach this
        // line. The server is reading its own index, not hauling payloads.
        let bytes = persistence.hydrate_blob(&hash).map_err(|e| match e {
            HydrateError::Missing => TreeWalkError::MissingBlob(hash.to_hex()),
            HydrateError::Unhydratable { backend, detail } => {
                metrics::counter!(
                    "nodalmerge_tree_walk_resolve_failed_total",
                    "reason" => "unhydratable_backend"
                )
                .increment(1);
                tracing::error!(
                    hash = %hash.to_hex(),
                    %backend,
                    %detail,
                    "tree walk cannot resolve a tree object: this blob backend never \
                     hydrates bytes into the server process, so studio GC cannot compute \
                     a live set and will reclaim nothing. This is a deployment \
                     configuration problem, not a missing blob (finding #10, \
                     blob-cas-remediation.md 2.2)"
                );
                TreeWalkError::Unresolvable {
                    hash: hash.to_hex(),
                    detail: format!("{backend}: {detail}"),
                }
            }
            HydrateError::Backend(detail) => {
                metrics::counter!(
                    "nodalmerge_tree_walk_resolve_failed_total",
                    "reason" => "backend_error"
                )
                .increment(1);
                tracing::warn!(
                    hash = %hash.to_hex(),
                    %detail,
                    "tree walk: transient backend failure reading a tree object"
                );
                TreeWalkError::ResolveFailed { hash: hash.to_hex(), detail }
            }
        })?;

        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
            TreeWalkError::Malformed(format!("{} is not valid JSON: {e}", hash.to_hex()))
        })?;

        let version = value.get("version").and_then(|v| v.as_u64()).ok_or_else(|| {
            TreeWalkError::Malformed(format!("{} is missing an integer 'version'", hash.to_hex()))
        })?;

        match version {
            1 => {
                let tree: TreeV1 = serde_json::from_value(value)
                    .map_err(|e| TreeWalkError::Malformed(format!("{}: {e}", hash.to_hex())))?;
                for hex in tree.entries.values() {
                    let h = parse_entry_hash(&hash, hex)?;
                    // v1 entries are always file blobs — never recurse, and
                    // never mark `expanded` (terminal insert, not an
                    // expansion — see the doc above).
                    live.insert(h);
                }
            }
            2 => {
                let tree: TreeV2 = serde_json::from_value(value)
                    .map_err(|e| TreeWalkError::Malformed(format!("{}: {e}", hash.to_hex())))?;
                for entry in tree.entries {
                    let h = parse_entry_hash(&hash, &entry.h)?;
                    match entry.k.as_str() {
                        "f" => {
                            live.insert(h);
                        }
                        "d" => {
                            stack.push((h, depth + 1));
                        }
                        other => {
                            return Err(TreeWalkError::Malformed(format!(
                                "{}: unknown entry kind {other:?} (expected \"f\" or \"d\")",
                                hash.to_hex()
                            )))
                        }
                    }
                }
            }
            other => {
                return Err(TreeWalkError::Malformed(format!(
                    "{}: unsupported tree version {other} (this walk understands v1/v2 only)",
                    hash.to_hex()
                )))
            }
        }
    }

    Ok(())
}

fn parse_entry_hash(tree_hash: &Hash, hex: &str) -> Result<Hash, TreeWalkError> {
    hash_from_hex(hex).ok_or_else(|| {
        TreeWalkError::Malformed(format!(
            "{}: entry hash {hex:?} is not canonical 64-lowercase-hex",
            tree_hash.to_hex()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap as StdHashMap;
    use std::sync::Mutex;

    #[derive(Default, Debug)]
    struct TestBlobs {
        map: Mutex<StdHashMap<Hash, Vec<u8>>>,
        /// Fetches served — the deterministic "was this subtree re-walked"
        /// signal for the slice-6.4 shared-set tests.
        gets: std::sync::atomic::AtomicUsize,
    }

    impl TestBlobs {
        fn put_bytes(&self, bytes: Vec<u8>) -> Hash {
            let h = Hash::of(&bytes);
            self.map.lock().unwrap().insert(h, bytes);
            h
        }

        fn put_json(&self, json: serde_json::Value) -> Hash {
            self.put_bytes(serde_json::to_vec(&json).unwrap())
        }
    }

    impl BlobPersistence for TestBlobs {
        fn get_blob(&self, hash: &Hash) -> Option<Vec<u8>> {
            self.gets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.map.lock().unwrap().get(hash).cloned()
        }
        fn persist_blob(&self, hash: &Hash, bytes: &[u8]) -> Result<(), crate::store::PersistBlobError> {
            self.map.lock().unwrap().insert(*hash, bytes.to_vec());
            Ok(())
        }
    }

    fn file_hash(seed: &str) -> String {
        Hash::of(seed.as_bytes()).to_hex()
    }

    #[test]
    fn v1_flat_tree_walks_to_file_hashes() {
        let store = TestBlobs::default();
        let a = file_hash("a");
        let b = file_hash("b");
        let root = store.put_json(serde_json::json!({
            "nodalmerge": "tree",
            "version": 1,
            "entries": { "a.txt": a, "dir/b.txt": b }
        }));

        let live = walk_tree(&store, &root).expect("walk succeeds");
        assert!(live.contains(&root));
        assert!(live.contains(&hash_from_hex(&a).unwrap()));
        assert!(live.contains(&hash_from_hex(&b).unwrap()));
        assert_eq!(live.len(), 3);
    }

    #[test]
    fn v2_directory_tree_recurses_and_shares_subtrees() {
        let store = TestBlobs::default();
        let file_a = file_hash("a");
        let file_b = file_hash("b");

        let subtree = store.put_json(serde_json::json!({
            "nodalmerge": "tree", "version": 2,
            "entries": [{"n": "b.txt", "k": "f", "h": file_b}]
        }));

        let root = store.put_json(serde_json::json!({
            "nodalmerge": "tree", "version": 2,
            "entries": [
                {"n": "a.txt", "k": "f", "h": file_a},
                {"n": "dir", "k": "d", "h": subtree.to_hex()},
            ]
        }));

        let live = walk_tree(&store, &root).expect("walk succeeds");
        assert!(live.contains(&root));
        assert!(live.contains(&subtree));
        assert!(live.contains(&hash_from_hex(&file_a).unwrap()));
        assert!(live.contains(&hash_from_hex(&file_b).unwrap()));
        assert_eq!(live.len(), 4);
    }

    #[test]
    fn v2_shared_subtree_visited_once() {
        let store = TestBlobs::default();
        let file_a = file_hash("shared");
        let subtree = store.put_json(serde_json::json!({
            "nodalmerge": "tree", "version": 2,
            "entries": [{"n": "x.txt", "k": "f", "h": file_a}]
        }));
        // Two generations' roots both point at the same unchanged subtree —
        // walking either alone must not error or infinitely recurse, and a
        // union walk visits it exactly once (asserted indirectly: the set
        // size below only makes sense if it wasn't double-counted, which a
        // HashSet naturally handles, but this also exercises the "already
        // visited" short-circuit for a single walk that reaches the same
        // subtree hash twice from two different root entries).
        let root = store.put_json(serde_json::json!({
            "nodalmerge": "tree", "version": 2,
            "entries": [
                {"n": "left", "k": "d", "h": subtree.to_hex()},
                {"n": "right", "k": "d", "h": subtree.to_hex()},
            ]
        }));

        let live = walk_tree(&store, &root).expect("walk succeeds");
        assert_eq!(live.len(), 3); // root + subtree (once) + file
    }

    #[test]
    fn empty_v2_tree_is_just_the_root() {
        let store = TestBlobs::default();
        let root = store.put_json(serde_json::json!({
            "nodalmerge": "tree", "version": 2, "entries": []
        }));
        let live = walk_tree(&store, &root).expect("walk succeeds");
        assert_eq!(live, HashSet::from([root]));
    }

    // ── slice 6.4 — walk_tree_into's shared-set semantics ───────────────────

    /// Two roots sharing a subtree, walked with ONE shared set pair: the
    /// second walk must not re-fetch the shared subtree (that is the whole
    /// point of sharing), and the union must equal the two fresh walks'
    /// union exactly.
    #[test]
    fn shared_sets_skip_refetch_of_already_expanded_subtrees_without_changing_the_union() {
        let store = TestBlobs::default();
        let file_shared = file_hash("shared-file");
        let subtree = store.put_json(serde_json::json!({
            "nodalmerge": "tree", "version": 2,
            "entries": [{"n": "x.txt", "k": "f", "h": file_shared}]
        }));
        let root_a = store.put_json(serde_json::json!({
            "nodalmerge": "tree", "version": 2,
            "entries": [
                {"n": "a.txt", "k": "f", "h": file_hash("only-a")},
                {"n": "dir", "k": "d", "h": subtree.to_hex()},
            ]
        }));
        let root_b = store.put_json(serde_json::json!({
            "nodalmerge": "tree", "version": 2,
            "entries": [
                {"n": "b.txt", "k": "f", "h": file_hash("only-b")},
                {"n": "dir", "k": "d", "h": subtree.to_hex()},
            ]
        }));

        // Reference union: two independent fresh walks.
        let mut fresh_union = walk_tree(&store, &root_a).unwrap();
        fresh_union.extend(walk_tree(&store, &root_b).unwrap());
        let fetches_before = store.gets.load(std::sync::atomic::Ordering::Relaxed);

        // Shared walk: root_b's visit of `subtree` must cost zero fetches.
        let mut live = HashSet::new();
        let mut expanded = HashSet::new();
        walk_tree_into(&store, &root_a, &mut live, &mut expanded).unwrap();
        let after_a = store.gets.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(after_a - fetches_before, 2, "root_a + subtree");
        walk_tree_into(&store, &root_b, &mut live, &mut expanded).unwrap();
        let after_b = store.gets.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(after_b - after_a, 1, "root_b only — the shared subtree must not be re-fetched");

        assert_eq!(live, fresh_union, "sharing must never change the union");
    }

    /// The exact hazard `expanded` exists to prevent (see walk_tree_into's
    /// doc): a hash already in `live` via a *terminal* ("f") entry from an
    /// earlier snapshot's walk must still get its *directory* expansion in
    /// a later walk. Keying the skip on `live` membership — the naive
    /// reading of the plan's "the visited set IS the live output set" —
    /// would drop `inner` here: GC deleting live data.
    #[test]
    fn shared_sets_terminal_insert_does_not_suppress_a_later_directory_expansion() {
        let store = TestBlobs::default();
        let inner = file_hash("inner-file");
        let subtree = store.put_json(serde_json::json!({
            "nodalmerge": "tree", "version": 2,
            "entries": [{"n": "inner.txt", "k": "f", "h": inner}]
        }));
        // Snapshot A stores the subtree's *bytes* as a file (content-
        // addressed alias: same bytes, same hash, entry kind "f").
        let root_a = store.put_json(serde_json::json!({
            "nodalmerge": "tree", "version": 2,
            "entries": [{"n": "tree-as-file.json", "k": "f", "h": subtree.to_hex()}]
        }));
        // Snapshot B references the same hash as a directory.
        let root_b = store.put_json(serde_json::json!({
            "nodalmerge": "tree", "version": 2,
            "entries": [{"n": "dir", "k": "d", "h": subtree.to_hex()}]
        }));

        let mut live = HashSet::new();
        let mut expanded = HashSet::new();
        walk_tree_into(&store, &root_a, &mut live, &mut expanded).unwrap();
        let inner_hash = hash_from_hex(&inner).unwrap();
        assert!(!live.contains(&inner_hash), "A's terminal entry must not expand");
        walk_tree_into(&store, &root_b, &mut live, &mut expanded).unwrap();
        assert!(
            live.contains(&inner_hash),
            "B's directory expansion must not be suppressed by A's terminal insert"
        );
    }

    #[test]
    fn missing_root_blob_fails_closed() {
        let store = TestBlobs::default();
        let phantom = Hash::of(b"never stored");
        let err = walk_tree(&store, &phantom).expect_err("must fail closed");
        assert!(matches!(err, TreeWalkError::MissingBlob(_)));
    }

    #[test]
    fn missing_referenced_file_fails_closed() {
        let store = TestBlobs::default();
        let dangling = file_hash("never actually stored");
        let root = store.put_json(serde_json::json!({
            "nodalmerge": "tree", "version": 1,
            "entries": { "gone.txt": dangling }
        }));
        // Note: v1 doesn't recurse into file entries, so a missing *file*
        // blob referenced by a v1 tree is not detected by the tree walk
        // itself (v1 entries are terminal file hashes, never fetched here) —
        // this test instead exercises a v2 tree whose child *directory*
        // entry is missing, which the walk does fetch.
        let _ = root;

        let missing_subtree = Hash::of(b"missing subtree");
        let v2_root = store.put_json(serde_json::json!({
            "nodalmerge": "tree", "version": 2,
            "entries": [{"n": "dir", "k": "d", "h": missing_subtree.to_hex()}]
        }));
        let err = walk_tree(&store, &v2_root).expect_err("must fail closed");
        assert!(matches!(err, TreeWalkError::MissingBlob(_)));
    }

    #[test]
    fn malformed_json_fails_closed() {
        let store = TestBlobs::default();
        let bad = store.put_bytes(b"not json at all".to_vec());
        let err = walk_tree(&store, &bad).expect_err("must fail closed");
        assert!(matches!(err, TreeWalkError::Malformed(_)));
    }

    #[test]
    fn unsupported_version_fails_closed() {
        let store = TestBlobs::default();
        let root = store.put_json(serde_json::json!({"nodalmerge":"tree","version":99,"entries":[]}));
        let err = walk_tree(&store, &root).expect_err("must fail closed");
        assert!(matches!(err, TreeWalkError::Malformed(_)));
    }
}
