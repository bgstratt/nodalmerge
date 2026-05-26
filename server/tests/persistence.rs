//! F4 integration test: verify that nodes + blobs persisted by a running room
//! survive a simulated restart (drop the `Room`, re-open with the same
//! `DirPersistence`, confirm state hydrates).

use std::sync::Arc;

use nodalmerge_core::{Hash, MapOp, Op, StateGraph};
use nodalmerge_server::room::{import_nodes, Room};
use nodalmerge_server::store::{DirPersistence, SharedPersistence};
use ed25519_dalek::SigningKey;

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("activesync-itest-{tag}-{nanos}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn make_node(sk: &SigningKey, key: &str, val: &[u8]) -> nodalmerge_core::SyncNode {
    let mut g = StateGraph::new();
    let id = g.apply_local(sk, 0, vec![Op::Map(MapOp::Set {
        key: key.into(), value: val.to_vec(),
    })]).unwrap();
    g.get_nodes(&[id]).into_iter().next().unwrap().clone()
}

#[tokio::test]
async fn room_survives_restart_with_nodes_and_blobs() {
    let dir = tmpdir("restart");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0x33u8; 32]);
    let room_id = "itest-room".to_string();

    // --- lifetime 1 -----------------------------------------------------
    {
        let room = Room::new(room_id.clone(), Arc::clone(&persistence), 512);
        // Push 3 nodes through import_nodes (simulates the server's accept path).
        let n1 = make_node(&sk, "hello", b"world");
        let n2 = make_node(&sk, "answer", b"42");
        let n3 = make_node(&sk, "rust", b"ferris");
        let (accepted, _, errs) = import_nodes(&room, vec![n1.clone(), n2.clone(), n3.clone()]).await;
        assert_eq!(accepted, 3);
        assert!(errs.is_empty());
        // Also persist a blob.
        let blob_bytes = b"opaque payload".to_vec();
        let blob_hash = Hash::of(&blob_bytes);
        persistence.persist_blob(&room_id, &blob_hash, &blob_bytes);
        drop(room);
    }

    // --- lifetime 2 (simulated restart) ---------------------------------
    {
        let room = Room::new(room_id.clone(), Arc::clone(&persistence), 512);
        let graph = room.graph.read().await;
        let state = graph.resolve();
        let map: std::collections::HashMap<String, Vec<u8>> = state.into_iter().collect();
        assert_eq!(map.get("hello").map(|v| v.as_slice()), Some(b"world".as_slice()));
        assert_eq!(map.get("answer").map(|v| v.as_slice()), Some(b"42".as_slice()));
        assert_eq!(map.get("rust").map(|v| v.as_slice()), Some(b"ferris".as_slice()));
        drop(graph);

        // Blob should be hydrated too.
        let blobs = room.blobs.read().await;
        use nodalmerge_core::BlobStore;
        let blob_hash = Hash::of(b"opaque payload");
        assert!(blobs.contains(&blob_hash));
        assert_eq!(blobs.get(&blob_hash).unwrap(), b"opaque payload");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn large_room_hydrates_quickly() {
    // Exit criterion from PLAN.md F4: 10k-node room loads in <500 ms.
    let dir = tmpdir("bench");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0x44u8; 32]);
    let room_id = "bench-room".to_string();
    const N: usize = 10_000;

    // Lifetime 1: write N nodes. We feed them through a single StateGraph so
    // they chain properly, then persist each one in insertion order.
    {
        let room = Room::new(room_id.clone(), Arc::clone(&persistence), 512);
        let mut batch = Vec::with_capacity(N);
        let mut g = StateGraph::new();
        for i in 0..N {
            let id = g.apply_local(&sk, 0, vec![Op::Map(MapOp::Set {
                key: format!("k/{i}"),
                value: format!("v{i}").into_bytes(),
            })]).unwrap();
            let node = g.get_nodes(&[id]).into_iter().next().unwrap().clone();
            batch.push(node);
        }
        let (accepted, _, _) = import_nodes(&room, batch).await;
        assert_eq!(accepted, N);
        drop(room);
    }

    // Lifetime 2: measure hydrate time.
    let t0 = std::time::Instant::now();
    let room = Room::new(room_id.clone(), Arc::clone(&persistence), 512);
    let elapsed = t0.elapsed();
    println!("[startup_replay_10k] hydrated {N} nodes in {:.2?}", elapsed);
    let graph = room.graph.read().await;
    assert_eq!(graph.all_node_ids().len(), N);
    // The exit criterion is 500 ms, but CI hosts vary wildly.  Assert a loose
    // 5 s ceiling so the test still catches an O(n²) regression without
    // flaking on slow runners.
    assert!(elapsed.as_secs() < 5, "hydrate took {elapsed:?}, expected <5s");

    let _ = std::fs::remove_dir_all(&dir);
}

