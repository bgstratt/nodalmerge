/// Benchmark: Blake3 integrity verification of a 50 KB blob.
///
/// Target: <0.5ms per verify.
///
/// This is the hot path when a peer receives a blob from the server or another
/// peer and must verify it before storing. The blob's hash was transmitted
/// out-of-band (in a `SetBlob` op) and we confirm the received bytes match.
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use activesync_core::Hash;

const BLOB_SIZE: usize = 50 * 1024; // 50 KB

fn bench_blob_verify(c: &mut Criterion) {
    // Create a 50 KB blob with a pattern so it isn't optimised away.
    let blob: Vec<u8> = (0..BLOB_SIZE).map(|i| (i & 0xff) as u8).collect();
    let expected_hash = Hash::of(&blob);

    c.bench_function("blob_verify_50kb", |b| {
        b.iter(|| {
            let actual = Hash::of(black_box(&blob));
            black_box(actual == expected_hash)
        });
    });
}

criterion_group!(benches, bench_blob_verify);
criterion_main!(benches);
