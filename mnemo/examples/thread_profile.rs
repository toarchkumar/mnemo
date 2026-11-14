//! FMT-9e: the thread-heavy profile on the real engine (R-004.2: real
//! files hold far more turns than facts). Builds a v9 file of `--facts`
//! facts (default 10,000, ~300 B each) and `--turns` turns (default
//! 50,000, in threads of 500), with the FMT-9a turn-length mix (60% short,
//! 30% medium, 10% long tool-style output) cut from this repo's Markdown,
//! then reports:
//!
//! - file bytes before and after `compact_file`, and bytes per turn;
//! - ingest time (one `ingest_thread` + `flush` per thread);
//! - reading a whole thread back (`thread_export_turns`), before and
//!   after compaction: the first thread on a fresh handle (`cold`, which
//!   builds the handle caches), 20 other threads' first reads
//!   (`first_touch`, median) and the same 20 read again (`warm`, median).
//!
//! Turns are stamped with the ingest time, so they're "recent". Three modes:
//!
//! ```text
//! cargo run --release --example thread_profile                         # default: compress when cold (7 days)
//! cargo run --release --example thread_profile -- --compress-after 0   # compress every turn at write
//! MNEMO_DEV_TURN_ZSTD=0 cargo run --release --example thread_profile   # never compress (baseline)
//! cargo run --release --example thread_profile -- --facts 1000 --turns 5000
//! ```
//!
//! Writes `target/thread_profile/thread_profile_{cold,eager,plain}.json`.

use std::path::PathBuf;
use std::time::Instant;

use mnemo_engine::{
    Embedder, EmbedderIdentity, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, ThreadTurn,
};
use serde_json::json;

const DIMS: usize = 384;
const TURNS_PER_THREAD: usize = 500;
const PASSPHRASE: &str = "bench-pw";

/// A deterministic 384-dim embedding seeded from the text's bytes.
struct HashEmbedder;
impl Embedder for HashEmbedder {
    fn identity(&self) -> EmbedderIdentity {
        EmbedderIdentity {
            name: "hash-384".into(),
            dimensions: DIMS,
            normalize: false,
            ..Default::default()
        }
    }
    fn embed(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| hash_vector(t)).collect())
    }
}

fn hash_vector(t: &str) -> Vec<f32> {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in t.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    (0..DIMS)
        .map(|_| {
            h = h
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((h >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0 + 1e-3
        })
        .collect()
}

/// This repo's Markdown, whitespace-normalized (as in `fmt9_proto.rs`).
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

/// Deterministic turn lengths: 60% short (100–600 B), 30% medium (1–4 KB),
/// 10% long (8–40 KB). The same mix as `fmt9_proto.rs`.
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

/// Cuts consecutive slices of the repo text, wrapping around at the end.
struct TextCursor {
    text: String,
    off: usize,
}

impl TextCursor {
    fn take(&mut self, len: usize) -> String {
        let t = &self.text;
        if self.off + len >= t.len() {
            self.off = 0;
        }
        let mut lo = self.off;
        while !t.is_char_boundary(lo) {
            lo += 1;
        }
        let mut hi = (lo + len).min(t.len());
        while !t.is_char_boundary(hi) {
            hi -= 1;
        }
        self.off = hi;
        t[lo..hi].to_string()
    }
}

/// Read threads back on a fresh handle: the first (cold, it builds the
/// handle caches), then 20 others once (first touch) and again (warm).
fn measure_reads(path: &std::path::Path, n_threads: usize, n_turns: usize) -> serde_json::Value {
    let mut db = Mnemo::open(path, PASSPHRASE).expect("open");
    let t = Instant::now();
    let first = db.thread_export_turns("thread-0").expect("export");
    let cold_ms = ms(t);
    assert_eq!(first.len(), TURNS_PER_THREAD.min(n_turns));
    let picks: Vec<usize> = (0..20.min(n_threads.saturating_sub(1)))
        .map(|k| 1 + (k * 7 + 3) % (n_threads - 1))
        .collect();
    let time_all = |db: &mut Mnemo| {
        let mut xs: Vec<f64> = picks
            .iter()
            .map(|th| {
                let t = Instant::now();
                let turns = db
                    .thread_export_turns(&format!("thread-{th}"))
                    .expect("export");
                assert!(!turns.is_empty());
                ms(t)
            })
            .collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        xs.get(xs.len() / 2).copied().unwrap_or(0.0)
    };
    let first_touch = time_all(&mut db);
    let warm = time_all(&mut db);
    json!({ "cold_ms": cold_ms, "first_touch_median_ms": first_touch, "warm_median_ms": warm })
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1_000.0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |name: &str, default: usize| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let n_facts = arg("--facts", 10_000);
    let n_turns = arg("--turns", 50_000);
    let compress_after: Option<u64> = args
        .iter()
        .position(|a| a == "--compress-after")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok());
    let tag = match (
        std::env::var("MNEMO_DEV_TURN_ZSTD").as_deref(),
        compress_after,
    ) {
        (Ok("0"), _) => "plain",
        (_, Some(0)) => "eager",
        _ => "cold",
    };
    let base = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_string());
    let dir = PathBuf::from(base).join("thread_profile");
    std::fs::create_dir_all(&dir).expect("create target/thread_profile");
    let path = dir.join(format!("thread_profile_{tag}.mnemo"));
    let _ = std::fs::remove_file(&path);

    let cfg = MnemoConfig {
        dimensions: DIMS,
        kdf: KdfParams::fast(),
        format_version: 9,
        ..Default::default()
    };
    let mut cur = TextCursor {
        text: repo_text(),
        off: 0,
    };
    let mut db = Mnemo::create(&path, PASSPHRASE, cfg).expect("create");
    db.set_embedder(HashEmbedder).expect("embedder");
    if let Some(secs) = compress_after {
        db.set_turn_compress_after(secs);
    }

    let t = Instant::now();
    for i in 0..n_facts {
        let text = cur.take(300);
        let v = hash_vector(&text);
        db.remember(Memory::new(text, MemoryType::Semantic, v))
            .expect("remember");
        if (i + 1) % 1_000 == 0 {
            db.flush().expect("flush");
        }
    }
    db.flush().expect("flush");
    let facts_ms = ms(t);

    let lens = turn_lengths(n_turns);
    let text_bytes: usize = lens.iter().sum();
    let n_threads = n_turns.div_ceil(TURNS_PER_THREAD);
    let t = Instant::now();
    for (th, group) in lens.chunks(TURNS_PER_THREAD).enumerate() {
        let turns: Vec<ThreadTurn> = group
            .iter()
            .enumerate()
            .map(|(j, &len)| ThreadTurn {
                turn_id: format!("th{th}-t{j}"),
                role: if j % 2 == 0 { "user" } else { "assistant" }.into(),
                content: cur.take(len),
                timestamp: None,
            })
            .collect();
        db.ingest_thread(&format!("thread-{th}"), &turns)
            .expect("ingest");
        db.flush().expect("flush");
    }
    let ingest_ms = ms(t);
    let records = db.len();
    db.close().expect("close");
    let file_bytes = std::fs::metadata(&path).expect("stat").len();

    let before = measure_reads(&path, n_threads, n_turns);
    Mnemo::compact_file(path.to_str().expect("utf-8 path"), PASSPHRASE).expect("compact");
    let live_bytes = std::fs::metadata(&path).expect("stat").len();
    let after = measure_reads(&path, n_threads, n_turns);

    let out = json!({
        "turn_records": tag,
        "facts": n_facts,
        "turns": n_turns,
        "threads": n_threads,
        "records": records,
        "turn_text_bytes": text_bytes,
        "file_bytes": file_bytes,
        "live_bytes_after_compact": live_bytes,
        "live_bytes_per_turn": live_bytes as f64 / n_turns as f64,
        "facts_ms": facts_ms,
        "ingest_ms": ingest_ms,
        "ingest_ms_per_thread": ingest_ms / n_threads as f64,
        "reads_before_compact": before,
        "reads_after_compact": after,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
    let json_path = dir.join(format!("thread_profile_{tag}.json"));
    std::fs::write(&json_path, serde_json::to_string_pretty(&out).unwrap()).expect("write json");
    let _ = std::fs::remove_file(&path);
}
