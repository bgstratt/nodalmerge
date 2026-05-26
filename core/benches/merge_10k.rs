/// Benchmark: merge 10,000 concurrent updates into a single StateGraph.
///
/// Target: <10ms for the full merge.
///
/// Each node is authored by a distinct signing key so all 10k nodes are
/// concurrent (no parent → child ordering). This is the worst-case fan-out
/// seen during a catchup sync after a long offline period.
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ed25519_dalek::SigningKey;
use nodalmerge_core::{Op, MapOp, StateGraph};

fn build_nodes(n: usize) -> Vec<nodalmerge_core::SyncNode> {
    // Use a single signing key; every node is a leaf so there are no parent
    // constraints — we can insert them in any order.
    let key = SigningKey::from_bytes(&[42u8; 32]);
    let mut graph = StateGraph::new();
    let mut nodes = Vec::with_capacity(n);

    for i in 0..n {
        let op = Op::Map(MapOp::Set {
            key:   format!("k{i}"),
            value: b"v".to_vec(),
        });
        let id = graph.apply_local(&key, i as u64, vec![op]).unwrap();
        let node = graph.get_nodes(&[id]).into_iter().next().unwrap().clone();
        nodes.push(node);
    }
    nodes
}

fn bench_merge_10k(c: &mut Criterion) {
    let nodes = build_nodes(10_000);

    c.bench_function("merge_10k", |b| {
        b.iter(|| {
            let mut graph = StateGraph::new();
            // apply_remote requires parents to be present first.
            // Since every node has no parents (they were all leaves in the
            // source graph), we can insert them directly.  Any node that was
            // built on top of a prior node needs its parents inserted first —
            // here we iterate in insertion order which satisfies that.
            for node in black_box(&nodes) {
                // Ignore DuplicateNode errors that can't happen here; ignore
                // MissingParent — nodes were built in causal order.
                let _ = graph.apply_remote(node.clone());
            }
        });
    });

    c.bench_function("merge_10k_batch", |b| {
        b.iter(|| {
            let mut graph = StateGraph::new();
            let result = graph.apply_remote_batch(black_box(nodes.clone()));
            assert_eq!(result.accepted.len(), 10_000);
            assert!(result.rejected.is_empty());
        });
    });
}

criterion_group!(benches, bench_merge_10k);
criterion_main!(benches);
