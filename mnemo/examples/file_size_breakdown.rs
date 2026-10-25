//! Phase 6 file-size breakdown.
//!
//! Walks the cached bench corpora (`target/benches/corpus_<n>_384d.mnemo`,
//! only the tiers that exist on disk) and reports what fraction of the
//! file goes to each publicly observable section:
//!
//! - **WAL region** — fixed-size reservation (`wal_pages × 8 KiB`). At
//!   small N this dominates; it shrinks as a share as the data region
//!   grows.
//! - **Payload** — the logical content callers wrote: text bytes +
//!   `4 × dimensions × live_memories` for the vectors. This is the
//!   "cost of knowing stuff"; everything else is per-page encryption
//!   framing, catalog bookkeeping, snapshot history, and the index.
//! - **Everything else** — `file_bytes − wal_bytes − payload_bytes`.
//!   Mostly: AES-GCM nonce+tag framing per page (28 B/page × page
//!   count), catalog + manifest snapshots (one per flush, capped by
//!   `max_snapshots`), and the IVF+PQ index.
//!
//! We don't break "everything else" into finer-grained sections — that
//! would need engine-internal accounting APIs this dispatch forbids
//! adding. The three numbers above are the ones that drive design
//! decisions (WAL share picks the initial WAL reservation; payload
//! share picks whether to inline hot fields; everything-else share
//! picks whether to compact more aggressively).
//!
//! Output: appended under the `file_size_breakdown` key in
//! `test/scripts/perf_v0.5.0.json`.
//!
//! Run:
//! ```sh
//! cargo run --release --example file_size_breakdown
//! ```

use std::fs;
use std::path::PathBuf;

use mnemo::Mnemo;
use serde_json::{json, Value};

const PASSPHRASE: &str = "bench-pw";
const DIMENSIONS: usize = 384;
const PAGE_SIZE: u64 = 8192;

/// Average text content size per bench memory. The bench corpus
/// writes `format!("bench memory #{i}")` — ranges from 18 to 24
/// characters depending on the digit count; 21 is a safe mean for
/// a 10K-to-1M run. Vectors always weigh exactly `4 × DIMENSIONS`.
const AVG_TEXT_BYTES: u64 = 21;

fn cached_corpus_path(n: usize) -> PathBuf {
    let base = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_string());
    PathBuf::from(base)
        .join("benches")
        .join(format!("corpus_{n}_{DIMENSIONS}d.mnemo"))
}

fn breakdown_for(n: usize) -> Option<Value> {
    let path = cached_corpus_path(n);
    if !path.exists() {
        return None;
    }
    let mut db = Mnemo::open(&path, PASSPHRASE).ok()?;
    let stats = db.stats().ok()?;
    let file_bytes = stats.file_bytes;
    let wal_bytes = stats.wal_pages * PAGE_SIZE;
    let live = stats.memories as u64;
    let vector_bytes = live * (4 * DIMENSIONS as u64);
    let text_bytes = live * AVG_TEXT_BYTES;
    let payload_bytes = vector_bytes + text_bytes;
    let other_bytes = file_bytes.saturating_sub(wal_bytes + payload_bytes);

    let pct = |n: u64| round4(n as f64 * 100.0 / file_bytes as f64);
    let per_mem = |n: u64| round4(n as f64 / live.max(1) as f64);

    Some(json!({
        "corpus_size": n,
        "dimensions": DIMENSIONS,
        "live_memories": live,
        "file_bytes": file_bytes,
        "file_mib": round4(file_bytes as f64 / (1024.0 * 1024.0)),
        "wal_bytes": wal_bytes,
        "wal_share_pct": pct(wal_bytes),
        "payload_bytes": payload_bytes,
        "payload_share_pct": pct(payload_bytes),
        "other_bytes": other_bytes,
        "other_share_pct": pct(other_bytes),
        "effective_bytes_per_memory": per_mem(file_bytes),
        "payload_bytes_per_memory": per_mem(payload_bytes),
        "overhead_bytes_per_memory": per_mem(file_bytes - payload_bytes),
        "has_index": stats.index.is_some(),
    }))
}

fn main() {
    let mut tiers: Vec<Value> = Vec::new();
    for &n in &[10_000usize, 100_000, 1_000_000] {
        match breakdown_for(n) {
            Some(v) => {
                let file_mib = v["file_mib"].as_f64().unwrap_or(0.0);
                let wal_pct = v["wal_share_pct"].as_f64().unwrap_or(0.0);
                let payload_pct = v["payload_share_pct"].as_f64().unwrap_or(0.0);
                let other_pct = v["other_share_pct"].as_f64().unwrap_or(0.0);
                let per_mem = v["effective_bytes_per_memory"].as_f64().unwrap_or(0.0);
                println!(
                    "  {n:>8} memories  {file_mib:>8.2} MiB  WAL {wal_pct:>5.1}%  payload {payload_pct:>5.1}%  other {other_pct:>5.1}%  {per_mem:>7.0} B/memory",
                );
                tiers.push(v);
            }
            None => {
                let label = match n {
                    10_000 => "10k",
                    100_000 => "100k",
                    1_000_000 => "1m",
                    _ => "?",
                };
                println!(
                    "  {n:>8} memories  (no cached corpus — skipped; run `cargo bench --features benches --bench recall -- {label}`)"
                );
            }
        }
    }

    if tiers.is_empty() {
        eprintln!(
            "[file-size] no cached corpora found under target/benches/. \
             Build at least one via `cargo bench --features benches --bench recall -- 10k`."
        );
        std::process::exit(1);
    }

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
    out["file_size_breakdown"] = json!({ "tiers": tiers });
    fs::write(
        &perf_path,
        serde_json::to_string_pretty(&out).expect("serialize") + "\n",
    )
    .expect("write perf json");
    println!("[file-size] wrote {}", perf_path.display());
}

fn round4(x: f64) -> Value {
    Value::from((x * 10000.0).round() / 10000.0)
}
