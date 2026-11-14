//! FMT-9a prototype measurement: the numbers the v9 format design note
//! projected, measured before the format is frozen. No format change; it
//! reads the v8 corpora `device_budget` builds and simulates v9 layouts.
//!
//! ```text
//! cargo run --release --example device_budget --features benches   # builds the corpora once
//! cargo run --release --example fmt9_proto --features benches,fmt9_proto
//! cargo run --release --example fmt9_proto --features benches -- --part int8
//! ```
//!
//! Parts (`--part`, default all): `packing` (records + vector column +
//! catalog rows), `int8` (int8 against float rerank: recall@10, plus scan
//! timings for the ANN threshold), `bm25` (index size on real text),
//! `threads` (10K facts + 50K turns), `compress` (lz4_flex against zstd on
//! packed record pages; needs `--features fmt9_proto`). Results go to
//! `target/fmt9_proto.json`.
//!
//! Real text for BM25, compression and turns comes from this repo's own
//! Markdown (PLAN.md, CHANGELOG.md, READMEs): English prose dense with
//! identifiers, like an agent's memory. No transcripts are read.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use mnemo_engine::{Memory, MemoryType, Mnemo};
use serde_json::{json, Value};

#[path = "../benches/corpus.rs"]
mod corpus;
use corpus::{gen_clustered_vectors, DIMENSIONS, PASSPHRASE};

const PAGE: usize = 8192;
/// Encrypted payload of a page (minus nonce and tag), as in v8.
const PAYLOAD: usize = PAGE - 12 - 16;
/// Projected v9 slotted-page header and per-slot directory entry.
const PAGE_HEADER: usize = 16;
const SLOT: usize = 4;
/// v9 int8 vector entry: d bytes + scale f32 + norm f32.
fn vec_entry(d: usize) -> usize {
    d + 8
}

fn corpus_path(n: usize) -> PathBuf {
    let base = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_string());
    PathBuf::from(base)
        .join("device_budget")
        .join(format!("corpus_{n}_{DIMENSIONS}d_300b.mnemo"))
}

fn open_corpus(n: usize) -> Mnemo {
    let p = corpus_path(n);
    assert!(
        p.exists(),
        "{} is missing: run `cargo run --release --example device_budget --features benches` once",
        p.display()
    );
    Mnemo::open_read_only(&p, PASSPHRASE).expect("open corpus")
}

// --- page packing ------------------------------------------------------

/// Pages a v9 slotted layout needs for records of these byte sizes, packed
/// greedily in order. A record too big for one page becomes a span over a
/// contiguous run (its slot stays in an ordinary page).
fn pack_pages(sizes: impl Iterator<Item = usize>) -> (usize, usize) {
    let cap = PAYLOAD - PAGE_HEADER;
    let (mut pages, mut span_pages, mut used) = (0usize, 0usize, cap);
    for len in sizes {
        let (inline, span) = if len + SLOT > cap {
            (SLOT + 12, len.div_ceil(PAYLOAD))
        } else {
            (len + SLOT, 0)
        };
        span_pages += span;
        if used + inline > cap {
            pages += 1;
            used = 0;
        }
        used += inline;
    }
    (pages, span_pages)
}

fn rmp_len<T: serde::Serialize>(v: &T) -> usize {
    rmp_serde::to_vec(v).expect("serialize").len()
}

/// The projected v9 catalog row, serialized like v8's (rmp, positional).
#[derive(serde::Serialize)]
struct V9Row {
    id: u128,
    rec_page: u64,
    rec_slot: u16,
    rec_gen: u64,
    vec_page: u64,
    vec_slot: u16,
    vec_gen: u64,
    kind: u8,
    flags: u8,
    thread_hash: u32,
    key_hash: u32,
    accessed_at: i64,
    access_count: u32,
}

fn part_packing(sizes_n: &[usize]) -> Value {
    let mut out = serde_json::Map::new();
    for &n in sizes_n {
        let mut db = open_corpus(n);
        let mems = db.memories().expect("memories");
        let v8: Vec<usize> = mems.iter().map(rmp_len).collect();
        let v9: Vec<usize> = mems
            .iter()
            .map(|m| {
                let mut r = m.clone();
                r.vector = Vec::new();
                rmp_len(&r)
            })
            .collect();
        let (rec_pages, span_pages) = pack_pages(v9.iter().copied());
        let per_vec_page = (PAYLOAD - PAGE_HEADER) / vec_entry(DIMENSIONS);
        let vec_pages = n.div_ceil(per_vec_page);
        let txn = 1_000u64 + n as u64;
        let row_bytes: usize = mems
            .iter()
            .enumerate()
            .map(|(i, m)| {
                rmp_len(&V9Row {
                    id: m.id.0,
                    rec_page: (i / 20) as u64 + 2,
                    rec_slot: (i % 20) as u16,
                    rec_gen: txn,
                    vec_page: (i / per_vec_page) as u64 + 2 + rec_pages as u64,
                    vec_slot: (i % per_vec_page) as u16,
                    vec_gen: txn,
                    kind: 0,
                    flags: 0,
                    thread_hash: 0,
                    key_hash: 0,
                    accessed_at: m.accessed_at,
                    access_count: m.access_count,
                })
            })
            .sum();
        let avg = |v: &[usize]| v.iter().sum::<usize>() as f64 / v.len() as f64;
        let rec_bytes = (rec_pages + span_pages) * PAGE;
        let live_record_bytes: usize = v9.iter().sum();
        out.insert(
            n.to_string(),
            json!({
                "v8_record_avg_bytes": avg(&v8),
                "v9_record_avg_bytes": avg(&v9),
                "record_pages": rec_pages,
                "span_pages": span_pages,
                "records_per_page": n as f64 / rec_pages.max(1) as f64,
                "record_bytes_per_memory": rec_bytes as f64 / n as f64,
                "record_slack_pct": 100.0 * (1.0 - (live_record_bytes + n * SLOT) as f64 / rec_bytes as f64),
                "vector_bytes_per_memory": (vec_pages * PAGE) as f64 / n as f64,
                "catalog_row_avg_bytes": row_bytes as f64 / n as f64,
            }),
        );
        eprintln!(
            "[packing] n={n}: v8 record {:.0} B → v9 {:.0} B; {:.1}/page; records {:.0} B/mem, vectors {:.0} B/mem, row {:.0} B",
            avg(&v8),
            avg(&v9),
            n as f64 / rec_pages.max(1) as f64,
            rec_bytes as f64 / n as f64,
            (vec_pages * PAGE) as f64 / n as f64,
            row_bytes as f64 / n as f64
        );
    }
    Value::Object(out)
}

// --- int8 vectors ------------------------------------------------------

struct Quantized {
    q: Vec<i8>,
    scale: Vec<f32>,
    norm: Vec<f32>,
    d: usize,
}

fn quantize(vs: &[Vec<f32>]) -> Quantized {
    let d = vs[0].len();
    let mut q = Vec::with_capacity(vs.len() * d);
    let (mut scale, mut norm) = (Vec::new(), Vec::new());
    for v in vs {
        let max = v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let s = if max == 0.0 { 1.0 } else { max / 127.0 };
        q.extend(v.iter().map(|x| (x / s).round().clamp(-127.0, 127.0) as i8));
        scale.push(s);
        norm.push(v.iter().map(|x| x * x).sum::<f32>().sqrt());
    }
    Quantized { q, scale, norm, d }
}

/// Asymmetric dot: float query against an int8 vector. Written as a
/// plain zip so it auto-vectorizes (NEON: sxtl + scvtf + fmla).
#[inline(never)]
fn dot_i8_asym(q: &[f32], v: &[i8]) -> f32 {
    let mut acc = [0.0f32; 8];
    for (qc, vc) in q.chunks_exact(8).zip(v.chunks_exact(8)) {
        for ((a, &x), &y) in acc.iter_mut().zip(qc).zip(vc) {
            *a += x * y as f32;
        }
    }
    acc.iter().sum()
}

/// Symmetric dot: int8 query against int8 vector, i32 accumulate (NEON:
/// smull/smlal or sdot where available).
#[inline(never)]
fn dot_i8_sym(q: &[i8], v: &[i8]) -> i32 {
    let mut acc = [0i32; 16];
    for (qc, vc) in q.chunks_exact(16).zip(v.chunks_exact(16)) {
        for ((a, &x), &y) in acc.iter_mut().zip(qc).zip(vc) {
            *a += x as i32 * y as i32;
        }
    }
    acc.iter().sum()
}

/// Round an f32 to the nearest IEEE half-precision value (10-bit mantissa,
/// round-half-up on the dropped bits), kept as f32. Embedding components
/// sit well inside fp16's normal range, so subnormals and overflow don't
/// arise; this is what a stored fp16 column would give back.
fn f16_round(x: f32) -> f32 {
    f32::from_bits((x.to_bits().wrapping_add(0x1000)) & 0xFFFF_E000)
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (mut d, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        d += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        d / (na.sqrt() * nb.sqrt())
    }
}

fn top_k(scores: impl Iterator<Item = (usize, f32)>, k: usize) -> Vec<usize> {
    let mut v: Vec<(usize, f32)> = scores.collect();
    if v.len() > k {
        v.select_nth_unstable_by(k - 1, |a, b| b.1.total_cmp(&a.1));
        v.truncate(k);
    }
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v.into_iter().map(|(i, _)| i).collect()
}

fn recall_at(found: &[usize], truth: &[usize]) -> f64 {
    let t: HashSet<usize> = truth.iter().copied().collect();
    found.iter().filter(|i| t.contains(i)).count() as f64 / truth.len() as f64
}

fn part_int8(sizes_n: &[usize]) -> Value {
    let queries = gen_clustered_vectors(200, DIMENSIONS, 100, 0xDE71_CE00);
    let mut out = serde_json::Map::new();
    for &n in sizes_n {
        let mut db = open_corpus(n);
        let mems = db.memories().expect("memories");
        let row_of: HashMap<u128, usize> =
            mems.iter().enumerate().map(|(i, m)| (m.id.0, i)).collect();
        let vs: Vec<Vec<f32>> = mems.iter().map(|m| m.vector.clone()).collect();
        let qz = quantize(&vs);
        let d = qz.d;
        let int8_cos = |qv: &[f32], qn: f32, i: usize| {
            dot_i8_asym(qv, &qz.q[i * d..(i + 1) * d]) * qz.scale[i] / (qn * qz.norm[i]).max(1e-12)
        };
        let (mut r_full_i8, mut r_full_sym) = (0.0, 0.0);
        let (mut r_ann_f, mut r_ann_i8) = (0.0, 0.0);
        let (mut r_ann_f16, mut r_full_f16) = (0.0, 0.0);
        let vs16: Vec<Vec<f32>> = vs
            .iter()
            .map(|v| v.iter().map(|&x| f16_round(x)).collect())
            .collect();
        let mut shortlist_len = 0usize;
        for qv in &queries {
            let qn = qv.iter().map(|x| x * x).sum::<f32>().sqrt();
            let truth = top_k((0..n).map(|i| (i, cosine(qv, &vs[i]))), 10);
            r_full_i8 += recall_at(&top_k((0..n).map(|i| (i, int8_cos(qv, qn, i))), 10), &truth);
            let qmax = qv.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            let qs = qmax / 127.0;
            let q8: Vec<i8> = qv.iter().map(|x| (x / qs).round() as i8).collect();
            r_full_sym += recall_at(
                &top_k(
                    (0..n).map(|i| {
                        let dot = dot_i8_sym(&q8, &qz.q[i * d..(i + 1) * d]) as f32;
                        (i, dot * qs * qz.scale[i] / (qn * qz.norm[i]).max(1e-12))
                    }),
                    10,
                ),
                &truth,
            );
            let short: Vec<usize> = db
                .__ann_shortlist(qv, None, None)
                .expect("shortlist")
                .iter()
                .filter_map(|id| row_of.get(&id.0).copied())
                .collect();
            shortlist_len += short.len();
            r_ann_f += recall_at(
                &top_k(short.iter().map(|&i| (i, cosine(qv, &vs[i]))), 10),
                &truth,
            );
            r_ann_i8 += recall_at(
                &top_k(short.iter().map(|&i| (i, int8_cos(qv, qn, i))), 10),
                &truth,
            );
            r_ann_f16 += recall_at(
                &top_k(short.iter().map(|&i| (i, cosine(qv, &vs16[i]))), 10),
                &truth,
            );
            r_full_f16 += recall_at(
                &top_k((0..n).map(|i| (i, cosine(qv, &vs16[i]))), 10),
                &truth,
            );
        }
        let nq = queries.len() as f64;
        // Exact-scan timings over the first m vectors, for the ANN threshold.
        let mut scan = serde_json::Map::new();
        for m in [10_000usize, 20_000, 50_000, 100_000] {
            if m > n {
                continue;
            }
            let q = &queries[0];
            let qn = q.iter().map(|x| x * x).sum::<f32>().sqrt();
            let reps = 20;
            let t = Instant::now();
            for _ in 0..reps {
                std::hint::black_box(top_k((0..m).map(|i| (i, int8_cos(q, qn, i))), 10));
            }
            let asym_ms = t.elapsed().as_secs_f64() * 1e3 / reps as f64;
            let qmax = q.iter().fold(0.0f32, |a, x| a.max(x.abs()));
            let q8: Vec<i8> = q
                .iter()
                .map(|x| (x / (qmax / 127.0)).round() as i8)
                .collect();
            let t = Instant::now();
            for _ in 0..reps {
                std::hint::black_box(top_k(
                    (0..m).map(|i| (i, dot_i8_sym(&q8, &qz.q[i * d..(i + 1) * d]) as f32)),
                    10,
                ));
            }
            let sym_ms = t.elapsed().as_secs_f64() * 1e3 / reps as f64;
            scan.insert(
                m.to_string(),
                json!({ "asym_ms": asym_ms, "sym_ms": sym_ms, "phone_estimate_asym_ms": asym_ms * 4.0, "phone_estimate_sym_ms": sym_ms * 4.0 }),
            );
            eprintln!("[int8] exact int8 scan of {m}: asym {asym_ms:.2} ms, sym {sym_ms:.2} ms (phone ×4: {:.1} / {:.1})", asym_ms * 4.0, sym_ms * 4.0);
        }
        out.insert(
            n.to_string(),
            json!({
                "queries": queries.len(),
                "exact_scan_recall_at_10": { "int8_asymmetric": r_full_i8 / nq, "int8_symmetric": r_full_sym / nq, "fp16": r_full_f16 / nq },
                "ann_default_recall_at_10": { "float_rerank": r_ann_f / nq, "int8_rerank": r_ann_i8 / nq, "delta": (r_ann_i8 - r_ann_f) / nq, "fp16_rerank": r_ann_f16 / nq, "fp16_delta": (r_ann_f16 - r_ann_f) / nq },
                "ann_shortlist_avg": shortlist_len as f64 / nq,
                "exact_scan_ms": Value::Object(scan),
            }),
        );
        eprintln!(
            "[int8] n={n}: exact recall@10 int8 asym {:.4} sym {:.4} fp16 {:.4}; ANN default float {:.4} → int8 {:.4} (Δ {:+.4}), fp16 {:.4} (Δ {:+.4})",
            r_full_i8 / nq,
            r_full_sym / nq,
            r_full_f16 / nq,
            r_ann_f / nq,
            r_ann_i8 / nq,
            (r_ann_i8 - r_ann_f) / nq,
            r_ann_f16 / nq,
            (r_ann_f16 - r_ann_f) / nq
        );
    }
    Value::Object(out)
}

// --- real text, tokenizer, BM25 ----------------------------------------

/// This repo's Markdown, whitespace-normalized.
fn repo_text() -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut s = String::new();
    for f in [
        "PLAN.md",
        "CHANGELOG.md",
        "README.md",
        "reviewer_notes.md",
        "mnemo/README.md",
        "mnemo-python/README.md",
        "AGENTS.md",
    ] {
        if let Ok(t) = std::fs::read_to_string(root.join(f)) {
            s.push_str(&t);
            s.push(' ');
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Split text into chunks of about `target` bytes at word boundaries.
fn chunks(text: &str, target: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for w in text.split(' ') {
        if cur.len() + w.len() + 1 > target && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Prototype tokenizer: lowercase alphanumeric runs (a stand-in for UAX
/// #29 words on this mostly-ASCII text), plus compound tokens: whitespace
/// tokens with leading and trailing punctuation stripped that still hold
/// internal punctuation ("bp.1", "v0.9.0", "pr-10b.2").
fn tokenize(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut out: Vec<String> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect();
    for raw in lower.split_whitespace() {
        let t = raw.trim_matches(|c: char| !c.is_alphanumeric());
        if t.chars().any(|c| !c.is_alphanumeric()) && t.chars().any(|c| c.is_alphanumeric()) {
            out.push(t.to_string());
        }
    }
    out
}

fn varint_len(mut x: u64) -> usize {
    let mut n = 1;
    while x >= 0x80 {
        x >>= 7;
        n += 1;
    }
    n
}

/// Size of a BM25 segment over `docs`: postings `(delta-varint row, tf u8)`,
/// a front-coded sorted dictionary with a 4-byte postings offset per term,
/// and a `u16` length per document.
fn bm25_bytes(docs: &[Vec<String>]) -> (usize, usize, usize, usize) {
    let mut index: BTreeMap<&str, Vec<(u32, u8)>> = BTreeMap::new();
    for (row, toks) in docs.iter().enumerate() {
        let mut tf: HashMap<&str, u32> = HashMap::new();
        for t in toks {
            *tf.entry(t.as_str()).or_insert(0) += 1;
        }
        for (t, c) in tf {
            index
                .entry(t)
                .or_default()
                .push((row as u32, c.min(255) as u8));
        }
    }
    let mut postings = 0usize;
    for list in index.values() {
        let mut prev = 0u32;
        for &(row, _) in list {
            postings += varint_len((row - prev) as u64) + 1;
            prev = row;
        }
    }
    let mut dict = 0usize;
    let mut prev: &str = "";
    for t in index.keys() {
        let shared = prev
            .bytes()
            .zip(t.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        dict += 1 + 1 + (t.len() - shared) + 4;
        prev = t;
    }
    (postings, dict, docs.len() * 2, index.len())
}

fn part_bm25() -> Value {
    let text = repo_text();
    let facts = chunks(&text, 300);
    let toks: Vec<Vec<String>> = facts.iter().map(|c| tokenize(c)).collect();
    let mut out = serde_json::Map::new();
    let n_all = toks.len();
    for &n in &[1_000usize, 2_000, 4_000, n_all] {
        if n > n_all {
            continue;
        }
        let (post, dict, lens, terms) = bm25_bytes(&toks[..n]);
        let total = post + dict + lens;
        out.insert(
            n.to_string(),
            json!({
                "postings_bytes_per_memory": post as f64 / n as f64,
                "dictionary_bytes_per_memory": dict as f64 / n as f64,
                "lengths_bytes_per_memory": lens as f64 / n as f64,
                "total_bytes_per_memory": total as f64 / n as f64,
                "terms": terms,
                "dictionary_bytes": dict,
            }),
        );
        eprintln!(
            "[bm25] {n} facts: {:.0} B/memory (postings {:.0}, dict {:.0}), {terms} terms",
            total as f64 / n as f64,
            post as f64 / n as f64,
            dict as f64 / n as f64
        );
    }
    let uniq: f64 = toks
        .iter()
        .map(|t| t.iter().collect::<HashSet<_>>().len() as f64)
        .sum::<f64>()
        / n_all as f64;
    out.insert("unique_terms_per_memory".into(), json!(uniq));
    out.insert("source_chunks".into(), json!(n_all));
    Value::Object(out)
}

// --- thread-heavy profile ----------------------------------------------

/// Deterministic turn lengths: 60% short (100–600 B), 30% medium (1–4 KB),
/// 10% long (8–40 KB, tool output).
fn turn_lengths(n: usize) -> Vec<usize> {
    let mut x: u64 = 0x7E5D_0001;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    (0..n)
        .map(|_| {
            let r = next() % 100;
            let u = (next() % 10_000) as usize;
            if r < 60 {
                100 + u * 500 / 10_000
            } else if r < 90 {
                1_000 + u * 3_000 / 10_000
            } else {
                8_000 + u * 32_000 / 10_000
            }
        })
        .collect()
}

fn turn_record(content: String, seq: u64, part: Option<u64>) -> Memory {
    let mut m = Memory::new(content, MemoryType::Episodic, Vec::new());
    m.metadata.insert("_mnemo.kind".into(), json!("turn"));
    m.metadata.insert(
        "_mnemo.thread".into(),
        json!("claude-code:session-1234abcd"),
    );
    m.metadata.insert(
        "_mnemo.turn_id".into(),
        json!(format!("a1b2c3d4-0000-4000-8000-{seq:012x}")),
    );
    m.metadata.insert("_mnemo.seq".into(), json!(seq));
    m.metadata.insert(
        "_mnemo.role".into(),
        json!(if seq.is_multiple_of(2) {
            "user"
        } else {
            "assistant"
        }),
    );
    if let Some(p) = part {
        m.metadata.insert("_mnemo.part".into(), json!(p));
    }
    m
}

fn part_threads(facts_bytes_per_memory: f64, bm25_fact_bytes: f64) -> Value {
    let text = repo_text();
    let bytes = text.as_bytes();
    let lens = turn_lengths(50_000);
    let mut out = serde_json::Map::new();
    // Three chunk sizes: today's 8 KB; 8,000 B; and 7,900 B, the largest
    // that leaves room for a turn record's ~180 B of metadata inside one
    // page's 8,144 usable bytes.
    for chunk_bytes in [8_192usize, 8_000, 7_900] {
        let mut records: Vec<usize> = Vec::new();
        let mut turn_toks: Vec<Vec<String>> = Vec::new();
        let mut off = 0usize;
        for (seq, &len) in lens.iter().enumerate() {
            let start = off % (bytes.len() - 45_000);
            let mut end = (start + len).min(bytes.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let mut s0 = start;
            while !text.is_char_boundary(s0) {
                s0 += 1;
            }
            off += len + 137;
            let body = &text[s0..end];
            let parts: Vec<&str> = if body.len() > chunk_bytes {
                let mut v = Vec::new();
                let mut i = 0;
                while i < body.len() {
                    let mut j = (i + chunk_bytes).min(body.len());
                    while !body.is_char_boundary(j) {
                        j -= 1;
                    }
                    v.push(&body[i..j]);
                    i = j;
                }
                v
            } else {
                vec![body]
            };
            let multi = parts.len() > 1;
            for (pi, p) in parts.iter().enumerate() {
                let m = turn_record(p.to_string(), seq as u64, multi.then_some(pi as u64));
                records.push(rmp_len(&m));
                if chunk_bytes == 7_900 {
                    turn_toks.push(tokenize(p));
                }
            }
        }
        let n_chunks = records.len();
        let (rec_pages, span_pages) = pack_pages(records.iter().copied());
        let turn_record_bytes = (rec_pages + span_pages) * PAGE;
        let per_vec_page = (PAYLOAD - PAGE_HEADER) / vec_entry(DIMENSIONS);
        let turn_vec_bytes = n_chunks.div_ceil(per_vec_page) * PAGE;
        let turn_row_bytes = n_chunks * 58;
        let facts_bytes = (facts_bytes_per_memory * 10_000.0) as usize;
        let mut entry = json!({
            "turns": lens.len(),
            "turn_chunks": n_chunks,
            "turn_text_bytes": lens.iter().sum::<usize>(),
            "turn_record_pages": rec_pages,
            "turn_span_pages": span_pages,
            "turn_record_bytes": turn_record_bytes,
            "turn_vector_bytes": turn_vec_bytes,
            "turn_catalog_bytes": turn_row_bytes,
            "facts_bytes": facts_bytes,
        });
        if chunk_bytes == 7_900 {
            let (post, dict, lens_b, terms) = bm25_bytes(&turn_toks);
            let turn_bm25 = post + dict + lens_b;
            let facts_bm25 = (bm25_fact_bytes * 10_000.0) as usize;
            let total_no_turn_bm25 =
                facts_bytes + turn_record_bytes + turn_vec_bytes + turn_row_bytes;
            entry["turn_bm25_bytes"] = json!(turn_bm25);
            entry["turn_bm25_terms"] = json!(terms);
            entry["total_bytes_turns_out_of_bm25"] = json!(total_no_turn_bm25);
            entry["total_bytes_turns_in_bm25"] = json!(total_no_turn_bm25 + turn_bm25);
            entry["facts_bm25_bytes"] = json!(facts_bm25);
            // RAM: catalog rows for everything; int8 column resident for
            // facts only (turns are read by thread, not by similarity).
            let ram = (10_000 + n_chunks) * 72 + 10_000 * vec_entry(DIMENSIONS);
            entry["ram_rows_and_fact_vectors_bytes"] = json!(ram);
            eprintln!(
                "[threads] 10K facts + 50K turns ({n_chunks} chunks): {:.1} MB without turns in BM25, {:.1} MB with ({:.1} MB of turn postings); RAM rows + fact vectors {:.1} MB",
                total_no_turn_bm25 as f64 / 1e6,
                (total_no_turn_bm25 + turn_bm25) as f64 / 1e6,
                turn_bm25 as f64 / 1e6,
                ram as f64 / 1e6
            );
        }
        eprintln!(
            "[threads] chunk {chunk_bytes} B: {n_chunks} chunks, {rec_pages} record pages + {span_pages} span pages = {:.1} MB of turn records",
            turn_record_bytes as f64 / 1e6
        );
        out.insert(format!("chunk_{chunk_bytes}"), entry);
    }
    Value::Object(out)
}

// --- compression --------------------------------------------------------

#[cfg(feature = "fmt9_proto")]
fn part_compress() -> Value {
    let text = repo_text();
    let facts = chunks(&text, 300);
    let recs: Vec<Vec<u8>> = facts
        .iter()
        .map(|c| {
            rmp_serde::to_vec(&Memory::new(c.clone(), MemoryType::Semantic, Vec::new())).unwrap()
        })
        .collect();
    // Pack into pages, then compress each page's slot area.
    let cap = PAYLOAD - PAGE_HEADER;
    let mut pages: Vec<Vec<u8>> = vec![Vec::new()];
    for r in &recs {
        if pages.last().unwrap().len() + r.len() + SLOT > cap {
            pages.push(Vec::new());
        }
        let p = pages.last_mut().unwrap();
        p.extend_from_slice(r);
        p.extend_from_slice(&[0u8; SLOT]);
    }
    let raw: usize = pages.iter().map(|p| p.len()).sum();
    let lz4: usize = pages
        .iter()
        .map(|p| lz4_flex::block::compress(p).len())
        .sum();
    let zstd3: usize = pages
        .iter()
        .map(|p| zstd::bulk::compress(p, 3).unwrap().len())
        .sum();
    eprintln!(
        "[compress] {} records on {} pages: lz4_flex {:.2}×, zstd-3 {:.2}×",
        recs.len(),
        pages.len(),
        raw as f64 / lz4 as f64,
        raw as f64 / zstd3 as f64
    );
    json!({
        "records": recs.len(),
        "raw_page_bytes": raw,
        "lz4_flex_bytes": lz4,
        "zstd3_bytes": zstd3,
        "lz4_flex_ratio": raw as f64 / lz4 as f64,
        "zstd3_ratio": raw as f64 / zstd3 as f64,
        "record_bytes_saved_per_memory_lz4": (raw - lz4) as f64 / recs.len() as f64,
        "record_bytes_saved_per_memory_zstd3": (raw - zstd3) as f64 / recs.len() as f64,
    })
}

#[cfg(not(feature = "fmt9_proto"))]
fn part_compress() -> Value {
    eprintln!("[compress] skipped: build with --features fmt9_proto");
    Value::Null
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let part = args
        .iter()
        .position(|a| a == "--part")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "all".into());
    let want = |p: &str| part == "all" || part.split(',').any(|x| x == p);
    let mut out = serde_json::Map::new();
    let mut fact_bytes_10k = 800.0;
    let mut bm25_fact = 170.0;
    if want("packing") {
        let v = part_packing(&[1_000, 10_000, 100_000]);
        if let Some(r) = v.get("10000") {
            fact_bytes_10k = r["record_bytes_per_memory"].as_f64().unwrap_or(0.0)
                + r["vector_bytes_per_memory"].as_f64().unwrap_or(0.0)
                + r["catalog_row_avg_bytes"].as_f64().unwrap_or(0.0);
        }
        out.insert("packing".into(), v);
    }
    if want("int8") {
        out.insert("int8".into(), part_int8(&[10_000, 100_000]));
    }
    if want("bm25") {
        let v = part_bm25();
        if let Some(t) = v.get("4000").or_else(|| v.get("2000")) {
            bm25_fact = t["total_bytes_per_memory"].as_f64().unwrap_or(bm25_fact);
        }
        out.insert("bm25".into(), v);
    }
    if want("threads") {
        out.insert("threads".into(), part_threads(fact_bytes_10k, bm25_fact));
    }
    if want("compress") {
        out.insert("compress".into(), part_compress());
    }
    let base = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_string());
    let path = PathBuf::from(base).join("fmt9_proto.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&Value::Object(out)).unwrap(),
    )
    .unwrap();
    println!("wrote {}", path.display());
}
