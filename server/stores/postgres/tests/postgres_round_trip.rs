//! F7 — live Postgres integration test. Spins up `postgres:16` via
//! testcontainers, runs migrations, then runs the full conformance suite.
//! Skips gracefully if Docker is unavailable.

use nodalmerge_postgres_store::{PostgresNodeStore, PostgresNodeStoreConfig};
use testcontainers::{
    core::{IntoContainerPort, WaitFor},
    runners::SyncRunner,
    GenericImage, ImageExt,
};

const PG_USER: &str = "nodalmerge";
const PG_PASS: &str = "nodalmerge";
const PG_DB: &str = "nodalmerge_test";

fn start_postgres() -> Option<testcontainers::Container<GenericImage>> {
    let res = GenericImage::new("postgres", "16")
        .with_exposed_port(5432.tcp())
        .with_wait_for(WaitFor::message_on_stderr(
            "database system is ready to accept connections",
        ))
        .with_env_var("POSTGRES_USER", PG_USER)
        .with_env_var("POSTGRES_PASSWORD", PG_PASS)
        .with_env_var("POSTGRES_DB", PG_DB)
        .start();
    match res {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("Postgres container start failed: {e:?}");
            None
        }
    }
}

#[test]
fn postgres_passes_full_conformance() {
    let _ = tracing_subscriber::fmt::try_init();
    let Some(container) = start_postgres() else {
        eprintln!("skipping: Docker / Postgres container unavailable");
        return;
    };
    let port = container
        .get_host_port_ipv4(5432)
        .expect("host port for 5432");
    // Postgres takes a beat after the "ready" line before accepting auth;
    // retry a few times.
    let uri = format!("postgres://{PG_USER}:{PG_PASS}@127.0.0.1:{port}/{PG_DB}");

    let mut store = None;
    for attempt in 0..10 {
        match PostgresNodeStore::connect_and_migrate(PostgresNodeStoreConfig::new(&uri)) {
            Ok(s) => { store = Some(s); break; }
            Err(e) => {
                eprintln!("connect attempt {attempt} failed: {e}");
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    }
    let store = store.expect("postgres connect");
    nodalmerge_nodestore_conformance::run_all(&store, "pg-conformance");
    drop(container);
}
