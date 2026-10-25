//! Phase 6 ANN quality harness: recall@10 of IVF+PQ vs exact scan
//! over a grid of `n_probe` × `n_rerank` dials.
//!
//! Different from the ranking harness (which asks "is the *right*
//! memory at rank 1?") and from the criterion recall bench (which
//! asks "how fast?"). This asks: given exact top-10 as ground truth,
//! how many of those does IVF+PQ return at each setting of its two
//! query-time dials?
//!
//! Grid:
//! - `n_probe` ∈ {1, 2, 4, 8, 16, 32}  — IVF partitions scanned per query
//! - `n_rerank` ∈ {16, 32, 64, 128, 256} — PQ candidates reranked exactly
//!
//! Corpus: the 10K cached corpus from the recall bench
//! (`target/benches/corpus_10000_384d.mnemo`). Built in-place if
//! absent so the example runs standalone. Index built once at the
//! coarsest setting; the two query-time dials are overridden per
//! query via `RecallRequest::n_probe` / `n_rerank`.
//!
//! Output: a matrix of mean recall@10 (per cell, averaged across the
//! query batch), written to `test/scripts/perf_v0.5.0.json` under the
//! `ann_recall_grid` key — alongside the ranking + speed blocks.
//!
//! Run:
//! ```sh
//! cargo run --release --example ann_recall_grid
//! ```

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use mnemo::{IndexConfig, KdfParams, Memory, MemoryType, Metric, Mnemo, MnemoConfig, RecallRequest};
use serde_json::{json, Value};

const PASSPHRASE: &str = "bench-pw";
const DIMENSIONS: usize = 384;
const CORPUS_SIZE: usize = 10_000;
const QUERY_COUNT: usize = 50;
const TOP_K: usize = 10;

// --- Match the bench harness's deterministic RNG + vector gen so the --
// cached corpus is bit-identical whether built by the bench or by this
// example. See mnemo/benches/corpus.rs for the full rationale.
fn gen_clustered_vectors(n: usize, dims: usize, k_clusters: usize, seed: u64) -> Vec<Vec<f32>> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
    let mut next_u64 = || -> u64 {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let mut next_normal = || -> f32 {
        let u1 = ((next_u64() >> 11) as f64 / (1u64 << 53) as f64).max(1e-12);
        let u2 = (next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        let r = (-2.0 * u1.ln()).sqrt();
        (r * (2.0 * std::f64::consts::PI * u2).cos()) as f32
    };
    let normalize = |v: &mut [f32]| {
        let mut norm = 0.0f32;
        for x in v.iter() {
            norm += x * x;
        }
        norm = norm.sqrt();
        if norm == 0.0 {
            v[0] = 1.0;
        } else {
            for x in v.iter_mut() {
                *x /= norm;
            }
        }
    };
    let mut centres: Vec<Vec<f32>> = Vec::with_capacity(k_clusters);
    for _ in 0..k_clusters {
        let mut c = (0..dims).map(|_| next_normal()).collect::<Vec<_>>();
        normalize(&mut c);
        centres.push(c);
    }
    const NOISE_SIGMA: f32 = 0.1;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let cluster = i % k_clusters;
        let mut v: Vec<f32> = centres[cluster]
            .iter()
            .map(|&c| c + NOISE_SIGMA * next_normal())
            .collect();
        normalize(&mut v);
        out.push(v);
    }
    out
}

fn cached_corpus_path() -> PathBuf {
    let base = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_string());
    PathBuf::from(base)
        .join("benches")
        .join(format!("corpus_{CORPUS_SIZE}_{DIMENSIONS}d.mnemo"))
}

fn ensure_corpus() -> PathBuf {
    let path = cached_corpus_path();
    if path.exists() {
        if let Ok(db) = Mnemo::open(&path, PASSPHRASE) {
            if db.dimensions() == DIMENSIONS {
                println!("[ann-grid] using cached corpus at {}", path.display());
                return path;
            }
        }
        let _ = fs::remove_file(&path);
    }
    println!(
        "[ann-grid] building {CORPUS_SIZE}-memory corpus at {} (one-time)",
        path.display()
    );
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let cfg = MnemoConfig {
        dimensions: DIMENSIONS,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    let mut db = Mnemo::create(&path, PASSPHRASE, cfg).expect("create corpus");
    let k_clusters = ((CORPUS_SIZE as f64).sqrt().ceil() as usize).max(16);
    let vectors = gen_clustered_vectors(CORPUS_SIZE, DIMENSIONS, k_clusters, 0xBEEF_C0DE);
    for (i, v) in vectors.into_iter().enumerate() {
        db.remember(Memory::new(
            format!("bench memory #{i}"),
            MemoryType::Semantic,
            v,
        ))
        .expect("remember");
    }
    db.flush().expect("flush corpus");
    db.build_index_with(IndexConfig::default())
        .expect("build index");
    db.flush().expect("flush after index build");
    path
}

fn main() {
    let path = ensure_corpus();
    let mut db = Mnemo::open(&path, PASSPHRASE).expect("open corpus");
    assert!(
        db.stats().expect("stats").index.is_some(),
        "cached corpus has no index — rebuild via `cargo bench --bench recall --features benches` or delete the cached file"
    );

    // 50 queries, same cluster distribution as corpus but different
    // seed so none are duplicates. The ground truth is each query's
    // exact top-10 computed via `search()` (which is always exact).
    println!("[ann-grid] generating {QUERY_COUNT} queries + ground truth");
    let queries = gen_clustered_vectors(QUERY_COUNT, DIMENSIONS, 100, 0xDEAD_BEEF);
    let truth: Vec<HashSet<String>> = queries
        .iter()
        .map(|q| {
            db.search(q, TOP_K, Metric::Cosine)
                .expect("exact search")
                .into_iter()
                .map(|(m, _sim)| m.id.to_string())
                .collect()
        })
        .collect();

    // The two dials. Powers of two cover the useful range.
    let n_probes: Vec<usize> = vec![1, 2, 4, 8, 16, 32];
    let n_reranks: Vec<usize> = vec![16, 32, 64, 128, 256];

    println!(
        "[ann-grid] measuring {}×{} = {} configurations × {} queries",
        n_probes.len(),
        n_reranks.len(),
        n_probes.len() * n_reranks.len(),
        QUERY_COUNT
    );

    // Cell = (n_probe, n_rerank) → mean recall@10 across the query
    // batch. Also track mean per-query wall time so a reader can trade
    // recall against cost in one glance.
    let mut rows: Vec<Value> = Vec::new();
    for &p in &n_probes {
        for &r in &n_reranks {
            let mut recalls: Vec<f32> = Vec::with_capacity(QUERY_COUNT);
            let t0 = Instant::now();
            for (q, truth_ids) in queries.iter().zip(truth.iter()) {
                let req = RecallRequest::new(q.clone())
                    .top_k(TOP_K)
                    .n_probe(p)
                    .n_rerank(r)
                    .track_access(false);
                let hits = db.recall(&req).expect("ivfpq recall");
                let got: HashSet<String> =
                    hits.into_iter().map(|h| h.memory.id.to_string()).collect();
                let hit = truth_ids.intersection(&got).count();
                recalls.push(hit as f32 / TOP_K as f32);
            }
            let elapsed_per_query = t0.elapsed().as_secs_f64() / QUERY_COUNT as f64;
            let mean = recalls.iter().sum::<f32>() / recalls.len() as f32;
            rows.push(json!({
                "n_probe": p,
                "n_rerank": r,
                "recall_at_10": round4(mean),
                "mean_query_secs": round6(elapsed_per_query),
            }));
            println!(
                "  n_probe={:>2} n_rerank={:>3}  recall@10={:.3}  mean={:>7.1}µs/query",
                p,
                r,
                mean,
                elapsed_per_query * 1_000_000.0
            );
        }
    }

    // Merge into perf_v0.5.0.json under `ann_recall_grid` next to
    // `ranking` (and whatever the collector eventually puts under
    // `speed`). Handle the legacy list format from v0.1.0.
    let perf_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("test/scripts/perf_v0.5.0.json");
    let mut out: Value = if perf_path.exists() {
        let txt = fs::read_to_string(&perf_path).expect("read perf json");
        let v: Value = serde_json::from_str(&txt).expect("parse perf json");
        if v.is_array() {
            json!({ "legacy_rows": v })
        } else {
            v
        }
    } else {
        json!({})
    };
    out["ann_recall_grid"] = json!({
        "corpus_size": CORPUS_SIZE,
        "dimensions": DIMENSIONS,
        "query_count": QUERY_COUNT,
        "top_k": TOP_K,
        "metric": "cosine",
        "index_config": "IndexConfig::default() (n_partitions=auto, pq_subspaces=auto, kmeans_iters=25)",
        "cells": rows,
    });
    fs::write(
        &perf_path,
        serde_json::to_string_pretty(&out).expect("serialize") + "\n",
    )
    .expect("write perf json");
    println!("[ann-grid] wrote {}", perf_path.display());
}

fn round4(x: f32) -> Value {
    Value::from((x * 10000.0).round() / 10000.0)
}

fn round6(x: f64) -> Value {
    Value::from((x * 1_000_000.0).round() / 1_000_000.0)
}
