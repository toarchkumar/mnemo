//! Fresh-handle loop harness (PR MEM-1; `fresh_handle_loop_n50`).
//!
//! Opens a cached corpus, measures the first recall on each fresh
//! handle, drops the handle, and prints the process's RSS after the
//! drop. Run it read-write, then read-only:
//!
//! ```text
//! cargo run --release --example open_rss --features benches -- --corpus 100000
//! cargo run --release --example open_rss --features benches -- --corpus 100000 --read-only
//! ```
//!
//! MEM-1 closed in v0.9.0: the 13–43 GB "growth" was this harness
//! (and the deleted `first_recall` bench) passing `0xDEAD_BEEF` as
//! `k_clusters` to `gen_clustered_vectors`, not an engine or allocator
//! leak. `corpus.rs` now asserts on implausible `k_clusters`.
//!
//! The macOS allocator switches from the diagnostic are kept as
//! optional knobs: `MallocSpaceEfficient=1 MallocAggressiveMadvise=1`
//! in the environment, `--pressure-relief` (calls
//! `malloc_zone_pressure_relief` after each drop), and
//! `--features benches,mimalloc_bench` (mimalloc as the global
//! allocator, in this example only).

#[cfg(feature = "mimalloc_bench")]
use mimalloc::MiMalloc;
#[cfg(feature = "mimalloc_bench")]
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use std::hint::black_box;
use std::process::Command;
use std::time::Instant;

use mnemo_engine::{Mnemo, RecallRequest, ScoreWeights};

#[path = "../benches/corpus.rs"]
mod corpus;
use corpus::{gen_clustered_vectors, setup_cached_corpus, DIMENSIONS, PASSPHRASE};

const OPEN_LOOPS: usize = 50;

/// Macro picks the active-allocator banner so the CLI header leaves
/// no ambiguity about which variant produced the numbers.
const ALLOCATOR_BANNER: &str = if cfg!(feature = "mimalloc_bench") {
    "mimalloc (variant 4)"
} else {
    "system default (variant 1/2/3 depending on env + --pressure-relief)"
};

/// macOS `libSystem` purge hook. The planner-A DISPATCH wants variant
/// 3 to call this after every handle drop. Declared only on
/// `target_os = "macos"`; other platforms fall back to a no-op so the
/// example still builds.
#[cfg(target_os = "macos")]
mod purge {
    #[link(name = "System", kind = "dylib")]
    extern "C" {
        fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
    }
    pub fn pressure_relief() -> usize {
        // SAFETY: libSystem's `malloc_zone_pressure_relief` accepts a
        // null zone (meaning "all zones") and a goal of 0 (meaning
        // "as much as you can give back"). Returns the number of
        // bytes released.
        unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) }
    }
}
#[cfg(not(target_os = "macos"))]
mod purge {
    pub fn pressure_relief() -> usize {
        0
    }
}

/// Snapshot the current process's resident set size (RSS) in kB via
/// `ps -o rss= -p <pid>`. Portable enough for macOS + Linux.
fn rss_kb() -> u64 {
    let pid = std::process::id();
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output();
    out.ok()
        .and_then(|o| {
            String::from_utf8(o.stdout)
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
        })
        .unwrap_or(0)
}

fn one_iteration(
    path: &std::path::Path,
    q: &[f32],
    read_only: bool,
    pressure_relief: bool,
) -> (f64, u64, u64) {
    let mut db = if read_only {
        Mnemo::open_read_only(path, PASSPHRASE).expect("open_read_only")
    } else {
        Mnemo::open(path, PASSPHRASE).expect("open")
    };
    let req = RecallRequest::new(q.to_vec())
        .top_k(10)
        .track_access(false)
        .weights(ScoreWeights::default());
    let start = Instant::now();
    let hits = db.recall(&req).expect("recall");
    let elapsed = start.elapsed();
    black_box(hits);
    drop(db);
    // Variant 3: ask libSystem to return free pages to the OS before
    // we sample RSS.
    let released = if pressure_relief {
        purge::pressure_relief() as u64
    } else {
        0
    };
    let rss = rss_kb();
    (elapsed.as_secs_f64() * 1_000.0, rss, released)
}

fn percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((pct / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn median(sorted: &[f64]) -> f64 {
    percentile(sorted, 50.0)
}

/// Parse the CLI args (hand-rolled to avoid pulling in clap for a
/// one-shot measurement example). Accepts `--corpus N`, `--loops N`,
/// `--pressure-relief`, `--read-only`.
struct Args {
    corpus: usize,
    loops: usize,
    pressure_relief: bool,
    read_only_phase: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        corpus: 100_000,
        loops: OPEN_LOOPS,
        pressure_relief: false,
        read_only_phase: false,
    };
    let mut it = std::env::args().skip(1).peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--corpus" => {
                args.corpus = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--corpus needs a positive integer");
            }
            "--loops" => {
                args.loops = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--loops needs a positive integer");
            }
            "--pressure-relief" => args.pressure_relief = true,
            "--read-only" => args.read_only_phase = true,
            other => panic!("unknown arg: {other}"),
        }
    }
    args
}

fn main() {
    let args = parse_args();
    println!("mnemo PR MEM-1 open_rss harness");
    println!("  pid         : {}", std::process::id());
    println!(
        "  corpus size : {} memories, {}-dim vectors",
        args.corpus, DIMENSIONS
    );
    println!("  loops       : {}", args.loops);
    println!(
        "  phase       : {}",
        if args.read_only_phase {
            "read-only"
        } else {
            "read-write"
        }
    );
    println!(
        "  pressure_relief : {}",
        if args.pressure_relief {
            "on (variant 3)"
        } else {
            "off"
        }
    );
    println!("  allocator       : {ALLOCATOR_BANNER}");
    println!(
        "  macOS env       : MallocSpaceEfficient={} MallocAggressiveMadvise={}",
        std::env::var("MallocSpaceEfficient").unwrap_or_else(|_| "unset".into()),
        std::env::var("MallocAggressiveMadvise").unwrap_or_else(|_| "unset".into()),
    );

    let cached = setup_cached_corpus(args.corpus, DIMENSIONS);
    let queries = gen_clustered_vectors(args.loops, DIMENSIONS, 100, 0xDEAD_BEEF);
    println!("rss_kb at start: {}\n", rss_kb());

    println!(
        "{:>4}  {:>14}  {:>14}  {:>14}",
        "iter", "first_recall_ms", "rss_kb_after", "released_kb"
    );
    let mut ms_log = Vec::with_capacity(args.loops);
    let mut rss_log = Vec::with_capacity(args.loops);
    for i in 0..args.loops {
        let q = &queries[i % queries.len()];
        let (ms, rss, released) =
            one_iteration(&cached, q, args.read_only_phase, args.pressure_relief);
        println!("{i:>4}  {ms:>14.3}  {rss:>14}  {:>14}", released / 1024);
        ms_log.push(ms);
        rss_log.push(rss);
    }

    let mut sorted = ms_log.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    println!("\n== summary ==");
    println!(
        "first-recall ms: median {:.3}, p95 {:.3}",
        median(&sorted),
        percentile(&sorted, 95.0),
    );
    let start = *rss_log.first().unwrap_or(&0);
    let end = *rss_log.last().unwrap_or(&0);
    let peak = *rss_log.iter().max().unwrap_or(&0);
    println!(
        "rss_kb: start {start}, end {end}, peak {peak}, delta {}",
        end as i64 - start as i64
    );
    // Slope over the second half: positive = climbing, near-zero =
    // plateau.
    let slope = {
        if rss_log.len() < 4 {
            0.0
        } else {
            let half = rss_log.len() / 2;
            let last_half = &rss_log[half..];
            let first = *last_half.first().unwrap_or(&0) as f64;
            let last = *last_half.last().unwrap_or(&0) as f64;
            (last - first) / (last_half.len() as f64 - 1.0)
        }
    };
    println!("rss_kb slope (second half): {:.2} kB/iter", slope);
}
