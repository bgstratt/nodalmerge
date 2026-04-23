/// Benchmark: LWW resolution on a 1,000-key map.
///
/// Target: <1ms to resolve the full state.
///
/// Scenario: 1,000 distinct keys each written once, then `resolve()` is called.
/// This measures the map-scan cost on the hot path of every UI render.
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ed25519_dalek::SigningKey;
use activesync_core::{Op, MapOp, StateGraph};

fn build_1k_graph() -> StateGraph {
    let key = SigningKey::from_bytes(&[1u8; 32]);
    let mut graph = StateGraph::new();
    for i in 0..1_000usize {
        let op = Op::Map(MapOp::Set {
            key:   format!("key{i:04}"),
            value: format!("value{i}").into_bytes(),
        });
        graph.apply_local(&key, i as u64, vec![op]).unwrap();
    }
    graph
}

fn bench_resolve_1k(c: &mut Criterion) {
    let graph = build_1k_graph();

    c.bench_function("resolve_1k", |b| {
        b.iter(|| {
            let state = black_box(&graph).resolve();
            black_box(state.len())
        });
    });
}

criterion_group!(benches, bench_resolve_1k);
criterion_main!(benches);
