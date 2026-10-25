//! Phase 6 speed bench: `recall` — exact scan vs IVF+PQ, at 10K, 100K
//! and 1M tiers (the planner's spec). Each tier produces two bench
//! entries: `ivfpq_recall_{10k|100k|1m}` and `exact_scan_{10k|100k|1m}`.
//!
//! The ivfpq config uses `IndexConfig::default()` (n_probe=8,
//! n_rerank=64, auto partition count ≈ √n). The exact config loads
//! the same cached file but opens a tempdir copy with the in-memory
//! index dropped, so the cached file keeps its index intact for the
//! ivfpq bench's next run — see [`corpus::open_corpus_exact_only`].
//!
//! Query vectors are a 20-entry batch seeded with `0xDEAD_BEEF`,
//! identical across tiers so hit/miss patterns are comparable; they
//! share the corpus's cluster structure but use a different seed so
//! no query is a corpus duplicate.
//!
//! `track_access=false` so repeated iterations don't mutate the catalog
//! — otherwise one bench run would bump every returned memory's
//! `access_count`, which is fine for correctness but muddies the
//! per-iteration measurement.
//!
//! The 100K and 1M setup is expensive: ~2 min and ~20 min respectively
//! on first run (insert loop + IVF+PQ build with fast KDF). Subsequent
//! runs reuse the cached `target/benches/corpus_<n>_384d.mnemo` file in
//! milliseconds. If the reference machine is short on disk the
//! maintainer skips a tier by not naming it on the `cargo bench`
//! command line.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use mnemo::RecallRequest;

#[path = "corpus.rs"]
mod corpus;
use corpus::{
    gen_clustered_vectors, open_corpus_exact_only, open_corpus_with_index, setup_cached_corpus,
    DIMENSIONS,
};

const QUERY_COUNT: usize = 20;
const TOP_K: usize = 10;

/// Run both configurations (ivfpq + exact scan) for a single corpus
/// tier. `label` is the suffix in the bench name (`"10k"`, `"100k"`,
/// `"1m"`). `sample_size` is tuned per tier because exact-scan latency
/// grows linearly with corpus size and the default 100 samples blows
/// criterion's wall-time budget at 100K+.
fn bench_recall_pair(c: &mut Criterion, corpus_size: usize, label: &str, sample_size: usize) {
    let cached = setup_cached_corpus(corpus_size, DIMENSIONS);
    let queries: Vec<Vec<f32>> = gen_clustered_vectors(QUERY_COUNT, DIMENSIONS, 100, 0xDEAD_BEEF);

    // ivfpq first: opens the cached file directly, uses its on-disk
    // index. Scoped {} so the handle drops (and releases the lock)
    // before the exact-scan copy opens.
    {
        let mut db = open_corpus_with_index(&cached);
        let mut group = c.benchmark_group("recall");
        group.sample_size(sample_size);
        group.throughput(criterion::Throughput::Elements(1));
        let mut idx = 0usize;
        group.bench_function(format!("ivfpq_recall_{label}"), |b| {
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

    // Exact scan: reopen the cached file (ivfpq's handle has been
    // dropped above, releasing the lock), unload the in-memory ANN
    // without flushing, and recall falls back to brute force. On-disk
    // index stays intact for the next bench run.
    {
        let mut db = open_corpus_exact_only(&cached);
        let mut group = c.benchmark_group("recall");
        group.sample_size(sample_size);
        group.throughput(criterion::Throughput::Elements(1));
        let mut idx = 0usize;
        group.bench_function(format!("exact_scan_{label}"), |b| {
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
}

fn bench_recall_10k(c: &mut Criterion) {
    bench_recall_pair(c, 10_000, "10k", 50);
}

fn bench_recall_100k(c: &mut Criterion) {
    // 100K × 384-dim ≈ 150 MB of raw vectors. Setup: ~2 min first run.
    bench_recall_pair(c, 100_000, "100k", 20);
}

fn bench_recall_1m(c: &mut Criterion) {
    // 1M × 384-dim ≈ 1.5 GB of raw vectors. Setup: ~20 min first run.
    // Exact-scan latency expected in the multi-second range.
    bench_recall_pair(c, 1_000_000, "1m", 10);
}

criterion_group!(
    recall_benches,
    bench_recall_10k,
    bench_recall_100k,
    bench_recall_1m,
);
criterion_main!(recall_benches);
