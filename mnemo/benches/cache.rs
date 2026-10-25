//! Phase 6 speed bench: result cache.
//!
//! Covers:
//!
//! 1. [`cache_get_exact_hit`] — the hot path. SHA-256 key hash +
//!    directory lookup + record page read. No flush — reads don't dirty.
//! 2. [`cache_put_exact_strict`] — `cache_put` under `CacheFlushPolicy::
//!    Strict`. Every put forces a flush before returning, so the cost
//!    is dominated by fsync (same floor the write bench measures).
//! 3. [`cache_put_exact_batched`] — same `cache_put` under
//!    `CacheFlushPolicy::Batched { max_dirty: 1_000, max_age: 1 min }`.
//!    The engine's own `should_auto_flush` keeps flushes rare; most
//!    puts return without disk I/O.
//! 4. [`cache_get_semantic_hit`] — scans the live namespace, filters
//!    by model, computes cosine, picks the best. Linear in namespace
//!    size — pre-populated with 500 semantic entries so this measures
//!    the realistic cost of a semantic lookup against a non-trivial
//!    cache.
//! 5. [`cache_put_semantic_strict`] — the semantic put path under
//!    Strict; same fsync floor as exact-key put plus the vector write.
//!
//! The exact-get bench pre-populates one entry per iteration key so
//! the hot path never misses; the semantic benches pre-populate 500
//! entries with different vectors to give the scan something to walk.
//!
//! Fast KDF everywhere — these benches measure cache mechanics, not
//! key derivation (the open bench isolates that).

use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use mnemo::{CacheFlushPolicy, CachePutOpts, Mnemo, SemanticCachePutOpts};
use tempfile::TempDir;

#[path = "corpus.rs"]
mod corpus;
use corpus::{fast_kdf_cfg, gen_clustered_vectors, DIMENSIONS, PASSPHRASE};

const NS: &str = "bench";
const EXACT_PREPOP: usize = 100;
const SEMANTIC_PREPOP: usize = 500;
const SEMANTIC_MODEL: &str = "bench-model";

fn make_db(td: &TempDir) -> Mnemo {
    let path = td.path().join("cache.mnemo");
    Mnemo::create(&path, PASSPHRASE, fast_kdf_cfg(DIMENSIONS)).expect("create")
}

fn bench_cache_get_exact_hit(c: &mut Criterion) {
    let td = TempDir::new().expect("tempdir");
    let mut db = make_db(&td);
    // Pre-populate with keys "k0".."k99" so each iter can hit without
    // mutating anything downstream.
    for i in 0..EXACT_PREPOP {
        db.cache_put(
            NS,
            &format!("k{i}"),
            format!("v{i}").as_bytes(),
            CachePutOpts::default(),
        )
        .expect("prepop cache_put");
    }
    db.flush().expect("flush prepop");

    let mut idx = 0usize;
    let mut group = c.benchmark_group("cache");
    group.throughput(criterion::Throughput::Elements(1));
    group.bench_function("cache_get_exact_hit", |b| {
        b.iter(|| {
            let key = format!("k{}", idx % EXACT_PREPOP);
            idx += 1;
            let hit = db.cache_get(NS, &key).expect("cache_get");
            black_box(hit);
        });
    });
    group.finish();
}

fn bench_cache_put_exact_strict(c: &mut Criterion) {
    let td = TempDir::new().expect("tempdir");
    let mut db = make_db(&td);
    db.set_cache_flush_policy(CacheFlushPolicy::Strict);

    // Each iteration writes a *different* key so the directory grows
    // but entries don't thrash via LRU eviction (default budget is
    // 10K entries / 64 MiB — plenty of headroom for a bench run).
    let mut idx = 0usize;
    let mut group = c.benchmark_group("cache");
    // Strict is bounded by fsync (same floor as remember+flush: ~20ms
    // on a laptop SSD). 20 samples keeps wall-time ~400ms.
    group.sample_size(20);
    group.throughput(criterion::Throughput::Elements(1));
    group.bench_function("cache_put_exact_strict", |b| {
        b.iter(|| {
            let key = format!("strict-{idx}");
            idx += 1;
            db.cache_put(NS, &key, b"v", CachePutOpts::default())
                .expect("cache_put strict");
        });
    });
    group.finish();
}

fn bench_cache_put_exact_batched(c: &mut Criterion) {
    let td = TempDir::new().expect("tempdir");
    let mut db = make_db(&td);
    db.set_cache_flush_policy(CacheFlushPolicy::Batched {
        max_dirty: 1_000,
        max_age: Duration::from_secs(60),
    });

    let mut idx = 0usize;
    let mut group = c.benchmark_group("cache");
    group.throughput(criterion::Throughput::Elements(1));
    group.bench_function("cache_put_exact_batched", |b| {
        b.iter(|| {
            let key = format!("batched-{idx}");
            idx += 1;
            db.cache_put(NS, &key, b"v", CachePutOpts::default())
                .expect("cache_put batched");
        });
    });
    group.finish();
    // Not strictly necessary — tempdir drop runs the engine's own
    // teardown — but makes the pending-batch state explicit.
    db.flush().expect("final flush");
}

fn prepop_semantic(db: &mut Mnemo) -> Vec<Vec<f32>> {
    // 500 semantic entries with distinct vectors — gives the semantic
    // scan something to walk. All share one model so none are filtered
    // out at lookup time.
    let vectors = gen_clustered_vectors(SEMANTIC_PREPOP, DIMENSIONS, 32, 0xCACE_5EED);
    for (i, v) in vectors.iter().enumerate() {
        db.cache_put_semantic(
            NS,
            &format!("sem-k{i}"),
            v.clone(),
            format!("sv{i}").as_bytes(),
            SemanticCachePutOpts::new(SEMANTIC_MODEL),
        )
        .expect("cache_put_semantic prepop");
    }
    db.flush().expect("flush semantic prepop");
    vectors
}

fn bench_cache_get_semantic_hit(c: &mut Criterion) {
    let td = TempDir::new().expect("tempdir");
    let mut db = make_db(&td);
    let prepop_vectors = prepop_semantic(&mut db);

    // Query vectors seeded differently from the corpus so none are an
    // exact match; same cluster count so hits are plausible.
    let queries: Vec<Vec<f32>> = gen_clustered_vectors(20, DIMENSIONS, 32, 0xCACE_B055);

    let mut idx = 0usize;
    let mut group = c.benchmark_group("cache");
    group.throughput(criterion::Throughput::Elements(1));
    // Threshold 0.0 so every query hits something — we want to measure
    // the scan-and-find-best cost, not the no-result cost.
    group.bench_function("cache_get_semantic_hit", |b| {
        b.iter(|| {
            let q = &queries[idx % queries.len()];
            idx += 1;
            let hit = db
                .cache_get_semantic(NS, q, 0.0, SEMANTIC_MODEL)
                .expect("cache_get_semantic");
            black_box(hit);
        });
    });
    group.finish();
    // Keep `prepop_vectors` alive until here so the compiler can't
    // DCE the setup work above as unused.
    black_box(prepop_vectors);
}

fn bench_cache_put_semantic_strict(c: &mut Criterion) {
    let td = TempDir::new().expect("tempdir");
    let mut db = make_db(&td);
    db.set_cache_flush_policy(CacheFlushPolicy::Strict);

    // One vector per iteration, cycled from a small pool so vector gen
    // isn't inside the measured region.
    let pool: Vec<Vec<f32>> = gen_clustered_vectors(50, DIMENSIONS, 8, 0xCACE_D04E);
    let mut idx = 0usize;
    let mut group = c.benchmark_group("cache");
    group.sample_size(20);
    group.throughput(criterion::Throughput::Elements(1));
    group.bench_function("cache_put_semantic_strict", |b| {
        b.iter_batched(
            || {
                let i = idx;
                idx += 1;
                let key = format!("sem-strict-{i}");
                let v = pool[i % pool.len()].clone();
                (key, v)
            },
            |(key, v)| {
                db.cache_put_semantic(NS, &key, v, b"v", SemanticCachePutOpts::new(SEMANTIC_MODEL))
                    .expect("cache_put_semantic strict");
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(
    cache_benches,
    bench_cache_get_exact_hit,
    bench_cache_put_exact_strict,
    bench_cache_put_exact_batched,
    bench_cache_get_semantic_hit,
    bench_cache_put_semantic_strict,
);
criterion_main!(cache_benches);
