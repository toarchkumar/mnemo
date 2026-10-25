//! Phase 6 speed bench: `recall` — exact scan vs IVF+PQ, at the 10K
//! tier for now. Follow-up commits extend to 100K and 1M once the
//! harness is proven and the maintainer confirms there's disk for it.
//!
//! Two configurations, same corpus, same queries:
//!
//! 1. [`exact_scan_10k`] — recall over a corpus with no ANN index
//!    built. This is the ground truth and the fallback mnemo uses when
//!    the index is absent or disabled.
//! 2. [`ivfpq_recall_10k`] — recall over the same corpus with
//!    `IndexConfig::default()` applied (n_probe=8, n_rerank=64, auto
//!    partition count = ~sqrt(n) = 100).
//!
//! Query vectors are a separately-seeded clustered batch so no query
//! is identical to any corpus vector; this measures nearest-neighbour
//! work rather than hash-key lookup.
//!
//! `track_access=false` on the recall request so repeated iterations
//! don't mutate the catalog — otherwise a single bench run would
//! bump every top-k memory's `access_count`, which would be fine for
//! correctness but muddies the per-iteration-cost measurement.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use mnemo::RecallRequest;

#[path = "corpus.rs"]
mod corpus;
use corpus::{
    gen_clustered_vectors, open_corpus_exact_only, open_corpus_with_index, setup_cached_corpus,
    DIMENSIONS,
};

const CORPUS_SIZE: usize = 10_000;
const QUERY_COUNT: usize = 20;
const TOP_K: usize = 10;

fn bench_exact_scan_10k(c: &mut Criterion) {
    let cached = setup_cached_corpus(CORPUS_SIZE, DIMENSIONS);
    // Open an in-tempdir copy with the in-memory index dropped — see
    // `open_corpus_exact_only` for why we don't just drop_index on the
    // cached file. _td is held so the TempDir doesn't delete the copy
    // mid-bench.
    let (_td, mut db) = open_corpus_exact_only(&cached);
    // 20 pre-generated query vectors, cycled through so each iter is
    // one query. The vectors share the corpus's cluster structure but
    // use a different seed so no query is a corpus duplicate.
    let queries: Vec<Vec<f32>> = gen_clustered_vectors(QUERY_COUNT, DIMENSIONS, 100, 0xDEAD_BEEF);

    let mut group = c.benchmark_group("recall");
    group.sample_size(50);
    group.throughput(criterion::Throughput::Elements(1));
    let mut idx = 0usize;
    group.bench_function("exact_scan_10k", |b| {
        b.iter(|| {
            let q = &queries[idx % QUERY_COUNT];
            idx += 1;
            let req = RecallRequest::new(q.clone())
                .top_k(TOP_K)
                .track_access(false);
            let hits = db.recall(&req).expect("recall");
            black_box(hits);
        });
    });
    group.finish();
}

fn bench_ivfpq_recall_10k(c: &mut Criterion) {
    let path = setup_cached_corpus(CORPUS_SIZE, DIMENSIONS);
    let mut db = open_corpus_with_index(&path);
    let queries: Vec<Vec<f32>> = gen_clustered_vectors(QUERY_COUNT, DIMENSIONS, 100, 0xDEAD_BEEF);

    let mut group = c.benchmark_group("recall");
    group.sample_size(50);
    group.throughput(criterion::Throughput::Elements(1));
    let mut idx = 0usize;
    group.bench_function("ivfpq_recall_10k", |b| {
        b.iter(|| {
            let q = &queries[idx % QUERY_COUNT];
            idx += 1;
            let req = RecallRequest::new(q.clone())
                .top_k(TOP_K)
                .track_access(false);
            let hits = db.recall(&req).expect("recall");
            black_box(hits);
        });
    });
    group.finish();
}

criterion_group!(recall_benches, bench_ivfpq_recall_10k, bench_exact_scan_10k);
criterion_main!(recall_benches);
