//! PR ANN-1 step 1: diagnose the 0.826 recall@10 ceiling.
//!
//! The Phase 6 grid topped out at recall@10 = 0.826 at the (n_probe=32,
//! n_rerank=256) corner. The planner's call: before tuning anything,
//! run the index at **full scan** — n_probe = every partition, large
//! n_rerank — and compare against exact `search()`. That setting
//! removes the "did we visit enough partitions?" and "did we rerank
//! enough?" questions, so if recall still isn't ≥0.99 the shortfall
//! has to be in the PQ codebook, the distance math, or the rerank
//! handoff. In that case there's a real bug to fix before tuning.
//!
//! Measures three sanity cells on the cached 10K and 100K corpora:
//!
//! 1. **Full-scan ceiling**: n_probe = partitions, n_rerank = 1000.
//!    This is what the diagnose actually asks. Target: recall@10 ≥ 0.99.
//! 2. **n_probe = partitions, n_rerank = 10 × top_k = 100.** Smaller
//!    rerank, same probe. If this drops recall markedly, the PQ shortlist
//!    is losing true neighbors — PQ codebook or sub_offsets are too
//!    coarse.
//! 3. **n_probe = 1, n_rerank = corpus_size.** Reverse of #1. Reranks
//!    everything PQ sees but only in one partition. If this drops
//!    recall markedly, true neighbors live outside the single IVF
//!    partition the query lands in — expected on cluster-boundary
//!    queries, which is why n_probe > 1 exists.
//!
//! Writes `test/scripts/perf_v0.5.0.json["ann_diagnose"]`.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use mnemo::{
    IndexConfig, KdfParams, Memory, MemoryType, Metric, Mnemo, MnemoConfig, RecallRequest,
};
use serde_json::{json, Value};

const PASSPHRASE: &str = "bench-pw";
const DIMENSIONS: usize = 384;
const QUERY_COUNT: usize = 50;
const TOP_K: usize = 10;

// Same hand-rolled RNG + clustered vector gen as the bench corpus
// harness. Keeps the cached file bit-identical whether built there or
// here. See `mnemo/benches/corpus.rs` for the full rationale.
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

fn cached_corpus_path(n: usize) -> PathBuf {
    let base = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_string());
    PathBuf::from(base)
        .join("benches")
        .join(format!("corpus_{n}_{DIMENSIONS}d.mnemo"))
}

fn ensure_corpus(n: usize) -> PathBuf {
    let path = cached_corpus_path(n);
    if path.exists() {
        if let Ok(db) = Mnemo::open(&path, PASSPHRASE) {
            if db.dimensions() == DIMENSIONS {
                println!("[ann-diagnose] using cached corpus at {}", path.display());
                return path;
            }
        }
        let _ = fs::remove_file(&path);
    }
    println!(
        "[ann-diagnose] building {n}-memory corpus at {}",
        path.display()
    );
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let cfg = MnemoConfig {
        dimensions: DIMENSIONS,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    let mut db = Mnemo::create(&path, PASSPHRASE, cfg).expect("create");
    let k_clusters = ((n as f64).sqrt().ceil() as usize).max(16);
    let vectors = gen_clustered_vectors(n, DIMENSIONS, k_clusters, 0xBEEF_C0DE);
    for (i, v) in vectors.into_iter().enumerate() {
        db.remember(Memory::new(
            format!("bench memory #{i}"),
            MemoryType::Semantic,
            v,
        ))
        .expect("remember");
    }
    db.flush().expect("flush");
    db.build_index_with(IndexConfig::default())
        .expect("build index");
    db.flush().expect("flush");
    path
}

fn diagnose_tier(n: usize) -> Value {
    let path = ensure_corpus(n);
    let mut db = Mnemo::open(&path, PASSPHRASE).expect("open");
    let info = db.stats().expect("stats").index.expect("index was built");
    let partitions = info.partitions;
    let subspaces = info.subspaces;
    println!("\n[ann-diagnose] tier={n}  partitions={partitions}  pq_subspaces={subspaces}");

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

    let mut cells = Vec::new();
    for (label, n_probe, n_rerank) in [
        ("full_scan_rerank_1000", partitions, 1000usize),
        ("full_probe_rerank_100", partitions, 100usize),
        ("single_probe_rerank_all", 1usize, n),
    ] {
        let t0 = Instant::now();
        let mut recalls = Vec::with_capacity(QUERY_COUNT);
        for (q, truth_ids) in queries.iter().zip(truth.iter()) {
            let req = RecallRequest::new(q.clone())
                .top_k(TOP_K)
                .n_probe(n_probe)
                .n_rerank(n_rerank)
                .track_access(false);
            let hits = db.recall(&req).expect("recall");
            let got: HashSet<String> = hits.into_iter().map(|h| h.memory.id.to_string()).collect();
            let hit = truth_ids.intersection(&got).count();
            recalls.push(hit as f32 / TOP_K as f32);
        }
        let mean_recall = recalls.iter().sum::<f32>() / QUERY_COUNT as f32;
        let mean_secs = t0.elapsed().as_secs_f64() / QUERY_COUNT as f64;
        println!(
            "  {label:30}  n_probe={n_probe:>4}  n_rerank={n_rerank:>6}  recall@10={mean_recall:.4}  {:>8.3} ms/query",
            mean_secs * 1000.0
        );
        cells.push(json!({
            "label": label,
            "n_probe": n_probe,
            "n_rerank": n_rerank,
            "recall_at_10": round4(mean_recall as f64),
            "mean_query_secs": round6(mean_secs),
        }));
    }

    let full = cells[0]["recall_at_10"].as_f64().unwrap_or(0.0);
    let verdict = if full >= 0.99 {
        "pass: no bug — ceiling is tunable"
    } else if full >= 0.9 {
        "borderline: PQ loses a few neighbors even with max rerank; check codebook"
    } else {
        "FAIL: real bug — investigate PQ encoding, distance math, or rerank handoff"
    };
    println!("  verdict: {verdict}");

    json!({
        "corpus_size": n,
        "dimensions": DIMENSIONS,
        "query_count": QUERY_COUNT,
        "top_k": TOP_K,
        "partitions": partitions,
        "pq_subspaces": subspaces,
        "cells": cells,
        "full_scan_recall_at_10": full,
        "verdict": verdict,
    })
}

fn main() {
    let tiers: Vec<Value> = [10_000usize, 100_000]
        .iter()
        .map(|&n| diagnose_tier(n))
        .collect();

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
    out["ann_diagnose"] = json!({ "tiers": tiers });
    fs::write(
        &perf_path,
        serde_json::to_string_pretty(&out).expect("serialize") + "\n",
    )
    .expect("write perf json");
    println!("\n[ann-diagnose] wrote {}", perf_path.display());
}

fn round4(x: f64) -> Value {
    Value::from((x * 10000.0).round() / 10000.0)
}

fn round6(x: f64) -> Value {
    Value::from((x * 1_000_000.0).round() / 1_000_000.0)
}
