//! Phase 6 speed bench: `Mnemo::open` latency, split into KDF cost
//! and "everything else".
//!
//! The planner's dispatch calls this out specifically to settle the
//! "~300 ms open" question — that number was measured in debug build
//! with fast KDF. The real-world cost is dominated by Argon2id key
//! derivation and we want numbers that reflect **release build with
//! production KDF params**.
//!
//! Two bench entries:
//!
//! 1. [`open_latency_fast_kdf`] — opens a file encrypted with
//!    `KdfParams::fast()`. Argon2id runs with m_cost=512, t_cost=1 —
//!    sub-millisecond. The reported time is "everything else": header
//!    decode, WAL probe, catalog load, cache directory load.
//! 2. [`open_latency_secure_kdf`] — opens a file encrypted with
//!    `KdfParams::secure()` (m_cost=19 456 ≈ 19 MiB, t_cost=2). The
//!    difference between this and the fast variant is the Argon2id
//!    cost.
//!
//! Corpus is a 100-memory file (small enough that catalog load isn't
//! the signal). Both files are cached under `target/benches/`.

use std::hint::black_box;
use std::path::PathBuf;

use criterion::{criterion_group, criterion_main, Criterion};
use mnemo_engine::{Memory, MemoryType, Mnemo};

#[path = "corpus.rs"]
mod corpus;
use corpus::{
    fast_kdf_cfg, gen_clustered_vectors, secure_kdf_cfg, setup_cached_corpus, DIMENSIONS,
    PASSPHRASE,
};

const OPEN_CORPUS_SIZE: usize = 100;

/// Build (or reuse) a tiny 100-memory corpus under one of the two KDF
/// configs. The file is cached per KDF variant, so a cached file built
/// with fast KDF isn't re-used by the secure bench.
fn setup_open_corpus(cfg_tag: &str, kdf_fast: bool) -> PathBuf {
    let base = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_string());
    let dir = PathBuf::from(base).join("benches");
    std::fs::create_dir_all(&dir).expect("create target/benches directory");
    let path = dir.join(format!("open_corpus_{cfg_tag}.mnemo"));

    if path.exists() {
        // Quick probe — if open with this passphrase succeeds the file
        // is reusable. Mismatch (e.g. KDF changed) → rebuild.
        if Mnemo::open(&path, PASSPHRASE).is_ok() {
            return path;
        }
        let _ = std::fs::remove_file(&path);
    }

    let cfg = if kdf_fast {
        fast_kdf_cfg(DIMENSIONS)
    } else {
        secure_kdf_cfg(DIMENSIONS)
    };
    let mut db = Mnemo::create(&path, PASSPHRASE, cfg).expect("create open-bench file");
    let vectors = gen_clustered_vectors(OPEN_CORPUS_SIZE, DIMENSIONS, 8, 0x0FEE_B000);
    for (i, v) in vectors.into_iter().enumerate() {
        db.remember(Memory::new(
            format!("open-bench memory #{i}"),
            MemoryType::Semantic,
            v,
        ))
        .expect("remember");
    }
    db.flush().expect("flush");
    path
}

fn bench_open_latency_fast_kdf(c: &mut Criterion) {
    let path = setup_open_corpus("fast_kdf", /*kdf_fast=*/ true);
    let mut group = c.benchmark_group("open");
    // Fast KDF is sub-ms; 100 samples at a few ms each is a budget-safe
    // ~500ms of actual measurement. Default sample_size is fine.
    group.bench_function("open_latency_fast_kdf", |b| {
        b.iter(|| {
            // The handle must drop fully each iteration or the file
            // lock would prevent the next open from succeeding. The
            // iter closure owns the handle locally; drop happens at
            // the semicolon.
            let db = Mnemo::open(&path, PASSPHRASE).expect("open");
            black_box(db);
        });
    });
    group.finish();
}

fn bench_open_latency_secure_kdf(c: &mut Criterion) {
    let path = setup_open_corpus("secure_kdf", /*kdf_fast=*/ false);
    let mut group = c.benchmark_group("open");
    // Secure KDF is ~50-200ms per open depending on hardware; 50
    // samples is a budget-safe ~5-10s of measurement.
    group.sample_size(50);
    group.bench_function("open_latency_secure_kdf", |b| {
        b.iter(|| {
            let db = Mnemo::open(&path, PASSPHRASE).expect("open");
            black_box(db);
        });
    });
    group.finish();
}

/// Phase 12.1.1 lane-A DISPATCH remeasure (v0.8.1): open latency on an
/// **indexed** 10K-memory file with fast KDF. This isolates the
/// non-KDF cost dominated by catalog + ANN-index load. Pairs with the
/// 100K variant below so the README's "open ~15 ms" line can be
/// replaced with numbers that reflect what agents actually pay.
fn bench_open_latency_indexed_10k(c: &mut Criterion) {
    let cached = setup_cached_corpus(10_000, DIMENSIONS);
    let mut group = c.benchmark_group("open");
    group.sample_size(30);
    group.bench_function("open_latency_indexed_10k", |b| {
        b.iter(|| {
            let db = Mnemo::open(&cached, PASSPHRASE).expect("open");
            black_box(db);
        });
    });
    group.finish();
}

/// Phase 12.1.1 lane-A DISPATCH remeasure (v0.8.1): open latency on an
/// **indexed** 100K-memory file with fast KDF. This is the number
/// Planner-A called out as "~115 ms in my sandbox" that doesn't
/// regress v0.7.0 → v0.8.0 but needs its own README mention.
fn bench_open_latency_indexed_100k(c: &mut Criterion) {
    let cached = setup_cached_corpus(100_000, DIMENSIONS);
    let mut group = c.benchmark_group("open");
    group.sample_size(20);
    group.bench_function("open_latency_indexed_100k", |b| {
        b.iter(|| {
            let db = Mnemo::open(&cached, PASSPHRASE).expect("open");
            black_box(db);
        });
    });
    group.finish();
}

criterion_group!(
    open_benches,
    bench_open_latency_fast_kdf,
    bench_open_latency_secure_kdf,
    bench_open_latency_indexed_10k,
    bench_open_latency_indexed_100k,
);
criterion_main!(open_benches);
