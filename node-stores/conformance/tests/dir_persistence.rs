//! Run the conformance suite against the in-tree `DirPersistence` to prove
//! the suite itself is correct. Any new `NodePersistence` adapter must
//! pass the same scenarios.

use std::sync::Arc;

use activesync_server::store::DirPersistence;

#[test]
fn dir_persistence_passes_full_conformance() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(DirPersistence::open(dir.path()).expect("open"));
    activesync_nodestore_conformance::run_all(&*store, "dir-conformance");
}
