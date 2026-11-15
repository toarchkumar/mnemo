//! 12.3 step 5: the dev-only retrieval check of the built-in embedder, on
//! this project's brain, through the shipped code path. Behind
//! `embed-candle`.
//!
//! ```sh
//! cargo run --release --features embed-candle --example candle_hard -- \
//!     <repo> <out.json> [<model dir>]
//! ```
//!
//! On a copy of `test/project.mnemo` (the passphrase read from `test/.env`
//! in-process; the copy deleted after), moved to the built-in with
//! `Mnemo::reembed_file`, every question of `questions_hard.json` and
//! `questions.json` (in `test/scripts/model_compare/`) is asked two ways:
//! - **shipped**: `RecallRequest::from_text` on a handle with the built-in
//!   registered, as the CLI, MCP and Python ask it, so the engine's
//!   identifier-aware recall chooses the keywords;
//! - **vector**: the same with `keywords("")`, for reference.
//!
//! The model comes from the given directory, or from the cache
//! (`BuiltinEmbedder::load`, which never downloads). The groups' rules, as
//! in part 3:
//! - **pair**: a hit needs both facts in the top 5 (each snippet names one
//!   memory); its rank is the worse of the two;
//! - **current**: a miss when the stale fact ranks above the current one.

use std::path::{Path, PathBuf};

use mnemo_engine::{BuiltinEmbedder, Memory, Mnemo, RecallRequest, Ulid};
use serde_json::{json, Value};

const AGENT: &str = "cursor-agent";

fn passphrase(repo: &Path) -> String {
    let text = std::fs::read_to_string(repo.join("test/.env")).expect("read test/.env");
    text.lines()
        .find_map(|l| l.trim().strip_prefix("MNEMO_PASSPHRASE="))
        .map(|v| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
        .expect("test/.env has no MNEMO_PASSPHRASE")
}

fn ids_for(all: &[Memory], snippets: &[Value]) -> Vec<Ulid> {
    let s: Vec<&str> = snippets.iter().map(|x| x.as_str().unwrap()).collect();
    all.iter()
        .filter(|m| s.iter().any(|x| m.content.contains(x)))
        .map(|m| m.id)
        .collect()
}

/// One snippet, one memory id.
fn one_id(all: &[Memory], snippet: &Value) -> Ulid {
    let s = snippet.as_str().unwrap();
    let hits: Vec<Ulid> = all
        .iter()
        .filter(|m| m.content.contains(s))
        .map(|m| m.id)
        .collect();
    assert_eq!(hits.len(), 1, "{s:?}");
    hits[0]
}

fn load_model(dir: Option<&String>) -> BuiltinEmbedder {
    match dir {
        Some(d) => BuiltinEmbedder::from_dir(Path::new(d)).unwrap(),
        None => BuiltinEmbedder::load().unwrap(),
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let repo = PathBuf::from(&a[1]);
    let out_path = PathBuf::from(&a[2]);
    let pass = passphrase(&repo);
    let work = tempfile::tempdir().unwrap();
    let load = |f: &str| -> Vec<Value> {
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(repo.join("test/scripts/model_compare").join(f)).unwrap(),
        )
        .unwrap();
        v["questions"].as_array().unwrap().clone()
    };
    let sets = [
        ("hard", load("questions_hard.json")),
        ("original", load("questions.json")),
    ];

    let copy = work.path().join("brain.mnemo");
    std::fs::copy(repo.join("test/project.mnemo"), &copy).unwrap();
    let model = load_model(a.get(3));
    assert!(model.is_builtin(), "not the pinned model");
    Mnemo::reembed_file(copy.to_str().unwrap(), &pass, &model).unwrap();
    let mut db = Mnemo::open_read_only(&copy, &pass).unwrap();
    db.set_embedder(load_model(a.get(3))).unwrap();
    let embedder = db.embedder_arc().unwrap();
    let all = db.memories().unwrap();

    let mut rows = Vec::new();
    for (set, qs) in &sets {
        for q in qs {
            let group = q
                .get("group")
                .or_else(|| q.get("kind"))
                .and_then(Value::as_str)
                .unwrap()
                .to_string();
            let expected: Vec<Ulid> = if group == "pair" {
                q["expected"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| one_id(&all, s))
                    .collect()
            } else {
                ids_for(&all, q["expected"].as_array().unwrap())
            };
            let stale: Vec<Ulid> = q
                .get("stale")
                .and_then(Value::as_array)
                .map(|s| ids_for(&all, s))
                .unwrap_or_default();
            let text = q["question"].as_str().unwrap();
            let base = RecallRequest::from_text(embedder.as_ref(), text)
                .unwrap()
                .top_k(10)
                .agent(AGENT)
                .track_access(false);
            let keywords = embedder.query_keywords(text).unwrap_or_default();
            let mut per_mode = serde_json::Map::new();
            for (mode, req) in [("shipped", base.clone()), ("vector", base.keywords(""))] {
                let hits = db.recall(&req).unwrap();
                let pos = |id: &Ulid| hits.iter().position(|h| h.memory.id == *id).map(|i| i + 1);
                let rank = if group == "pair" {
                    let r: Vec<Option<usize>> = expected.iter().map(pos).collect();
                    if r.iter().all(Option::is_some) {
                        r.into_iter().flatten().max()
                    } else {
                        None
                    }
                } else {
                    expected.iter().filter_map(pos).min()
                };
                let stale_rank = stale.iter().filter_map(pos).min();
                let stale_won = matches!((stale_rank, rank), (Some(s), Some(r)) if s < r)
                    || (stale_rank.is_some() && rank.is_none());
                let rank = if stale_won { None } else { rank };
                per_mode.insert(
                    mode.to_string(),
                    json!({"rank": rank, "stale_rank": stale_rank, "stale_won": stale_won}),
                );
            }
            rows.push(json!({
                "set": set, "id": q["id"], "group": group, "keywords": keywords, "modes": per_mode,
            }));
        }
    }
    drop(db);
    std::fs::remove_file(&copy).unwrap();

    // The summary, per set and per group: hit@1, hit@5 and MRR (top 10).
    let mut summary = serde_json::Map::new();
    for mode in ["shipped", "vector"] {
        for set in ["hard", "original"] {
            let mut groups: Vec<String> = vec!["all".into()];
            for r in &rows {
                let g = r["group"].as_str().unwrap().to_string();
                if r["set"] == set && !groups.contains(&g) {
                    groups.push(g);
                }
            }
            for g in groups {
                let ranks: Vec<Option<u64>> = rows
                    .iter()
                    .filter(|r| r["set"] == set && (g == "all" || r["group"] == g.as_str()))
                    .map(|r| r["modes"][mode]["rank"].as_u64())
                    .collect();
                let n = ranks.len();
                let at = |k: u64| ranks.iter().filter(|r| r.is_some_and(|r| r <= k)).count();
                let mrr: f64 = ranks
                    .iter()
                    .map(|r| r.map_or(0.0, |r| 1.0 / r as f64))
                    .sum::<f64>()
                    / n as f64;
                let stale_won = rows
                    .iter()
                    .filter(|r| r["set"] == set && (g == "all" || r["group"] == g.as_str()))
                    .filter(|r| r["modes"][mode]["stale_won"] == true)
                    .count();
                let line = format!(
                    "{mode:8} {set:9} {g:12} n={n:2} hit@1={:2} hit@5={:2} MRR={mrr:.3} stale_won={stale_won}",
                    at(1),
                    at(5)
                );
                println!("{line}");
                summary.insert(
                    format!("{mode}/{set}/{g}"),
                    json!({"n": n, "hit1": at(1), "hit5": at(5), "mrr": mrr, "stale_won": stale_won}),
                );
            }
        }
    }
    std::fs::write(
        &out_path,
        serde_json::to_string_pretty(&json!({"summary": summary, "rows": rows})).unwrap(),
    )
    .unwrap();
    eprintln!("wrote {}", out_path.display());
}
