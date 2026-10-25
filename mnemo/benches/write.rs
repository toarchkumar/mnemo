//! Phase 6 speed bench: `remember` + `flush` throughput.
//!
//! Two shapes measured:
//!
//! 1. [`remember_batch_100_then_flush`] — insert 100 memories, then flush
//!    once. This is what a real import or bulk-seed looks like, and
//!    amortises the single-fsync cost over many writes.
//! 2. [`remember_1_then_flush`] — insert one memory and flush. The
//!    worst case for a single interactive write (one small record + one
//!    fsync); the lower bound on how fast an agent can commit a single
//!    thought.
//!
//! Both use `KdfParams::fast()` via `fast_kdf_cfg` so Argon2id doesn't
//! swamp the signal we care about (page encryption + WAL + fsync).
//! Each iteration starts from a *fresh* file (`iter_batched` with
//! `BatchSize::SmallInput` + `tempfile::TempDir`) — otherwise the
//! bench would be measuring the catalog growing over N iterations
//! rather than the cost of one write.
//!
//! Numbers to look for on the M1 Pro reference machine:
//! - batch-of-100 should hit ~thousands of memories/sec in release.
//! - single-write-plus-flush is bounded by fsync — expect something
//!   in the 1–5 ms range per iteration on a laptop SSD.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use mnemo::{Memory, MemoryType, Mnemo};
use tempfile::TempDir;

#[path = "corpus.rs"]
mod corpus;
use corpus::{fast_kdf_cfg, gen_clustered_vectors, DIMENSIONS, PASSPHRASE};

fn bench_remember_batch_100_then_flush(c: &mut Criterion) {
    let mut group = c.benchmark_group("write");
    // 100 inserts + 1 flush is slow enough that criterion's default
    // 100 samples would run >10s per bench on cold hardware; the
    // explicit sample_size keeps bench total wall time ~20s.
    group.sample_size(50);
    group.throughput(criterion::Throughput::Elements(100));
    group.bench_function("remember_batch_100_then_flush", |b| {
        // Pre-generate the vectors once — their cost isn't part of
        // what we're measuring (an agent's embedder produces them).
        let vectors: Vec<Vec<f32>> = gen_clustered_vectors(100, DIMENSIONS, 16, 0xC001_BA75);
        b.iter_batched(
            || {
                // One fresh file per iteration: TempDir is dropped at
                // the end of the iteration and cleans itself up.
                let td = TempDir::new().expect("tempdir");
                let path = td.path().join("bench.mnemo");
                let db =
                    Mnemo::create(&path, PASSPHRASE, fast_kdf_cfg(DIMENSIONS)).expect("create");
                (td, db)
            },
            |(_td, mut db)| {
                for (i, v) in vectors.iter().enumerate() {
                    let content = format!("bench memory #{i}");
                    let m = Memory::new(content, MemoryType::Semantic, v.clone());
                    let _ = black_box(db.remember(m).expect("remember"));
                }
                db.flush().expect("flush");
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn bench_remember_1_then_flush(c: &mut Criterion) {
    let mut group = c.benchmark_group("write");
    group.sample_size(50);
    group.throughput(criterion::Throughput::Elements(1));
    group.bench_function("remember_1_then_flush", |b| {
        let vector = gen_clustered_vectors(1, DIMENSIONS, 1, 0xC001_BA76)
            .into_iter()
            .next()
            .unwrap();
        b.iter_batched(
            || {
                let td = TempDir::new().expect("tempdir");
                let path = td.path().join("bench.mnemo");
                let db =
                    Mnemo::create(&path, PASSPHRASE, fast_kdf_cfg(DIMENSIONS)).expect("create");
                (td, db)
            },
            |(_td, mut db)| {
                let m = Memory::new(
                    "single bench memory".to_string(),
                    MemoryType::Semantic,
                    vector.clone(),
                );
                let _ = black_box(db.remember(m).expect("remember"));
                db.flush().expect("flush");
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(
    write_benches,
    bench_remember_batch_100_then_flush,
    bench_remember_1_then_flush,
);
criterion_main!(write_benches);
