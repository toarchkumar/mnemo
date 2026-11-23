//! v0.21.0 (v0210-3b): two diverged copies of one brain, for timing
//! `mnemo merge` and its peak memory.
//!
//! ```text
//! cargo run --release --example merge_bench -- <dir> <records>
//! MNEMO_PASSPHRASE=merge-bench /usr/bin/time -l \
//!     target/release/mnemo merge <dir>/a.mnemo <dir>/b.mnemo -o <dir>/out.mnemo
//! cargo run --release --example merge_bench -- time <dir>
//! ```
//!
//! `time` reads the two copies and times the merge's rules alone (no
//! reads, no writes) three times.
//!
//! The base holds `<records>` facts of 384 dimensions, a fifth of them
//! keyed. Each copy then edits 1% of the plain facts and writes a new
//! version of 1% of the keys, deletes another 1% of the keys (so each copy
//! holds tombstones), writes again 100 of the keys the other copy deletes,
//! changes 100 plain facts and 100 keys the other copy changes too (the
//! conflicts), and adds 1% new facts of its own. The passphrase is `merge-bench`, a
//! throwaway, with the fast test KDF.

use std::path::Path;
use std::time::Instant;

use mnemo_engine::{KdfParams, Memory, MemoryPatch, MemoryType, Mnemo, MnemoConfig, Ulid};

const PW: &str = "merge-bench";
const DIM: usize = 384;

/// A deterministic vector, never all zero.
fn vector(seed: u64) -> Vec<f32> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..DIM)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            ((x >> 40) as f32 / (1u64 << 24) as f32) - 0.5
        })
        .collect()
}

fn fact(text: String, seed: u64) -> Memory {
    Memory::new(text, MemoryType::Semantic, vector(seed))
}

/// The merge's rules alone on the pair in `dir`, three times.
fn time_rules(dir: &Path) {
    let read = |name: &str| {
        let t = Instant::now();
        let mut db = Mnemo::open_read_only(dir.join(name), PW).unwrap();
        let input = db.__merge_input_for_testing().unwrap();
        eprintln!(
            "read {name}: {} records in {:.2}s",
            input.0.len(),
            t.elapsed().as_secs_f64()
        );
        input
    };
    let (a, b) = (read("a.mnemo"), read("b.mnemo"));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    for run in 1..=3 {
        let (a, b) = (a.clone(), b.clone());
        let t = Instant::now();
        let (out, _) = Mnemo::__merge_records_for_testing(a.0, a.1, b.0, b.1, now);
        eprintln!(
            "rules run {run}: {} records in {:.2}s",
            out.len(),
            t.elapsed().as_secs_f64()
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("time") {
        time_rules(Path::new(
            args.get(2).expect("usage: merge_bench time <dir>"),
        ));
        return;
    }
    let dir = Path::new(args.get(1).expect("usage: merge_bench <dir> <records>"));
    let n: usize = args
        .get(2)
        .expect("usage: merge_bench <dir> <records>")
        .parse()
        .expect("a number of records");
    std::fs::create_dir_all(dir).unwrap();
    let base = dir.join("base.mnemo");
    let cfg = MnemoConfig {
        dimensions: DIM,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    let t = Instant::now();
    let mut db = Mnemo::create(&base, PW, cfg).unwrap();
    db.remember(Memory::scaffold_manifest(DIM)).unwrap();
    let keyed = n / 5;
    let mut plain: Vec<Ulid> = Vec::new();
    let mut keys: Vec<Ulid> = Vec::new();
    for i in 0..n {
        let text = format!("fact {i}: a base record of the benchmark brain");
        if i < keyed {
            let (id, _) = db
                .remember_keyed(fact(text, i as u64 + 1), &format!("bench.key-{i}"))
                .unwrap();
            keys.push(id);
        } else {
            plain.push(db.remember(fact(text, i as u64 + 1)).unwrap());
        }
    }
    db.close().unwrap();
    eprintln!("base: {n} records in {:.1}s", t.elapsed().as_secs_f64());

    let pct = (n / 100).max(1);
    let k_pct = (keyed / 100).max(1);
    for (side, name) in [(0usize, "a.mnemo"), (1, "b.mnemo")] {
        let t = Instant::now();
        let path = dir.join(name);
        std::fs::copy(&base, &path).unwrap();
        let mut db = Mnemo::open(&path, PW).unwrap();
        let salt = 1_000_000 * (side as u64 + 1);
        // Edits of plain facts: a's from the start, b's from the end, so
        // a few overlap in the middle when the copies are small.
        for j in 0..pct {
            let id = if side == 0 {
                plain[j]
            } else {
                plain[plain.len() - 1 - j]
            };
            db.update(
                &id,
                MemoryPatch {
                    content: Some(format!("edited in copy {side}: {j}")),
                    vector: Some(vector(salt + j as u64)),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        // New versions of keys, deletes of others, and keys the other copy
        // deletes written again.
        let (rewrite, delete, again) = if side == 0 {
            (0..k_pct, k_pct..2 * k_pct, 3 * k_pct..3 * k_pct + 100)
        } else {
            (
                keyed - k_pct..keyed,
                3 * k_pct..4 * k_pct,
                k_pct..k_pct + 100,
            )
        };
        for i in rewrite {
            db.remember_keyed(
                fact(
                    format!("key {i}, rewritten in copy {side}"),
                    salt + 500_000 + i as u64,
                ),
                &format!("bench.key-{i}"),
            )
            .unwrap();
        }
        for i in delete {
            db.delete(&keys[i]).unwrap();
        }
        for i in again.filter(|&i| i < keyed) {
            db.remember_keyed(
                fact(
                    format!("key {i}, written again in copy {side}"),
                    salt + 700_000 + i as u64,
                ),
                &format!("bench.key-{i}"),
            )
            .unwrap();
        }
        // Concurrent edits: 100 plain facts and 100 keys both copies change,
        // each its own way (conflict copies and key flags).
        for j in 0..100.min(plain.len() / 3) {
            db.update(
                &plain[plain.len() / 2 + j],
                MemoryPatch {
                    content: Some(format!("edited in both, here by copy {side}: {j}")),
                    vector: Some(vector(salt + 800_000 + j as u64)),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        for i in (keyed / 2..keyed / 2 + 100).filter(|&i| i < keyed) {
            db.remember_keyed(
                fact(
                    format!("key {i}, rewritten in both, here by copy {side}"),
                    salt + 600_000 + i as u64,
                ),
                &format!("bench.key-{i}"),
            )
            .unwrap();
        }
        for j in 0..pct {
            db.remember(fact(
                format!("new in copy {side}: {j}"),
                salt + 900_000 + j as u64,
            ))
            .unwrap();
        }
        db.close().unwrap();
        eprintln!("{name}: diverged in {:.1}s", t.elapsed().as_secs_f64());
    }
}
