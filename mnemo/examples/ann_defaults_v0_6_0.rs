//! PR ANN-1.1 measurement: recall@10 + latency at the exact shipped
//! v0.6.0 defaults, on each cached corpus tier.
//!
//! Two scenarios per tier:
//!
//! 1. **Fresh install** — index built with `IndexConfig::default()`
//!    on v0.6.0+: `pq_subspaces = dims/4` (96 for 384-dim), auto
//!    `n_probe` + size-aware auto `n_rerank` recomputed per query.
//!    This is what every new user experiences.
//! 2. **Upgrade path** — index built on v0.5.x (so `pq_subspaces =
//!    dims/8` = 48 on 384-dim; stored `n_probe = 8, n_rerank = 64`)
//!    reopened under v0.6.0. The query-time auto formulas kick in
//!    without a rebuild (see `IvfPqIndex::effective_defaults` — the
//!    legacy sentinel `(8, 64)` triggers auto). Shows what a user
//!    gets *before* running `mnemo index --rebuild`.
//!
//! Writes `test/scripts/perf_v0.5.0.json["ann_defaults_v0_6_0"]`
//! with per-tier `{fresh_install, upgrade_path}` cells carrying
//! `recall_at_10` + `mean_query_ms` + the exact `n_probe` / `n_rerank`
//! values the formulas picked.
//!
//! Run:
//! ```sh
//! cargo run --release --example ann_defaults_v0_6_0
//! ```

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

// Deterministic clustered-vector gen — same as benches/corpus.rs so
// the cached files from Phase 6 / ANN-1 are byte-identical to what
// this harness would produce.
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
        "[ann-defaults] building {n}-memory corpus at {}",
        path.display()
    );
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let cfg = MnemoConfig {
        dimensions: DIMENSIONS,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    let mut db = Mnemo::create(&path, PASSPHRASE, cfg).unwrap();
    let k_clusters = ((n as f64).sqrt().ceil() as usize).max(16);
    let vectors = gen_clustered_vectors(n, DIMENSIONS, k_clusters, 0xBEEF_C0DE);
    for (i, v) in vectors.into_iter().enumerate() {
        db.remember(Memory::new(
            format!("bench memory #{i}"),
            MemoryType::Semantic,
            v,
        ))
        .unwrap();
    }
    db.flush().unwrap();
    db.build_index_with(IndexConfig::default()).unwrap();
    db.flush().unwrap();
    path
}

/// Measure one (pq_subspaces, scenario_label) config on a corpus.
///
/// Rebuilds the ANN index in memory with the given `pq_subspaces` and
/// explicit `(n_probe, n_rerank)` cfg values (sentinels or auto, per
/// scenario), *doesn't flush* (cached file on disk is unchanged), and
/// runs 50 recall queries. Returns a JSON cell with recall@10, mean
/// latency, and the n_probe/n_rerank the index actually used at
/// query time.
fn measure_scenario(
    cached: &PathBuf,
    scenario: &str,
    pq_subspaces: usize,
    cfg_n_probe: usize,
    cfg_n_rerank: usize,
) -> Value {
    let mut db = Mnemo::open(cached, PASSPHRASE).unwrap();

    // Ground truth via exact search. Done before the index rebuild so
    // it uses whatever's on disk — the vectors themselves are what
    // matter, not the ANN state.
    let queries = gen_clustered_vectors(QUERY_COUNT, DIMENSIONS, 100, 0xDEAD_BEEF);
    let truth: Vec<HashSet<String>> = queries
        .iter()
        .map(|q| {
            db.search(q, TOP_K, Metric::Cosine)
                .unwrap()
                .into_iter()
                .map(|(m, _)| m.id.to_string())
                .collect()
        })
        .collect();

    // Rebuild the index in memory with this scenario's config.
    db.drop_index();
    let cfg = IndexConfig {
        pq_subspaces,
        n_probe: cfg_n_probe,
        n_rerank: cfg_n_rerank,
        ..Default::default()
    };
    let t_build = Instant::now();
    db.build_index_with(cfg).unwrap();
    let build_secs = t_build.elapsed().as_secs_f64();

    // Both scenarios end up on the auto formulas at query time —
    // fresh_install because build() resolves `IndexConfig::default()`
    // to those values, upgrade_path because the stored (8, 64)
    // sentinel triggers them. ANN-1.1 made `IvfPqIndex::info()`
    // report the effective (shim-applied) numbers directly, so just
    // read them back from `Stats::index` instead of reimplementing
    // the formula here (where it would silently drift).
    let info = db.stats().unwrap().index.unwrap();
    let partitions = info.partitions;
    let effective_n_probe = info.n_probe;
    let effective_n_rerank = info.n_rerank;

    let mut recalls = Vec::with_capacity(QUERY_COUNT);
    let t0 = Instant::now();
    for (q, truth_ids) in queries.iter().zip(truth.iter()) {
        let req = RecallRequest::new(q.clone())
            .top_k(TOP_K)
            .track_access(false);
        // No .n_probe()/.n_rerank() — this is the whole point of the
        // test: measure what users get when they don't tune.
        let hits = db.recall(&req).unwrap();
        let got: HashSet<String> = hits.into_iter().map(|h| h.memory.id.to_string()).collect();
        let hit = truth_ids.intersection(&got).count();
        recalls.push(hit as f32 / TOP_K as f32);
    }
    let mean_secs = t0.elapsed().as_secs_f64() / QUERY_COUNT as f64;
    let mean_recall = recalls.iter().sum::<f32>() / QUERY_COUNT as f32;

    println!(
        "  {scenario:>14}  pq={pq_subspaces:>3}  partitions={partitions:>4}  \
         n_probe={effective_n_probe:>4}  n_rerank={effective_n_rerank:>4}  \
         recall@10={mean_recall:.3}  {:>7.2} ms/query  build={build_secs:.1}s",
        mean_secs * 1000.0,
    );

    json!({
        "scenario": scenario,
        "pq_subspaces": pq_subspaces,
        "partitions": partitions,
        "n_vectors": info.vectors,
        "n_probe_used": effective_n_probe,
        "n_rerank_used": effective_n_rerank,
        "recall_at_10": round4(mean_recall as f64),
        "mean_query_ms": round4(mean_secs * 1000.0),
        "build_secs": round4(build_secs),
    })
}

fn main() {
    // Tiers to measure. 1M is "if feasible" — the harness tries it if
    // the cached corpus is already on disk; otherwise it's skipped
    // with a note. Avoids the 20-minute first-build cost unless the
    // user already paid it in an earlier phase.
    let tiers_to_measure: Vec<usize> = [10_000usize, 100_000, 1_000_000]
        .iter()
        .filter(|&&n| {
            if n <= 100_000 {
                return true;
            }
            // 1M only if cached; otherwise skip + print.
            let p = cached_corpus_path(n);
            if p.exists() {
                true
            } else {
                println!(
                    "[ann-defaults] skipping 1M tier ({} not cached; label as unmeasured)",
                    p.display()
                );
                false
            }
        })
        .copied()
        .collect();

    let mut tiers_json: Vec<Value> = Vec::new();
    for n in tiers_to_measure {
        println!("\n[ann-defaults] tier={n}");
        let cached = ensure_corpus(n);
        let cells = vec![
            // Fresh install: new build defaults everywhere (dims/4
            // pq + auto probe + auto rerank).
            measure_scenario(&cached, "fresh_install", DIMENSIONS / 4, 0, 0),
            // Upgrade path: v0.5.x build left pq_subspaces = dims/8 =
            // 48 and stamped (8, 64) into the index. Simulate with
            // explicit (8, 64) — ANN-1.1's legacy sentinel detection
            // treats those as "auto" at query time.
            measure_scenario(&cached, "upgrade_path", DIMENSIONS / 8, 8, 64),
        ];
        tiers_json.push(json!({
            "corpus_size": n,
            "dimensions": DIMENSIONS,
            "scenarios": cells,
        }));
    }

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
    out["ann_defaults_v0_6_0"] = json!({
        "methodology":
            "Per-tier (10K, 100K, 1M-if-cached): rebuild the ANN index in \
             memory under two scenarios — 'fresh_install' = IndexConfig::default() \
             with pq_subspaces=dims/4 and ANN-1.1 query-time auto formulas; \
             'upgrade_path' = explicit (pq=dims/8, n_probe=8, n_rerank=64) to \
             simulate a v0.5.x-built index; ANN-1.1's legacy-sentinel detection \
             substitutes the new auto formulas at query time. 50 queries per \
             scenario, recall@10 against exact search, mean latency per query. \
             Index is rebuilt in memory only — cached corpus on disk is \
             untouched. See CHANGELOG `## [Unreleased]` → ANN-1.1.",
        "tiers": tiers_json,
    });
    fs::write(
        &perf_path,
        serde_json::to_string_pretty(&out).unwrap() + "\n",
    )
    .unwrap();
    println!("\n[ann-defaults] wrote {}", perf_path.display());
}

fn round4(x: f64) -> Value {
    Value::from((x * 10000.0).round() / 10000.0)
}
