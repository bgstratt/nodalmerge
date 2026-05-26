/// Benchmark: bytes required to transmit 1,000 missing nodes during a sync
/// handshake.
///
/// Target: <5KB total wire size for the pack containing 1,000 missing nodes.
///
/// Scenario:
///   - Peer A has 2,000 nodes.
///   - Peer B has the first 1,000 of those nodes.
///   - Peer A sends only the 1,000 missing nodes.
///   - We measure the byte length of `pack_nodes` output (postcard binary).
///
/// B1 IBF benchmark:
///   - Peer B sends an IBF of its 1,000 node IDs (~3.2KB, constant size).
///   - Peer A XORs its 2,000-node IBF and decodes the 1,000-element diff.
///   - We measure IBF encode time and wire size.
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ed25519_dalek::SigningKey;
use nodalmerge_core::{Ibf, MerkleSearchTree, Op, MapOp, StateGraph, pack_nodes};

fn build_split_graphs() -> (Vec<nodalmerge_core::SyncNode>, Vec<nodalmerge_core::NodeId>, Vec<nodalmerge_core::NodeId>) {
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let mut graph = StateGraph::new();
    let mut all_ids = Vec::with_capacity(2_000);
    let mut first_1k_ids = Vec::with_capacity(1_000);

    for i in 0..2_000usize {
        let op = Op::Map(MapOp::Set {
            key:   format!("key{i:04}"),
            value: format!("val{i}").into_bytes(),
        });
        let id = graph.apply_local(&key, i as u64, vec![op]).unwrap();
        all_ids.push(id);
        if i < 1_000 { first_1k_ids.push(id); }
    }

    let all_nodes: Vec<nodalmerge_core::SyncNode> = graph.get_nodes(&all_ids).into_iter().cloned().collect();
    (all_nodes, all_ids, first_1k_ids)
}

fn bench_sync_handshake(c: &mut Criterion) {
    let (all_nodes, all_ids, first_1k_ids) = build_split_graphs();
    let known_set: std::collections::HashSet<_> = first_1k_ids.iter().copied().collect();

    // Pre-collect the missing nodes (what Peer A would send Peer B).
    let missing: Vec<&nodalmerge_core::SyncNode> = all_nodes
        .iter()
        .filter(|n| !known_set.contains(&n.id))
        .collect();

    // ---- pack benchmark (unchanged from before) ----
    c.bench_function("sync_handshake_pack_1k_missing", |b| {
        b.iter(|| {
            let bytes = pack_nodes(black_box(&missing));
            black_box(bytes.len())
        });
    });

    // ---- IBF encode benchmark (B1) ----
    // Simulates what Peer B sends in hello: encode 1k IDs into an IBF.
    c.bench_function("sync_handshake_ibf_encode_1k", |b| {
        b.iter(|| {
            let ibf = Ibf::from_ids(black_box(&first_1k_ids));
            black_box(ibf.encode().len())
        });
    });

    // ---- IBF decode benchmark (B1) ----
    // Simulates server-side: XOR server IBF with client IBF and decode diff.
    c.bench_function("sync_handshake_ibf_decode_1k_diff", |b| {
        let client_ibf = Ibf::from_ids(&first_1k_ids);
        let server_ibf = Ibf::from_ids(&all_ids);
        b.iter(|| {
            let mut diff = black_box(server_ibf.clone());
            diff.subtract(black_box(&client_ibf));
            black_box(diff.decode())
        });
    });

    // Print size summary once for CI inspection.
    let pack_bytes = pack_nodes(&missing);
    let ibf_bytes  = Ibf::from_ids(&first_1k_ids).encode();

    // ---- MST benchmarks (B2) ----
    // Simulate the multi-round MST sync protocol between two peers:
    //   Peer B has 1,000 nodes, Peer A has 2,000 (1,000 extras).
    // We measure:
    //   (a) Build time for a 1,000-node MST.
    //   (b) Build time for a 2,000-node MST.
    //   (c) Simulated sync: round trips + bytes for 1,000-element diff.
    let mst_client = MerkleSearchTree::from_ids(&first_1k_ids);
    let mst_server = MerkleSearchTree::from_ids(&all_ids);

    c.bench_function("sync_handshake_mst_build_1k", |b| {
        b.iter(|| {
            let mst = MerkleSearchTree::from_ids(black_box(&first_1k_ids));
            black_box(mst.root_hash_hex())
        });
    });

    c.bench_function("sync_handshake_mst_build_2k", |b| {
        b.iter(|| {
            let mst = MerkleSearchTree::from_ids(black_box(&all_ids));
            black_box(mst.root_hash_hex())
        });
    });

    c.bench_function("sync_handshake_mst_simulate_1k_diff", |b| {
        b.iter(|| {
            let sim = black_box(&mst_client).simulate_sync(black_box(&mst_server));
            black_box((sim.round_trips, sim.bytes_sent))
        });
    });

    // Simulate and report stats.
    let sim = mst_client.simulate_sync(&mst_server);
    println!(
        "\nsync_handshake: pack {} missing → {} bytes ({:.1} KB)  |  IBF hello → {} bytes ({:.1} KB)\n\
         MST sync (1k diff in 2k graph): {} round trips, {} bytes ({:.1} KB) wire overhead  |  diff: {} mine, {} theirs",
        missing.len(),
        pack_bytes.len(), pack_bytes.len() as f64 / 1024.0,
        ibf_bytes.len(),  ibf_bytes.len()  as f64 / 1024.0,
        sim.round_trips, sim.bytes_sent, sim.bytes_sent as f64 / 1024.0,
        sim.only_mine.len(), sim.only_theirs.len(),
    );
}

criterion_group!(benches, bench_sync_handshake);
criterion_main!(benches);

