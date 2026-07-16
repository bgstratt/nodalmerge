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
        // Graceful skip is correct for a dev laptop that may not have Docker
        // running — but the exact same `eprintln!` + `return` reports as a
        // PASS to the test harness. In CI that is catastrophic: if the
        // Docker/testcontainers setup ever breaks, this job goes green while
        // testing nothing, silently losing all coverage of
        // `PostgresNodeStore` — including the safety-critical
        // `known_room_ids`/`can_enumerate_rooms` enumeration the blob GC
        // sweep trusts completely (blob-cas-remediation.md slice 1.1,
        // finding #1; see the CI follow-up notes and the new
        // `known_room_ids_is_superset_of_written_rooms` conformance
        // scenario). CI sets `NODALMERGE_REQUIRE_DOCKER=1` to convert this
        // skip into a hard failure; local/dev runs without the var keep
        // skipping gracefully.
        if std::env::var("NODALMERGE_REQUIRE_DOCKER").as_deref() == Ok("1") {
            panic!(
                "NODALMERGE_REQUIRE_DOCKER=1 but Docker / Postgres container is \
                 unavailable — this suite must not silently pass in CI"
            );
        }
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
