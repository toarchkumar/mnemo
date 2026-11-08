//! P-0 device budget bench (on-device track, PLANNER-A 2026-11-07).
//!
//! Reports, at 1K / 10K / 100K memories of ~300 bytes of text and
//! 384-dim vectors, with an IVF+PQ index and the fast KDF:
//!
//! - file bytes per memory, split into records (and their page slack),
//!   index, catalog, WAL and the rest;
//! - RSS of one open handle (open → first recall → 200 warm recalls),
//!   plus the engine's own heap breakdown (`Mnemo::__budget_report`), and
//!   the phase's peak RSS when `/usr/bin/time` is available;
//! - open time and first recall on fresh handles (20 cycles);
//! - warm recall (median / p95 over 200 queries);
//! - durable writes: single (`remember` + `flush`) and batched (100
//!   `remember` + one `flush`), with the fsync count per commit.
//!
//! ```text
//! cargo run --release --example device_budget --features benches
//! cargo run --release --example device_budget --features benches -- --sizes 1000,10000
//! ```
//!
//! Each measurement phase runs in its own child process (the binary
//! re-executes itself), so one phase's allocator retention doesn't leak
//! into the next phase's RSS. Plain loops, no Criterion, as in
//! `open_rss.rs`. Corpora are cached under `target/device_budget/`.
//! The parent prints a table and writes `target/device_budget/device_budget.json`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use mnemo_engine::{
    Embedder, EmbedderIdentity, EssenceScope, EssenceSection, IndexConfig, Memory, MemoryType,
    Mnemo, RecallRequest, ThreadTurn,
};
use serde_json::{json, Value};

#[path = "../benches/corpus.rs"]
mod corpus;
use corpus::{fast_kdf_cfg, gen_clustered_vectors, DIMENSIONS, PASSPHRASE};

const DEFAULT_SIZES: [usize; 3] = [1_000, 10_000, 100_000];
const TEXT_BYTES: usize = 300;
const OPEN_CYCLES: usize = 20;
const WARM_QUERIES: usize = 200;
const SINGLE_WRITES: usize = 20;
const BATCHES: usize = 5;
const BATCH_SIZE: usize = 100;

const WORDS: [&str; 16] = [
    "agent", "memory", "decided", "release", "the", "user", "prefers", "tea", "index", "thread",
    "essence", "budget", "device", "flush", "recall", "because",
];

/// Deterministic ~300-byte ASCII text for memory `i`.
fn text_for(i: usize) -> String {
    let mut s = format!("memory {i}:");
    // 64-bit LCG; the high bits pick each word.
    let mut k = (i as u64).wrapping_mul(2_654_435_761).wrapping_add(1);
    while s.len() < TEXT_BYTES {
        k = k
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        s.push(' ');
        s.push_str(WORDS[((k >> 33) % WORDS.len() as u64) as usize]);
    }
    s.truncate(TEXT_BYTES);
    s
}

fn out_dir() -> PathBuf {
    let base = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_string());
    PathBuf::from(base).join("device_budget")
}

fn corpus_path(n: usize) -> PathBuf {
    out_dir().join(format!("corpus_{n}_{DIMENSIONS}d_{TEXT_BYTES}b.mnemo"))
}

fn rss_kb() -> u64 {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output();
    out.ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

fn median_p95(mut xs: Vec<f64>) -> (f64, f64) {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let at = |p: f64| xs[((p * (xs.len() as f64 - 1.0)).round() as usize).min(xs.len() - 1)];
    (at(0.5), at(0.95))
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1_000.0
}

fn queries() -> Vec<Vec<f32>> {
    gen_clustered_vectors(WARM_QUERIES, DIMENSIONS, 100, 0xDE71_CE00)
}

fn recall_req(q: &[f32]) -> RecallRequest {
    RecallRequest::new(q.to_vec()).top_k(10).track_access(false)
}

/// Copy the cached corpus to a scratch file the phase may modify.
fn scratch_copy(n: usize, tag: &str) -> PathBuf {
    let dst = out_dir().join(format!("scratch_{n}_{tag}.mnemo"));
    std::fs::copy(corpus_path(n), &dst).expect("copy corpus");
    dst
}

// --- phases (each runs in its own child process) ----------------------

fn phase_build(n: usize) -> Value {
    let path = corpus_path(n);
    if path.exists() {
        return json!({ "built": false });
    }
    std::fs::create_dir_all(out_dir()).expect("create target/device_budget");
    let t = Instant::now();
    let mut db = Mnemo::create(&path, PASSPHRASE, fast_kdf_cfg(DIMENSIONS)).expect("create");
    let k = ((n as f64).sqrt().ceil() as usize).max(16);
    let vectors = gen_clustered_vectors(n, DIMENSIONS, k, 0xBEEF_C0DE);
    for (i, v) in vectors.into_iter().enumerate() {
        db.remember(Memory::new(text_for(i), MemoryType::Semantic, v))
            .expect("remember");
    }
    db.flush().expect("flush");
    db.build_index_with(IndexConfig::default())
        .expect("build_index");
    db.flush().expect("flush");
    json!({ "built": true, "build_secs": t.elapsed().as_secs_f64() })
}

fn phase_file(n: usize) -> Value {
    let db = Mnemo::open_read_only(corpus_path(n), PASSPHRASE).expect("open");
    let as_built = db.__budget_report().expect("budget");
    drop(db);
    let copy = scratch_copy(n, "compact");
    Mnemo::compact_file(copy.to_str().unwrap(), PASSPHRASE).expect("compact");
    let db = Mnemo::open_read_only(&copy, PASSPHRASE).expect("open compacted");
    let compacted = db.__budget_report().expect("budget");
    drop(db);
    let _ = std::fs::remove_file(&copy);
    json!({ "as_built": as_built, "compacted": compacted })
}

fn phase_handle(n: usize) -> Value {
    let qs = queries();
    let rss_before = rss_kb();
    let t = Instant::now();
    let mut db = Mnemo::open(corpus_path(n), PASSPHRASE).expect("open");
    let open_ms = ms(t);
    let rss_open = rss_kb();
    let t = Instant::now();
    std::hint::black_box(db.recall(&recall_req(&qs[0])).expect("recall"));
    let first_ms = ms(t);
    let rss_first = rss_kb();
    let mut warm = Vec::with_capacity(WARM_QUERIES);
    for q in &qs {
        let t = Instant::now();
        std::hint::black_box(db.recall(&recall_req(q)).expect("recall"));
        warm.push(ms(t));
    }
    let rss_warm = rss_kb();
    let engine = db.__budget_report().expect("budget");
    drop(db);
    let rss_dropped = rss_kb();
    let (warm_med, warm_p95) = median_p95(warm);
    json!({
        "open_ms": open_ms,
        "first_recall_ms": first_ms,
        "warm_recall_median_ms": warm_med,
        "warm_recall_p95_ms": warm_p95,
        "rss_kb_before_open": rss_before,
        "rss_kb_open": rss_open,
        "rss_kb_after_first_recall": rss_first,
        "rss_kb_after_warm": rss_warm,
        "rss_kb_after_drop": rss_dropped,
        "handle_rss_kb": rss_warm.saturating_sub(rss_before),
        "engine_heap": engine,
    })
}

fn phase_cycles(n: usize) -> Value {
    let qs = queries();
    let (mut opens, mut firsts) = (Vec::new(), Vec::new());
    for i in 0..OPEN_CYCLES {
        let t = Instant::now();
        let mut db = Mnemo::open(corpus_path(n), PASSPHRASE).expect("open");
        opens.push(ms(t));
        let t = Instant::now();
        std::hint::black_box(db.recall(&recall_req(&qs[i % qs.len()])).expect("recall"));
        firsts.push(ms(t));
        drop(db);
    }
    let (open_med, open_p95) = median_p95(opens);
    let (first_med, first_p95) = median_p95(firsts);
    json!({
        "cycles": OPEN_CYCLES,
        "open_median_ms": open_med,
        "open_p95_ms": open_p95,
        "first_recall_median_ms": first_med,
        "first_recall_p95_ms": first_p95,
    })
}

fn phase_writes(n: usize) -> Value {
    let copy = scratch_copy(n, "writes");
    let file_len = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let bytes_before = file_len(&copy);
    let mut db = Mnemo::open(&copy, PASSPHRASE).expect("open");
    let vs = gen_clustered_vectors(
        SINGLE_WRITES + BATCHES * BATCH_SIZE,
        DIMENSIONS,
        16,
        0x5717_0001,
    );
    let mut vs = vs.into_iter();
    let mut single = Vec::new();
    let mut fsyncs_single = 0;
    for i in 0..SINGLE_WRITES {
        let m = Memory::new(text_for(n + i), MemoryType::Semantic, vs.next().unwrap());
        let t = Instant::now();
        db.remember(m).expect("remember");
        db.flush().expect("flush");
        single.push(ms(t));
        fsyncs_single = db.stats().expect("stats").fsyncs_last_flush;
    }
    let bytes_after_single = file_len(&copy);
    let mut batched = Vec::new();
    let mut fsyncs_batch = 0;
    for b in 0..BATCHES {
        let t = Instant::now();
        for j in 0..BATCH_SIZE {
            let m = Memory::new(
                text_for(n + SINGLE_WRITES + b * BATCH_SIZE + j),
                MemoryType::Semantic,
                vs.next().unwrap(),
            );
            db.remember(m).expect("remember");
        }
        db.flush().expect("flush");
        batched.push(ms(t));
        fsyncs_batch = db.stats().expect("stats").fsyncs_last_flush;
    }
    drop(db);
    let bytes_after_batches = file_len(&copy);
    let _ = std::fs::remove_file(&copy);
    let (s_med, s_p95) = median_p95(single);
    let (b_med, b_p95) = median_p95(batched);
    json!({
        "single_write_median_ms": s_med,
        "single_write_p95_ms": s_p95,
        "single_write_fsyncs": fsyncs_single,
        "batch_size": BATCH_SIZE,
        "batch_write_median_ms": b_med,
        "batch_write_p95_ms": b_p95,
        "batch_write_per_memory_ms": b_med / BATCH_SIZE as f64,
        "batch_write_fsyncs": fsyncs_batch,
        // File growth per commit: every flush writes fresh catalog and
        // index runs, and the old ones stay until compact.
        "file_growth_per_single_write_bytes":
            bytes_after_single.saturating_sub(bytes_before) / SINGLE_WRITES as u64,
        "file_growth_per_batch_bytes":
            bytes_after_batches.saturating_sub(bytes_after_single) / BATCHES as u64,
    })
}

/// In-process embedder returning one canned vector, so the layers phase
/// measures engine cost only.
struct Canned(Vec<f32>);
impl Embedder for Canned {
    fn identity(&self) -> EmbedderIdentity {
        EmbedderIdentity {
            name: "device-budget-canned".into(),
            dimensions: self.0.len(),
            normalize: true,
        }
    }
    fn embed(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| self.0.clone()).collect())
    }
}

fn turn(i: usize) -> ThreadTurn {
    ThreadTurn {
        turn_id: format!("turn-{i:06}"),
        role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
        content: text_for(i),
        timestamp: None,
    }
}

/// PERF-1 step 2b (R-004.2): the `about` briefing + a thread's essence
/// render, and a single-turn ingest, cold (first call on a fresh handle,
/// which builds the handle caches) and warm.
fn phase_layers(n: usize) -> Value {
    let copy = scratch_copy(n, "layers");
    let canned = gen_clustered_vectors(1, DIMENSIONS, 1, 0x1A7E_0001).remove(0);
    {
        let mut db = Mnemo::open(&copy, PASSPHRASE).expect("open");
        db.set_embedder(Canned(canned.clone())).expect("embedder");
        db.remember(Memory::scaffold_manifest(DIMENSIONS))
            .expect("manifest");
        for i in 0..20 {
            let section = [EssenceSection::Conventions, EssenceSection::Decisions][i % 2];
            db.essence_add(
                EssenceScope::Project,
                section,
                &format!("project item {i}"),
                None,
            )
            .expect("essence");
        }
        for i in 0..10 {
            db.essence_add(
                EssenceScope::Thread("bench".into()),
                EssenceSection::Open,
                &format!("open item {i}"),
                None,
            )
            .expect("essence");
        }
        let turns: Vec<ThreadTurn> = (0..200).map(turn).collect();
        db.ingest_thread("bench", &turns).expect("ingest");
        db.close().expect("close");
    }
    let brief = |db: &mut Mnemo| {
        let t = Instant::now();
        std::hint::black_box(db.about().expect("about"));
        std::hint::black_box(db.about_essence().expect("about_essence"));
        std::hint::black_box(
            db.essence(EssenceScope::Thread("bench".into()), 6144)
                .expect("essence"),
        );
        ms(t)
    };
    // Cold brief: a fresh handle with no embedder, so the first call pays
    // the one lazy handle scan. (`set_embedder` reads the manifest through
    // `about()`, so on a handle with an embedder that scan happens at
    // registration instead.)
    let mut db = Mnemo::open(&copy, PASSPHRASE).expect("open");
    let brief_cold = brief(&mut db);
    db.set_embedder(Canned(canned.clone())).expect("embedder");
    let brief_warm: Vec<f64> = (0..20).map(|_| brief(&mut db)).collect();
    let mut ingest_warm = Vec::new();
    for i in 0..20 {
        let t = Instant::now();
        db.ingest_thread("bench", &[turn(1_000 + i)])
            .expect("ingest");
        ingest_warm.push(ms(t));
    }
    drop(db);
    // Cold ingest: fresh handle; registering the embedder pays the scan.
    let mut db = Mnemo::open(&copy, PASSPHRASE).expect("open");
    let t = Instant::now();
    db.set_embedder(Canned(canned)).expect("embedder");
    db.ingest_thread("bench", &[turn(2_000)]).expect("ingest");
    let ingest_cold = ms(t);
    drop(db);
    let _ = std::fs::remove_file(&copy);
    let (bw_med, bw_p95) = median_p95(brief_warm);
    let (iw_med, iw_p95) = median_p95(ingest_warm);
    json!({
        "brief_cold_ms": brief_cold,
        "brief_warm_median_ms": bw_med,
        "brief_warm_p95_ms": bw_p95,
        "ingest_one_turn_cold_ms": ingest_cold,
        "ingest_one_turn_warm_median_ms": iw_med,
        "ingest_one_turn_warm_p95_ms": iw_p95,
    })
}

// --- parent ------------------------------------------------------------

/// Peak RSS in kB from `/usr/bin/time` output: macOS `-l` prints bytes,
/// Linux `-v` prints kbytes.
fn parse_peak_kb(stderr: &str) -> Option<u64> {
    for line in stderr.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_suffix("maximum resident set size") {
            return rest.trim().parse::<u64>().ok().map(|b| b / 1024);
        }
        if let Some(rest) = l.strip_prefix("Maximum resident set size (kbytes):") {
            return rest.trim().parse::<u64>().ok();
        }
    }
    None
}

fn run_phase(phase: &str, n: usize) -> Value {
    let exe = std::env::current_exe().expect("current_exe");
    let time = Path::new("/usr/bin/time");
    let flag = if cfg!(target_os = "macos") {
        "-l"
    } else {
        "-v"
    };
    let mut cmd = if time.exists() {
        let mut c = Command::new(time);
        c.arg(flag).arg(&exe);
        c
    } else {
        Command::new(&exe)
    };
    let out = cmd
        .args(["--phase", phase, "--n", &n.to_string()])
        .output()
        .expect("spawn phase");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "phase {phase} n={n} failed: {stderr}");
    let line = stdout
        .lines()
        .find_map(|l| l.strip_prefix("RESULT "))
        .unwrap_or_else(|| panic!("phase {phase} printed no RESULT: {stdout}"));
    let mut v: Value = serde_json::from_str(line).expect("phase JSON");
    if let Some(peak) = parse_peak_kb(&stderr) {
        v["peak_rss_kb"] = json!(peak);
    }
    v
}

fn per_memory(bytes: &Value, n: usize) -> f64 {
    bytes.as_u64().unwrap_or(0) as f64 / n as f64
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    if let Some(phase) = arg("--phase") {
        let n: usize = arg("--n").and_then(|v| v.parse().ok()).expect("--n N");
        let v = match phase.as_str() {
            "build" => phase_build(n),
            "file" => phase_file(n),
            "handle" => phase_handle(n),
            "cycles" => phase_cycles(n),
            "writes" => phase_writes(n),
            "layers" => phase_layers(n),
            other => panic!("unknown phase {other}"),
        };
        println!("RESULT {v}");
        return;
    }

    let sizes: Vec<usize> = arg("--sizes")
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().parse().expect("--sizes"))
                .collect()
        })
        .unwrap_or_else(|| DEFAULT_SIZES.to_vec());

    // `--only layers` (comma-separated) runs a subset; build always runs.
    let only: Option<Vec<String>> =
        arg("--only").map(|s| s.split(',').map(|x| x.trim().to_string()).collect());
    let wanted = |p: &str| match &only {
        Some(o) => o.iter().any(|x| x == p),
        None => true,
    };
    let skip = || json!({});
    let mut results = serde_json::Map::new();
    for &n in &sizes {
        eprintln!("== n = {n}: build (cached after the first run)");
        let build = run_phase("build", n);
        eprintln!("== n = {n}: file, handle, cycles, writes");
        let run = |p: &str| if wanted(p) { run_phase(p, n) } else { skip() };
        let file = run("file");
        let handle = run("handle");
        let cycles = run("cycles");
        let writes = run("writes");
        let layers = run("layers");
        results.insert(
            n.to_string(),
            json!({ "build": build, "file": file, "handle": handle, "cycles": cycles, "writes": writes, "layers": layers }),
        );
    }

    println!(
        "\n{:>7} | {:>9} {:>9} {:>8} | {:>9} {:>9} {:>9} | {:>8} {:>8} {:>8} {:>8} | {:>8} {:>9}",
        "n",
        "B/mem",
        "compact",
        "slack%",
        "handleMB",
        "peakMB",
        "engineMB",
        "open",
        "open1st",
        "1stRcl",
        "warm",
        "1write",
        "batch/mem"
    );
    for &n in &sizes {
        let r = &results[&n.to_string()];
        let built = &r["file"]["as_built"];
        let comp = &r["file"]["compacted"];
        let pages = built["record_page_bytes"].as_f64().unwrap_or(0.0);
        let payload = built["record_payload_bytes"].as_f64().unwrap_or(0.0);
        let slack = if pages > 0.0 {
            100.0 * (pages - payload) / pages
        } else {
            0.0
        };
        let mb = |kb: &Value| kb.as_f64().unwrap_or(0.0) / 1024.0;
        println!(
            "{:>7} | {:>9.0} {:>9.0} {:>7.1}% | {:>9.1} {:>9.1} {:>9.1} | {:>8.2} {:>8.2} {:>8.2} {:>8.2} | {:>8.2} {:>9.3}",
            n,
            per_memory(&built["file_bytes"], n),
            per_memory(&comp["file_bytes"], n),
            slack,
            mb(&r["handle"]["handle_rss_kb"]),
            mb(&r["handle"]["peak_rss_kb"]),
            r["handle"]["engine_heap"]["ram_total"].as_f64().unwrap_or(0.0) / 1_048_576.0,
            r["cycles"]["open_median_ms"].as_f64().unwrap_or(0.0),
            r["handle"]["open_ms"].as_f64().unwrap_or(0.0),
            r["cycles"]["first_recall_median_ms"].as_f64().unwrap_or(0.0),
            r["handle"]["warm_recall_median_ms"].as_f64().unwrap_or(0.0),
            r["writes"]["single_write_median_ms"].as_f64().unwrap_or(0.0),
            r["writes"]["batch_write_per_memory_ms"].as_f64().unwrap_or(0.0),
        );
    }
    println!(
        "\n(B/mem: file bytes per memory as built / after compact; slack%: empty space in record \
         pages; handleMB: RSS growth for one open handle after 200 warm recalls; peakMB: peak RSS \
         of that phase's process; engineMB: the engine's own heap accounting; times in ms; open = \
         median over {OPEN_CYCLES} fresh opens, open1st = the handle phase's single open)"
    );
    println!(
        "\n{:>7} | {:>10} {:>10} | {:>12} {:>12}",
        "n", "brief cold", "brief warm", "ingest1 cold", "ingest1 warm"
    );
    for &n in &sizes {
        let l = &results[&n.to_string()]["layers"];
        let f = |k: &str| l[k].as_f64().unwrap_or(0.0);
        println!(
            "{:>7} | {:>10.2} {:>10.3} | {:>12.2} {:>12.3}",
            n,
            f("brief_cold_ms"),
            f("brief_warm_median_ms"),
            f("ingest_one_turn_cold_ms"),
            f("ingest_one_turn_warm_median_ms"),
        );
    }
    println!(
        "(brief = about() + about_essence() + essence(thread) with 1 manifest, 20 project and 10 \
         thread items; ingest1 = ingest_thread of one new turn into a 200-turn thread; cold = first \
         call on a fresh handle, which builds the handle caches; ingest1 cold includes \
         set_embedder)"
    );

    let doc = Value::Object(results);
    let path = out_dir().join("device_budget.json");
    std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).expect("write json");
    println!("full results: {}", path.display());
}
