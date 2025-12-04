// Benchmark for hash algorithm throughput.
// Run with: cargo bench

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256};

fn bench_hashing(c: &mut Criterion) {
    // Test sizes: 1KB, 64KB, 1MB, 5MB (default block size)
    let sizes: [(usize, &str); 4] = [
        (1024, "1KB"),
        (64 * 1024, "64KB"),
        (1024 * 1024, "1MB"),
        (5 * 1024 * 1024, "5MB"),
    ];

    let mut group = c.benchmark_group("hash_throughput");

    for (size, label) in sizes {
        let data = vec![0u8; size];

        group.throughput(Throughput::Bytes(size as u64));

        group.bench_with_input(BenchmarkId::new("md5", label), &data, |b, data| {
            b.iter(|| {
                let mut hasher = Md5::new();
                hasher.update(data);
                hasher.finalize()
            })
        });

        group.bench_with_input(BenchmarkId::new("sha1", label), &data, |b, data| {
            b.iter(|| {
                let mut hasher = Sha1::new();
                hasher.update(data);
                hasher.finalize()
            })
        });

        group.bench_with_input(BenchmarkId::new("sha256", label), &data, |b, data| {
            b.iter(|| {
                let mut hasher = Sha256::new();
                hasher.update(data);
                hasher.finalize()
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_hashing);
criterion_main!(benches);
