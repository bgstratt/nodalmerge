//! F7 — live MongoDB integration test. Spins up `mongo:7` via
//! testcontainers, then runs the full conformance suite.
//! Skips gracefully if Docker is unavailable.

use nodalmerge_mongo_store::{MongoNodeStore, MongoNodeStoreConfig};
use testcontainers::{
    core::{IntoContainerPort, WaitFor},
    runners::SyncRunner,
    GenericImage,
};

const DB_NAME: &str = "nodalmerge_test";

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

#[test]
fn mongo_passes_full_conformance() {
    let _ = tracing_subscriber::fmt::try_init();
    // Plain #[test] + explicit runtime: testcontainers' SyncRunner blocks,
    // which panics inside a #[tokio::test] runtime.
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let Some(container) = start_mongo() else {
        // Graceful skip is correct for a dev laptop that may not have Docker
        // running — but the exact same `eprintln!` + `return` reports as a
        // PASS to the test harness. In CI that is catastrophic: if the
        // Docker/testcontainers setup ever breaks, this job goes green while
        // testing nothing, silently losing all coverage of `MongoNodeStore`
        // — including the safety-critical
        // `known_room_ids`/`can_enumerate_rooms` enumeration the blob GC
        // sweep trusts completely (blob-cas-remediation.md slice 1.1,
        // finding #1; see the CI follow-up notes and the new
        // `known_room_ids_is_superset_of_written_rooms` conformance
        // scenario). CI sets `NODALMERGE_REQUIRE_DOCKER=1` to convert this
        // skip into a hard failure; local/dev runs without the var keep
        // skipping gracefully.
        if std::env::var("NODALMERGE_REQUIRE_DOCKER").as_deref() == Ok("1") {
            panic!(
                "NODALMERGE_REQUIRE_DOCKER=1 but Docker / Mongo container is \
                 unavailable — this suite must not silently pass in CI"
            );
        }
        eprintln!("skipping: Docker / Mongo container unavailable");
        return;
    };
    let port = container
        .get_host_port_ipv4(27017)
        .expect("host port for 27017");
    let uri = format!("mongodb://127.0.0.1:{port}");

    let mut store = None;
    for attempt in 0..10 {
        match rt.block_on(MongoNodeStore::connect(MongoNodeStoreConfig::new(&uri, DB_NAME))) {
            Ok(s) => { store = Some(s); break; }
            Err(e) => {
                eprintln!("connect attempt {attempt} failed: {e}");
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    }
    let store = store.expect("mongo connect");
    nodalmerge_nodestore_conformance::run_all(&store, "mongo-conformance");

    // S5 cross-runtime interop: hydrate documents written the way the .NET
    // MongoNodeStoreProvider writes them — ObjectId _id (legacy upsert path),
    // node_id_hex = "pack:<sha256>", payload = a multi-node postcard pack —
    // and assert this store returns every node inside the pack.
    {
        use ed25519_dalek::SigningKey;
        use mongodb::bson::{doc, Binary, DateTime as BsonDateTime};
        use nodalmerge_core::{pack_nodes, MapOp, Op, StateGraph};
        use nodalmerge_server::store::NodePersistence;

        let room = "dotnet-interop-room";
        let sk = SigningKey::from_bytes(&[0x51u8; 32]);
        let mut g = StateGraph::new();
        let mut nodes = Vec::new();
        for (k, v) in [("world/a", "1"), ("world/b", "2")] {
            let id = g
                .apply_local(&sk, 0, vec![Op::Map(MapOp::Set {
                    key: k.to_string(),
                    value: v.as_bytes().to_vec(),
                })])
                .expect("apply_local");
            nodes.push(g.get_nodes(&[id]).into_iter().next().unwrap().clone());
        }
        let pack = pack_nodes(&[&nodes[0], &nodes[1]]);

        let client = rt.block_on(mongodb::Client::with_uri_str(&uri)).expect("client");
        let coll = client
            .database(DB_NAME)
            .collection::<mongodb::bson::Document>("accepted_nodes");
        rt.block_on(async {
            coll.insert_one(doc! {
                // No explicit _id: Mongo assigns an ObjectId, like legacy .NET docs.
                "room_id": room,
                "node_id_hex": "pack:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "payload": Binary { subtype: mongodb::bson::spec::BinarySubtype::Generic, bytes: pack },
                "payload_kind": "pack",
                "causal_parent_node_ids": Vec::<String>::new(),
                "frontier_hash_hex": mongodb::bson::Bson::Null,
                "applied": false,
                "is_tombstone": false,
                "accepted_at_utc": BsonDateTime::now(),
                "eligible_for_compaction_at_utc": mongodb::bson::Bson::Null,
                "updated_at_utc": BsonDateTime::now(),
            })
            .await
        })
        .expect("insert .NET-shaped doc");

        let loaded = store.load_room_nodes(room);
        assert_eq!(
            loaded.len(),
            2,
            ".NET-shaped multi-node pack doc must hydrate every contained node"
        );
        let loaded_ids: Vec<String> = loaded.iter().map(|n| n.id.to_hex()).collect();
        assert!(loaded_ids.contains(&nodes[0].id.to_hex()));
        assert!(loaded_ids.contains(&nodes[1].id.to_hex()));

        // And the reverse-direction shape check: a doc written by this store
        // must carry every canonical field the .NET reader consumes.
        store.persist_node(room, &nodes[0]);
        let rust_doc = rt
            .block_on(async {
                coll.find_one(doc! { "room_id": room, "node_id_hex": nodes[0].id.to_hex() })
                    .await
            })
            .expect("find")
            .expect("rust-written doc present");
        assert_eq!(rust_doc.get_str("payload_kind").unwrap(), "pack");
        assert!(rust_doc.get_binary_generic("payload").is_ok());
        assert!(rust_doc.get_datetime("accepted_at_utc").is_ok());
        assert!(rust_doc.get_datetime("updated_at_utc").is_ok());
        assert_eq!(
            rust_doc.get_str("_id").unwrap(),
            format!("{room}:{}", nodes[0].id.to_hex()),
            "fresh Rust-written docs use the deterministic compound _id"
        );

        // Idempotency: re-persisting the same node must not error or change
        // accepted_at_utc.
        let accepted_at = *rust_doc.get_datetime("accepted_at_utc").unwrap();
        store.persist_node(room, &nodes[0]);
        let rewritten = rt
            .block_on(async {
                coll.find_one(doc! { "room_id": room, "node_id_hex": nodes[0].id.to_hex() })
                    .await
            })
            .expect("find")
            .expect("doc still present");
        assert_eq!(*rewritten.get_datetime("accepted_at_utc").unwrap(), accepted_at);
    }

    drop(container);
}
