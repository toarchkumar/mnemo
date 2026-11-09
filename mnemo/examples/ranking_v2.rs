//! Scoring v2 ranking harness (shipped with v0.7.0, maintained after).
//!
//! Loads a JSON fixture produced by `test/scripts/gen_ranking_v2_fixture.py`
//! (Python only embeds, Rust runs everything else — see the 2026-11-05
//! PLANNER DECISION), builds a backdated `.mnemo` file through the public
//! API (no new timestamp-setter — we just assign to the already-pub
//! `Memory.created_at` / `Memory.accessed_at` fields before `remember`),
//! runs the shipped scoring formula through a `track_access=false` pass
//! *and* a `track_access=true` 5-pass usage loop, and writes a block under
//! `ranking_v2` in `test/scripts/perf_v0.5.0.json`.
//!
//! Two rows are reported in every run:
//! * `default` — the shipped scoring formula on the v2 fixture.
//! * `similarity_only` — β = γ = δ = 0, i.e. `α·sim` alone. Kept as a
//!   permanent baseline per the 2026-11-06 PLANNER DECISION so the
//!   next release can see whether the corpus has drifted to a point
//!   where similarity alone solves it.
//!
//! Two pre-release gating checks are also recorded:
//! * `similarity_only_on_v2_fixture` — if this is hit@1 = 1.0, the v2
//!   fixture has become trivially solvable by similarity and the "the
//!   scorer still matters" claim no longer holds. BLOCKED.
//! * `default_on_phase6_fixture` — the shipped formula on the Phase 6
//!   corpus must stay within 0.02 of V0's shipped hit@1 (≥ 0.5467) and
//!   MRR (≥ 0.6665). Below either → BLOCKED: we've regressed on the
//!   pre-scoring-v2 corpus.
//!
//! Correctness notes:
//! * Each fixture memory ships with a pre-computed embedding (same
//!   model the Phase 6 harness uses), so the measurement is
//!   deterministic regardless of local MiniLM version drift.
//! * Backdating happens by setting `m.created_at = now - offset;
//!   m.accessed_at = now - offset;` before `remember`. The store's
//!   `put` preserves the caller's fields verbatim on a new catalog
//!   row (store.rs), so no new API is needed (verified by the
//!   2026-11-05 PLANNER DECISION addendum).
//! * Labels are not persisted on `Memory`; the harness keeps a
//!   side map `label → ulid` and translates every `RecallResult`
//!   back to a label for scoring.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use mnemo_engine::{
    KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, RecallRequest, Result as MnemoResult,
    ScoreWeights, Ulid,
};

#[derive(Parser, Debug)]
#[command(
    about = "Scoring v2 ranking harness: shipped formula + similarity-only \
             baseline + phase6 regression gate."
)]
struct Args {
    /// Path to the v2 fixture JSON produced by gen_ranking_v2_fixture.py.
    #[arg(long, default_value = "test/scripts/ranking_v2_fixture.json")]
    fixture: String,
    /// Path to the Phase 6 fixture, used by the regression gate.
    #[arg(long, default_value = "test/scripts/ranking_v2_phase6_fixture.json")]
    phase6_fixture: String,
    /// Path to the perf-results JSON the run writes into.
    #[arg(long, default_value = "test/scripts/perf_v0.5.0.json")]
    perf_json: String,
    /// Number of passes in the `track_access=true` usage loop.
    #[arg(long, default_value_t = 5)]
    usage_loop_passes: usize,
    /// Top-K for the recall (fixed at 10 to match Phase 6).
    #[arg(long, default_value_t = 10)]
    top_k: usize,
    /// Default on the Phase 6 fixture must clear this hit@1 threshold
    /// (= Phase 6 V0 hit@1 − 0.02). Below it → BLOCKED.
    #[arg(long, default_value_t = 0.5467)]
    phase6_min_hit_at_1: f64,
    /// Default on the Phase 6 fixture must clear this MRR threshold
    /// (= Phase 6 V0 MRR − 0.02). Below it → BLOCKED.
    #[arg(long, default_value_t = 0.6665)]
    phase6_min_mrr: f64,
}

// ------------------------------------------------------------------
// Fixture types (mirror `gen_ranking_v2_fixture.py`'s shape)
// ------------------------------------------------------------------

#[derive(Deserialize)]
struct Fixture {
    meta: Value,
    memories: Vec<FixMem>,
    queries: Vec<FixQuery>,
}

#[derive(Deserialize, Clone)]
struct FixMem {
    label: String,
    content: String,
    importance: f32,
    created_at_offset_secs: i64,
    agent_id: String,
    vector: Vec<f32>,
}

#[derive(Deserialize, Clone)]
struct FixQuery {
    // Query text: the keyword query under `RANKING_TEXT_WEIGHT` (FMT-9d
    // hybrid recall), and the "name the query every run misses" callout.
    text: String,
    category: String,
    expected_labels: Vec<String>,
    vector: Vec<f32>,
}

// ------------------------------------------------------------------
// Metrics
// ------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct CatMetrics {
    n: usize,
    hit_at_1: f64,
    hit_at_5: f64,
    mrr: f64,
}

#[derive(Debug, Clone, Serialize)]
struct PassMetrics {
    hit_at_1: f64,
    hit_at_5: f64,
    mrr: f64,
    per_category: BTreeMap<String, CatMetrics>,
    /// Top-1 label per query (same order as fixture.queries), for drift.
    top1_labels: Vec<Option<String>>,
}

fn metrics_from_ranks(queries: &[FixQuery], ranks: &[Option<usize>], top_k: usize) -> PassMetrics {
    assert_eq!(queries.len(), ranks.len());
    let n = queries.len();
    let mut hit1 = 0;
    let mut hit5 = 0;
    let mut mrr_sum = 0.0f64;
    let mut per_cat_hits: HashMap<&str, (usize, usize, usize, f64)> = HashMap::new();
    for (q, r) in queries.iter().zip(ranks.iter()) {
        let (cn, c1, c5, cmrr) = per_cat_hits
            .entry(q.category.as_str())
            .or_insert((0, 0, 0, 0.0));
        *cn += 1;
        match r {
            Some(rank) if *rank <= top_k => {
                if *rank == 1 {
                    hit1 += 1;
                    *c1 += 1;
                }
                if *rank <= 5 {
                    hit5 += 1;
                    *c5 += 1;
                }
                let inv = 1.0 / (*rank as f64);
                mrr_sum += inv;
                *cmrr += inv;
            }
            _ => {}
        }
    }
    let per_category = per_cat_hits
        .into_iter()
        .map(|(cat, (cn, c1, c5, cmrr))| {
            (
                cat.to_string(),
                CatMetrics {
                    n: cn,
                    hit_at_1: round4(c1 as f64 / cn as f64),
                    hit_at_5: round4(c5 as f64 / cn as f64),
                    mrr: round4(cmrr / cn as f64),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    PassMetrics {
        hit_at_1: round4(hit1 as f64 / n as f64),
        hit_at_5: round4(hit5 as f64 / n as f64),
        mrr: round4(mrr_sum / n as f64),
        per_category,
        top1_labels: Vec::new(), // filled by caller
    }
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

// ------------------------------------------------------------------
// Fixture → .mnemo
// ------------------------------------------------------------------

/// FMT-9a: symmetric per-vector int8 quantization and back
/// (`q = round(127 · v / max|v|)`, `v' = q · max|v| / 127`).
fn int8_roundtrip(v: &[f32]) -> Vec<f32> {
    let max = v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    if max == 0.0 {
        return v.to_vec();
    }
    let scale = max / 127.0;
    v.iter()
        .map(|x| (x / scale).round().clamp(-127.0, 127.0) * scale)
        .collect()
}

/// Build a fresh database from the fixture, return (handle, label→ulid map).
fn build_db(fixture: &Fixture, now: i64) -> MnemoResult<(Mnemo, HashMap<String, Ulid>)> {
    let tmp = tempfile::Builder::new()
        .prefix("ranking_v2_")
        .suffix(".mnemo")
        .tempfile()
        .expect("tempfile");
    let path: PathBuf = tmp.path().to_path_buf();
    drop(tmp); // let Mnemo::create own the file

    let dims = fixture.memories[0].vector.len();
    let cfg = MnemoConfig {
        dimensions: dims,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    let mut db = Mnemo::create(&path, "harness-pw", cfg)?;

    // FMT-9a: `RANKING_QUANTIZE_INT8=1` stores every memory vector as a v9
    // int8 column would hold it (symmetric per-vector quantization, then
    // dequantized), so the harness measures int8 rerank against float.
    // Queries stay float, as they would in v9.
    let quantize = std::env::var("RANKING_QUANTIZE_INT8").is_ok_and(|v| v == "1");
    let mut labels: HashMap<String, Ulid> = HashMap::new();
    for m in &fixture.memories {
        let vector = if quantize {
            int8_roundtrip(&m.vector)
        } else {
            m.vector.clone()
        };
        let mut mem = Memory::new(m.content.clone(), MemoryType::Semantic, vector)
            .with_agent(&m.agent_id)
            .with_importance(m.importance);
        let backdated = now - m.created_at_offset_secs;
        mem.created_at = backdated;
        mem.accessed_at = backdated;
        let id = db.remember(mem)?;
        labels.insert(m.label.clone(), id);
    }
    db.flush()?;
    Ok((db, labels))
}

// ------------------------------------------------------------------
// Running a single pass
// ------------------------------------------------------------------

fn reverse_labels(labels: &HashMap<String, Ulid>) -> HashMap<Ulid, String> {
    labels.iter().map(|(k, v)| (*v, k.clone())).collect()
}

fn rank_of_expected(
    hits: &[(Ulid, f32)],
    expected: &[String],
    rev: &HashMap<Ulid, String>,
) -> Option<usize> {
    for (i, (id, _)) in hits.iter().enumerate() {
        if let Some(label) = rev.get(id) {
            if expected.contains(label) {
                return Some(i + 1);
            }
        }
    }
    None
}

fn run_pass(
    db: &mut Mnemo,
    fixture: &Fixture,
    labels: &HashMap<String, Ulid>,
    weights: ScoreWeights,
    track_access: bool,
    top_k: usize,
) -> MnemoResult<PassMetrics> {
    let rev = reverse_labels(labels);
    let mut ranks = Vec::with_capacity(fixture.queries.len());
    let mut top1 = Vec::with_capacity(fixture.queries.len());
    for q in &fixture.queries {
        let mut req = RecallRequest::new(q.vector.clone())
            .top_k(top_k)
            .weights(weights)
            .track_access(track_access);
        // FMT-9d: `RANKING_TEXT_WEIGHT=<eps>` (on a v9 file, via
        // `MNEMO_DEV_FORMAT=9`) runs hybrid recall: the query text as a
        // BM25 keyword query with weight eps.
        if let Some(w) = std::env::var("RANKING_TEXT_WEIGHT")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
        {
            req = req.keywords(q.text.clone()).keyword_weight(w);
        }
        let hits = db.recall(&req)?;
        let hit_ids: Vec<(Ulid, f32)> = hits.iter().map(|h| (h.memory.id, h.similarity)).collect();
        let rank = rank_of_expected(&hit_ids, &q.expected_labels, &rev);
        let top = hit_ids.first().and_then(|(id, _)| rev.get(id).cloned());
        ranks.push(rank);
        top1.push(top);
    }
    let mut m = metrics_from_ranks(&fixture.queries, &ranks, top_k);
    m.top1_labels = top1;
    Ok(m)
}

/// Build a fresh db and run one `track_access=false` pass with the given
/// weights. Used by the similarity-only baseline and the Phase 6 gate
/// where the measurement is one-shot.
fn run_single(
    fixture: &Fixture,
    now: i64,
    weights: ScoreWeights,
    top_k: usize,
) -> MnemoResult<PassMetrics> {
    let (mut db, labels) = build_db(fixture, now)?;
    run_pass(&mut db, fixture, &labels, weights, false, top_k)
}

/// Similarity-only baseline: β = γ = δ = 0 (so the recency, importance
/// and usage terms all go to zero and the score reduces to `α·sim`).
fn sim_only_weights() -> ScoreWeights {
    ScoreWeights {
        alpha: 1.0,
        beta: 0.0,
        gamma: 0.0,
        delta: 0.0,
        half_life_secs: 7.0 * 24.0 * 3600.0,
    }
}

fn drift_top1(first: &PassMetrics, last: &PassMetrics) -> usize {
    first
        .top1_labels
        .iter()
        .zip(last.top1_labels.iter())
        .filter(|(a, b)| a != b)
        .count()
}

// ------------------------------------------------------------------
// Default-weights run on the v2 fixture (track_false + usage loop).
// ------------------------------------------------------------------

#[derive(Serialize)]
struct DefaultRunReport {
    /// hit@1 / hit@5 / MRR with `track_access=false` (the clean ranking).
    track_false: PassMetrics,
    /// Per-pass metrics under the `usage_loop_passes`-pass tracking loop.
    track_true_passes: Vec<PassMetrics>,
    /// Count of queries whose top-1 label flipped between pass 1 and the
    /// last pass under tracking. Scoring v2 pins this at 0 by design
    /// (recency reads `created_at`, so recalls can't bump their own
    /// recency on later queries).
    drift_top1_pass1_to_last: usize,
}

fn run_default(
    fixture: &Fixture,
    passes: usize,
    top_k: usize,
    now: i64,
) -> MnemoResult<DefaultRunReport> {
    let weights = ScoreWeights::default();

    let (mut db, labels) = build_db(fixture, now)?;
    let track_false = run_pass(&mut db, fixture, &labels, weights, false, top_k)?;

    // Rebuild for the tracking loop so track_false's reads don't bleed
    // into pass-1 numbers.
    let (mut db, labels) = build_db(fixture, now)?;
    let mut track_true_passes = Vec::with_capacity(passes);
    for _ in 0..passes {
        let m = run_pass(&mut db, fixture, &labels, weights, true, top_k)?;
        track_true_passes.push(m);
    }
    let drift = drift_top1(&track_true_passes[0], track_true_passes.last().unwrap());
    eprintln!(
        "[ranking_v2] default: track_false MRR={:.4}, track_true pass1 MRR={:.4} → pass{} MRR={:.4}, drift={}",
        track_false.mrr,
        track_true_passes[0].mrr,
        passes,
        track_true_passes.last().unwrap().mrr,
        drift,
    );
    Ok(DefaultRunReport {
        track_false,
        track_true_passes,
        drift_top1_pass1_to_last: drift,
    })
}

// ------------------------------------------------------------------
// Entry point
// ------------------------------------------------------------------

fn fixture_now(_fixture: &Fixture) -> i64 {
    // We anchor "now" to the current wall clock. Backdating subtracts
    // the memory's created_at_offset from this value, so all recency
    // terms are consistent within a single harness run.
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_secs() as i64
}

fn load_fixture(path: &str) -> Fixture {
    let text = fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "fixture not found at '{}': {e}. Run `python test/scripts/gen_ranking_v2_fixture.py` first.",
            path
        )
    });
    serde_json::from_str(&text).expect("fixture JSON failed to parse")
}

fn main() -> MnemoResult<()> {
    let args = Args::parse();

    let fixture = load_fixture(&args.fixture);
    let now = fixture_now(&fixture);

    // Default scoring (track_false + usage loop).
    let default_report = run_default(&fixture, args.usage_loop_passes, args.top_k, now)?;

    // Similarity-only baseline on the v2 fixture. Permanent row per
    // 2026-11-06 PLANNER DECISION. Also serves as gate 1: if sim-only
    // already hits@1 = 1.0, the corpus has become trivially solvable
    // by similarity alone.
    let sim_only_v2 = run_single(&fixture, now, sim_only_weights(), args.top_k)?;
    eprintln!(
        "[ranking_v2] similarity_only: hit@1={:.4} hit@5={:.4} MRR={:.4}",
        sim_only_v2.hit_at_1, sim_only_v2.hit_at_5, sim_only_v2.mrr,
    );
    let sim_only_perfect = sim_only_v2.hit_at_1 >= 0.9999;
    if sim_only_perfect {
        eprintln!(
            "BLOCKED: similarity-only is already perfect at hit@1 on the v2 fixture — \
             the scoring bonus can't differentiate on a corpus a dot-product solves alone. \
             Add harder queries before the next release."
        );
        std::process::exit(2);
    }

    // Phase 6 regression gate. Default weights on the pre-scoring-v2
    // corpus must stay within 0.02 of V0's shipped hit@1 and MRR
    // (= 0.5467 / 0.6665).
    let phase6 = load_fixture(&args.phase6_fixture);
    let phase6_now = fixture_now(&phase6);
    let phase6_report = run_single(&phase6, phase6_now, ScoreWeights::default(), args.top_k)?;
    eprintln!(
        "[ranking_v2] phase6 regression: hit@1={:.4} hit@5={:.4} MRR={:.4}",
        phase6_report.hit_at_1, phase6_report.hit_at_5, phase6_report.mrr,
    );
    for (cat, m) in &phase6_report.per_category {
        eprintln!(
            "[ranking_v2]   phase6 {cat}: hit@1={:.4} MRR={:.4}",
            m.hit_at_1, m.mrr
        );
    }
    let phase6_fail_hit1 = phase6_report.hit_at_1 < args.phase6_min_hit_at_1;
    let phase6_fail_mrr = phase6_report.mrr < args.phase6_min_mrr;
    if phase6_fail_hit1 || phase6_fail_mrr {
        eprintln!(
            "BLOCKED: default on phase6 fixture: hit@1={:.4} (threshold {}), MRR={:.4} (threshold {}). \
             Scoring regressed on the pre-scoring-v2 corpus below the 0.02 tolerance gate.",
            phase6_report.hit_at_1,
            args.phase6_min_hit_at_1,
            phase6_report.mrr,
            args.phase6_min_mrr,
        );
        std::process::exit(2);
    }

    // Merge into perf JSON under the `ranking_v2` key.
    let mut perf: Value =
        serde_json::from_str(&fs::read_to_string(&args.perf_json).expect("read perf"))
            .expect("parse perf");
    let block = json!({
        "generator": "mnemo/examples/ranking_v2.rs",
        "fixture_meta": fixture.meta,
        "usage_loop_passes": args.usage_loop_passes,
        "top_k": args.top_k,
        "corpus_size": fixture.memories.len(),
        "query_count": fixture.queries.len(),
        "default": serde_json::to_value(&default_report).expect("serialize default"),
        "similarity_only": serde_json::to_value(&sim_only_v2).expect("serialize sim_only_v2"),
        "phase6_regression": serde_json::to_value(&phase6_report).expect("serialize phase6"),
        "phase6_min_hit_at_1": args.phase6_min_hit_at_1,
        "phase6_min_mrr": args.phase6_min_mrr,
        "notes": "default = ScoreWeights::default() (scoring v2 shipped). \
                  similarity_only = β=γ=δ=0 baseline, kept as a permanent row \
                  per the 2026-11-06 Planner DECISION so the next release can \
                  tell if the corpus has become trivially similarity-solvable. \
                  phase6_regression = default scoring run on the pre-scoring-v2 \
                  corpus; gates hit@1 ≥ 0.5467 and MRR ≥ 0.6665 (V0 shipped − 0.02). \
                  Scoring v2 is on a new scale from the pre-v0.7.0 formula: raw \
                  scores sit on the cosine-similarity scale plus small gated \
                  bonuses; threshold-based callers need to re-check their cutoffs.",
    });
    perf.as_object_mut()
        .expect("perf object")
        .insert("ranking_v2".into(), block);
    fs::write(
        &args.perf_json,
        serde_json::to_string_pretty(&perf).expect("serialize") + "\n",
    )
    .expect("write perf");
    eprintln!("[ranking_v2] wrote ranking_v2 block to {}", args.perf_json);
    Ok(())
}
