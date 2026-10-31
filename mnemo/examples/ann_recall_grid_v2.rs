//! PR ANN-1 step 2: tune the ANN defaults.
//!
//! Diagnose (ann_diagnose.rs) ruled out an engine bug — the Phase 6
//! ceiling at 0.826 was a tunable combination of (1) the current
//! query-time defaults (`n_probe=8, n_rerank=64`) being too small for
//! realistic corpus sizes, and (2) PQ codebook capacity: with the
//! default `pq_subspaces=dims/8=48` on 384-dim data, the 100K tier
//! tops out at ~0.92 even with a 1000-wide rerank.
//!
//! This harness sweeps the two dimensions the planner called out
//! (partitions / PQ subspaces / rerank depth), expresses defaults
//! relative to the index (fraction of partitions) and the query
//! (multiple of top_k), and reports latency *next to* recall at
//! every cell. The chosen defaults get recorded under the
//! `ann_recall_grid_v2` key in `perf_v0.5.0.json`.
//!
//! Grid shape:
//!
//! - **Build dial** — `pq_subspaces`: `dims/8` (default, 48) and
//!   `dims/4` (96). More subspaces = more bits/vector = finer PQ
//!   distance approximation. Cost is at index build + roughly 2× the
//!   codes-per-vector storage.
//! - **Query dial 1** — `n_probe_frac` in {0.08, 0.16, 0.32, 0.5, 1.0}
//!   of the partition count.
//! - **Query dial 2** — `n_rerank_mult` in {5, 10, 20, 50, 100} times
//!   `top_k=10`.
//!
//! That's 2 × 5 × 5 = 50 cells per corpus tier. On the M1 Pro, 10K
//! takes ~1 min end-to-end; 100K takes ~5 min (dominated by the two
//! index rebuilds — ~30s each — and the full-scan queries at the
//! `n_probe_frac=1.0` corner of the 100K tier).
//!
//! Harness mutates the in-memory index only; it never flushes, so the
//! cached file on disk keeps its default (48-subspace) index for the
//! next bench run.

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
const TARGET_RECALL: f32 = 0.95;

// --- Deterministic clustered-vector gen (same as the bench corpus) --
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
                return path;
            }
        }
        let _ = fs::remove_file(&path);
    }
    println!(
        "[ann-grid-v2] building {n}-memory corpus at {}",
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
    db.build_index_with(IndexConfig::default()).expect("build");
    db.flush().expect("flush");
    path
}

fn sweep_tier(n: usize) -> Value {
    let path = ensure_corpus(n);
    println!("\n[ann-grid-v2] tier={n}");

    // Ground truth from exact search. Same 50 queries, same seed as the
    // Phase 6 harnesses so results are comparable across runs.
    let queries = gen_clustered_vectors(QUERY_COUNT, DIMENSIONS, 100, 0xDEAD_BEEF);
    let truth: Vec<HashSet<String>> = {
        let mut db = Mnemo::open(&path, PASSPHRASE).unwrap();
        queries
            .iter()
            .map(|q| {
                db.search(q, TOP_K, Metric::Cosine)
                    .unwrap()
                    .into_iter()
                    .map(|(m, _)| m.id.to_string())
                    .collect()
            })
            .collect()
    };

    // Grid axes — expressed as fractions of partitions and multiples of
    // top_k, so the chosen defaults are formulas not fixed numbers.
    let n_probe_fracs: Vec<f32> = vec![0.08, 0.16, 0.32, 0.5, 1.0];
    let n_rerank_mults: Vec<usize> = vec![5, 10, 20, 50, 100];
    // Build dial: default pq_subspaces (dims/8=48) and double (dims/4=96).
    let pq_configs: Vec<(usize, &str)> = vec![
        (DIMENSIONS / 8, "default_48"),
        (DIMENSIONS / 4, "doubled_96"),
    ];

    let mut configs_out: Vec<Value> = Vec::new();
    for (pq_subspaces, pq_label) in &pq_configs {
        println!("  pq_subspaces={pq_subspaces} ({pq_label}) — rebuilding index in memory");
        let mut db = Mnemo::open(&path, PASSPHRASE).unwrap();
        db.drop_index();
        let t_build = Instant::now();
        db.build_index_with(IndexConfig {
            pq_subspaces: *pq_subspaces,
            ..Default::default()
        })
        .expect("build index");
        let build_secs = t_build.elapsed().as_secs_f64();
        let partitions = db.stats().unwrap().index.unwrap().partitions;
        println!(
            "    built {} partitions × {} subspaces in {build_secs:.2}s",
            partitions, pq_subspaces,
        );

        let mut cells: Vec<Value> = Vec::new();
        for &frac in &n_probe_fracs {
            let n_probe = ((partitions as f32) * frac).ceil().max(1.0) as usize;
            for &mult in &n_rerank_mults {
                let n_rerank = TOP_K * mult;
                let t0 = Instant::now();
                let mut recalls = Vec::with_capacity(QUERY_COUNT);
                for (q, truth_ids) in queries.iter().zip(truth.iter()) {
                    let req = RecallRequest::new(q.clone())
                        .top_k(TOP_K)
                        .n_probe(n_probe)
                        .n_rerank(n_rerank)
                        .track_access(false);
                    let hits = db.recall(&req).expect("recall");
                    let got: HashSet<String> =
                        hits.into_iter().map(|h| h.memory.id.to_string()).collect();
                    let hit = truth_ids.intersection(&got).count();
                    recalls.push(hit as f32 / TOP_K as f32);
                }
                let mean_secs = t0.elapsed().as_secs_f64() / QUERY_COUNT as f64;
                let mean_recall = recalls.iter().sum::<f32>() / QUERY_COUNT as f32;
                cells.push(json!({
                    "n_probe_frac": frac,
                    "n_probe": n_probe,
                    "n_rerank_mult": mult,
                    "n_rerank": n_rerank,
                    "recall_at_10": round4(mean_recall as f64),
                    "mean_query_secs": round6(mean_secs),
                }));
            }
        }

        // Find the smallest (n_probe, n_rerank) product that still
        // hits TARGET_RECALL — that's the "knee" per the planner's spec.
        // If nothing hits the target, pick the single highest-recall
        // cell instead so the write-up can document the ceiling.
        let hitting_target: Vec<&Value> = cells
            .iter()
            .filter(|c| c["recall_at_10"].as_f64().unwrap_or(0.0) >= TARGET_RECALL as f64)
            .collect();
        let knee: Value = if hitting_target.is_empty() {
            // No cell meets 0.95 — pick the top recall as the ceiling.
            let mut best = &cells[0];
            for c in &cells {
                if c["recall_at_10"].as_f64() > best["recall_at_10"].as_f64() {
                    best = c;
                }
            }
            json!({ "hit_target": false, "cell": best })
        } else {
            // Cells that hit the target. Pick the fastest among them,
            // tiebreak by lower product of (n_probe * n_rerank) =
            // cheapest cell meeting the recall bar.
            let mut best = hitting_target[0];
            for c in &hitting_target {
                let b_t = best["mean_query_secs"].as_f64().unwrap_or(1e9);
                let c_t = c["mean_query_secs"].as_f64().unwrap_or(1e9);
                if c_t < b_t {
                    best = c;
                }
            }
            json!({ "hit_target": true, "cell": best })
        };

        println!("    knee: {}", knee);
        configs_out.push(json!({
            "pq_subspaces": pq_subspaces,
            "label": pq_label,
            "partitions": partitions,
            "build_secs": round4(build_secs),
            "cells": cells,
            "knee": knee,
        }));
    }

    json!({
        "corpus_size": n,
        "dimensions": DIMENSIONS,
        "query_count": QUERY_COUNT,
        "top_k": TOP_K,
        "target_recall": TARGET_RECALL,
        "configs": configs_out,
    })
}

fn main() {
    let tiers: Vec<Value> = [10_000usize, 100_000]
        .iter()
        .map(|&n| sweep_tier(n))
        .collect();

    let perf_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("test/scripts/perf_v0.5.0.json");
    let mut out: Value = if perf_path.exists() {
        let txt = fs::read_to_string(&perf_path).unwrap();
        let v: Value = serde_json::from_str(&txt).unwrap();
        if v.is_array() {
            json!({ "legacy_rows": v })
        } else {
            v
        }
    } else {
        json!({})
    };
    out["ann_recall_grid_v2"] = json!({
        "methodology":
            "For each (corpus_size, pq_subspaces) config: rebuild the ANN index in \
             memory (no flush — cached file keeps its default-config index), sweep \
             n_probe as fractions of partitions {0.08, 0.16, 0.32, 0.5, 1.0} crossed \
             with n_rerank as multiples of top_k {5, 10, 20, 50, 100}. Report mean \
             recall@10 against exact search + mean query latency per cell. 'Knee' per \
             config = fastest cell meeting target_recall=0.95; falls back to the \
             highest-recall cell when the target isn't reached (= the config's ceiling).",
        "tiers": tiers,
    });
    fs::write(
        &perf_path,
        serde_json::to_string_pretty(&out).unwrap() + "\n",
    )
    .unwrap();
    println!("\n[ann-grid-v2] wrote {}", perf_path.display());
}

fn round4(x: f64) -> Value {
    Value::from((x * 10000.0).round() / 10000.0)
}
fn round6(x: f64) -> Value {
    Value::from((x * 1_000_000.0).round() / 1_000_000.0)
}
