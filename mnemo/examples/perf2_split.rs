//! PERF-2 (v0.19.0): where a fresh handle's first recall goes on an indexed
//! file. For each of `--cycles` fresh handles it loads the index through the
//! dev hook `Mnemo::__index_load_split` (read and decrypt, decode, the `id
//! -> partition` map, the delta chain, the reconcile), then times the first
//! recall that follows. A second set of fresh handles times a plain first
//! recall for comparison. Medians are printed as JSON.
//!
//! ```text
//! cargo run --release --features benches --example device_budget -- --sizes 100000   # builds the corpus
//! cargo run --release --features benches --example perf2_split -- --corpus target/device_budget/corpus_100000_384d_300b_v9.mnemo
//! # the same index rewritten in this build's layout, measured on a copy:
//! cargo run --release --features benches --example perf2_split -- --corpus <file> --rewrite-to /tmp/copy.mnemo
//! ```

use std::collections::BTreeMap;
use std::time::Instant;

use mnemo_engine::{Mnemo, RecallRequest};

#[path = "../benches/corpus.rs"]
mod corpus;
use corpus::{gen_clustered_vectors, DIMENSIONS, PASSPHRASE};

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1).cloned())
    };
    let corpus = arg("--corpus").expect("--corpus <file>");
    // `--rewrite-to <file>`: copy the corpus there and rewrite its index base
    // in this build's layout (no retraining), then measure the copy.
    let path = match arg("--rewrite-to") {
        Some(dst) => {
            std::fs::copy(&corpus, &dst).expect("copy the corpus");
            let mut db = Mnemo::open(&dst, PASSPHRASE).expect("open the copy");
            db.__rewrite_index_base_for_testing()
                .expect("rewrite the index");
            db.close().expect("close");
            dst
        }
        None => corpus,
    };
    let cycles: usize = arg("--cycles").and_then(|v| v.parse().ok()).unwrap_or(20);
    let queries = gen_clustered_vectors(cycles, DIMENSIONS, 100, 0xDE71_CE00);
    let req = |q: &[f32]| RecallRequest::new(q.to_vec()).top_k(10).track_access(false);

    let mut stages: BTreeMap<&'static str, Vec<f64>> = BTreeMap::new();
    for q in &queries {
        let mut db = Mnemo::open(&path, PASSPHRASE).expect("open");
        let split = db.__index_load_split().expect("split");
        assert!(!split.is_empty(), "the file has no unloaded v9 index");
        for (k, v) in split {
            stages.entry(k).or_default().push(v);
        }
        let t = Instant::now();
        std::hint::black_box(db.recall(&req(q)).expect("recall"));
        stages
            .entry("recall_after_load_ms")
            .or_default()
            .push(t.elapsed().as_secs_f64() * 1e3);
    }
    let mut plain = Vec::new();
    for q in &queries {
        let mut db = Mnemo::open(&path, PASSPHRASE).expect("open");
        let t = Instant::now();
        std::hint::black_box(db.recall(&req(q)).expect("recall"));
        plain.push(t.elapsed().as_secs_f64() * 1e3);
    }
    let mut out = serde_json::Map::new();
    for (k, v) in stages {
        out.insert(k.into(), serde_json::json!(median(v)));
    }
    out.insert(
        "first_recall_plain_ms".into(),
        serde_json::json!(median(plain)),
    );
    out.insert("cycles".into(), serde_json::json!(cycles));
    println!("{}", serde_json::Value::Object(out));
}
