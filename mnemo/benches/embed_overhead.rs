//! Phase 6 speed bench: engine overhead on the text path.
//!
//! The planner's dispatch asks for "text-path engine overhead with a
//! trivial in-process embedder". The point is to isolate the mnemo
//! engine's cost on `remember_text` from the embedder's own compute
//! cost — a real embedder (sentence-transformers, OpenAI) dominates
//! the wall time, but when someone brings their own fast in-process
//! model (fastembed in PR 10c, say) the engine's share becomes
//! visible.
//!
//! Two bench entries:
//!
//! 1. [`remember_text_in_process_embedder`] — `remember_text` with an
//!    in-process `Embedder` impl that returns a canned 384-dim vector
//!    in <1 µs. The measured time minus the embedder time is the
//!    engine's text-path overhead on top of a raw `remember`.
//! 2. [`remember_raw_vector`] — baseline: `remember(Memory::new(...))`
//!    with the same canned vector, bypassing the text path entirely.
//!
//! Difference between the two is what calling `remember_text` adds:
//! an `Arc<dyn Embedder>` dereference, a one-element `&[&str]` embed
//! call, and the vector allocation the embedder returns.
//!
//! No fsync in either path — both batch 1 insert and flush once per
//! iteration, so the fsync floor doesn't dominate.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use mnemo::{Embedder, EmbedderIdentity, Memory, MemoryType, Mnemo, Result};
use tempfile::TempDir;

#[path = "corpus.rs"]
mod corpus;
use corpus::{fast_kdf_cfg, gen_clustered_vectors, DIMENSIONS, PASSPHRASE};

/// Trivial `Embedder` that returns a canned normalized vector no
/// matter what text comes in. Clones a cached `Vec<f32>` per call so
/// the allocation cost shows up in the measurement — real embedders
/// also allocate a fresh vector per text.
struct CannedEmbedder {
    identity: EmbedderIdentity,
    vector: Vec<f32>,
}

impl CannedEmbedder {
    fn new(vector: Vec<f32>) -> Self {
        Self {
            identity: EmbedderIdentity {
                name: "bench-canned-embedder".into(),
                dimensions: vector.len(),
                normalize: true,
            },
            vector,
        }
    }
}

impl Embedder for CannedEmbedder {
    fn identity(&self) -> EmbedderIdentity {
        self.identity.clone()
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        // One vector per input, same canned payload. Matches the
        // real-embedder contract (`Vec<Vec<f32>>`, same length as
        // `texts`), so the engine's downstream code path is exercised
        // honestly.
        Ok(texts.iter().map(|_| self.vector.clone()).collect())
    }
}

fn make_db_with_embedder(td: &TempDir, vector: Vec<f32>) -> Mnemo {
    let path = td.path().join("embed-overhead.mnemo");
    let mut db = Mnemo::create(&path, PASSPHRASE, fast_kdf_cfg(DIMENSIONS)).expect("create");
    db.set_embedder(CannedEmbedder::new(vector))
        .expect("set_embedder");
    db
}

fn make_db_plain(td: &TempDir) -> Mnemo {
    let path = td.path().join("embed-overhead.mnemo");
    Mnemo::create(&path, PASSPHRASE, fast_kdf_cfg(DIMENSIONS)).expect("create")
}

fn bench_remember_text_in_process_embedder(c: &mut Criterion) {
    // Pre-generate one canned vector (unit-normalized clustered output);
    // the embedder closes over it, cloning per call.
    let vector = gen_clustered_vectors(1, DIMENSIONS, 1, 0xE11B_0666)
        .into_iter()
        .next()
        .unwrap();

    let mut group = c.benchmark_group("embed_overhead");
    group.sample_size(50);
    group.throughput(criterion::Throughput::Elements(1));
    group.bench_function("remember_text_in_process_embedder", |b| {
        b.iter_batched(
            || {
                let td = TempDir::new().expect("tempdir");
                let db = make_db_with_embedder(&td, vector.clone());
                (td, db)
            },
            |(_td, mut db)| {
                let id = db
                    .remember_text("bench text for embedder overhead", MemoryType::Semantic)
                    .expect("remember_text");
                db.flush().expect("flush");
                black_box(id);
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn bench_remember_raw_vector(c: &mut Criterion) {
    let vector = gen_clustered_vectors(1, DIMENSIONS, 1, 0xE11B_0666)
        .into_iter()
        .next()
        .unwrap();

    let mut group = c.benchmark_group("embed_overhead");
    group.sample_size(50);
    group.throughput(criterion::Throughput::Elements(1));
    group.bench_function("remember_raw_vector", |b| {
        b.iter_batched(
            || {
                let td = TempDir::new().expect("tempdir");
                let db = make_db_plain(&td);
                (td, db)
            },
            |(_td, mut db)| {
                let m = Memory::new(
                    "bench text for embedder overhead".to_string(),
                    MemoryType::Semantic,
                    vector.clone(),
                );
                let id = db.remember(m).expect("remember");
                db.flush().expect("flush");
                black_box(id);
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(
    embed_overhead_benches,
    bench_remember_text_in_process_embedder,
    bench_remember_raw_vector,
);
criterion_main!(embed_overhead_benches);
