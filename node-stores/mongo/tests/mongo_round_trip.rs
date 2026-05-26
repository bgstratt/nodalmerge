//! F7 — live MongoDB integration test. Spins up `mongo:7` via
//! testcontainers, then runs the full conformance suite.
//! Skips gracefully if Docker is unavailable.

use nodalmerge_mongo_store::{MongoNodeStore, MongoNodeStoreConfig};
use testcontainers::{
    core::{IntoContainerPort, WaitFor},
    runners::SyncRunner,
    GenericImage,
};

const DB_NAME: &str = "activesync_test";

fn start_mongo() -> Option<testcontainers::Container<GenericImage>> {
    let res = GenericImage::new("mongo", "7")
        .with_exposed_port(27017.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Waiting for connections"))
        .start();
    match res {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("Mongo container start failed: {e:?}");
            None
        }
    }
}

#[tokio::test]
async fn mongo_passes_full_conformance() {
    let _ = tracing_subscriber::fmt::try_init();
    let Some(container) = start_mongo() else {
        eprintln!("skipping: Docker / Mongo container unavailable");
        return;
    };
    let port = container
        .get_host_port_ipv4(27017)
        .expect("host port for 27017");
    let uri = format!("mongodb://127.0.0.1:{port}");

    let mut store = None;
    for attempt in 0..10 {
        match MongoNodeStore::connect(MongoNodeStoreConfig::new(&uri, DB_NAME)).await {
            Ok(s) => { store = Some(s); break; }
            Err(e) => {
                eprintln!("connect attempt {attempt} failed: {e}");
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    }
    let store = store.expect("mongo connect");
    nodalmerge_nodestore_conformance::run_all(&store, "mongo-conformance");
    drop(container);
}
