//! `mnemo` — command-line interface to the Mnemo encrypted memory engine.
//!
//! Passphrases are read from `--passphrase` or, if omitted, the
//! `MNEMO_PASSPHRASE` environment variable. Passing secrets on a command line
//! is insecure (they land in shell history and process listings); prefer the
//! environment variable, and for real use prefer the library API.

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use mnemo::{
    Memory, MemoryType, Mnemo, MnemoConfig, RecallRequest, Result,
};

/// Encrypted, single-file agent-memory engine.
#[derive(Parser)]
#[command(name = "mnemo", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new empty encrypted database.
    Init {
        /// Path to the new `.mnemo` file.
        path: String,
        /// Embedding dimensionality.
        #[arg(long, default_value_t = 768)]
        dimensions: usize,
        /// Passphrase (else `MNEMO_PASSPHRASE`).
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Print database statistics.
    Info {
        /// Path to the `.mnemo` file.
        path: String,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Re-encrypt the data key under a new passphrase.
    Rekey {
        /// Path to the `.mnemo` file.
        path: String,
        #[arg(long)]
        passphrase: Option<String>,
        /// The new passphrase.
        #[arg(long)]
        new_passphrase: String,
    },
    /// Rebuild the file, dropping tombstoned and expired memories.
    Compact {
        /// Path to the `.mnemo` file.
        path: String,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Decrypt and re-verify every live record.
    Verify {
        /// Path to the `.mnemo` file.
        path: String,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Import memories from a JSON Lines file.
    ///
    /// Each line: {"content": "...", "vector": [..], "memory_type": "semantic",
    /// "agent_id": "...", "importance": 0.5}. Only `content` and `vector` are
    /// required.
    Import {
        /// Path to the `.mnemo` file.
        path: String,
        /// JSONL file to import.
        file: String,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Build, rebuild, or drop the approximate-nearest-neighbour index.
    Index {
        /// Path to the `.mnemo` file.
        path: String,
        #[arg(long)]
        passphrase: Option<String>,
        /// Drop the existing index instead of building one.
        #[arg(long)]
        drop: bool,
        /// IVF partitions (0 = auto, ~sqrt of memory count).
        #[arg(long, default_value_t = 0)]
        partitions: usize,
        /// PQ subspaces (0 = auto, ~8 dims each).
        #[arg(long, default_value_t = 0)]
        subspaces: usize,
        /// Partitions probed per query.
        #[arg(long, default_value_t = 8)]
        n_probe: usize,
        /// Candidates reranked exactly per query.
        #[arg(long, default_value_t = 64)]
        n_rerank: usize,
    },
    /// Run an exact nearest-neighbour search.
    Search {
        /// Path to the `.mnemo` file.
        path: String,
        /// Query vector as comma-separated floats, e.g. `0.1,0.2,0.9`.
        #[arg(long)]
        query: String,
        #[arg(long, default_value_t = 5)]
        top_k: usize,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Create a small database and exercise the full lifecycle.
    Demo {
        /// Where to write the demo database.
        #[arg(long, default_value = "demo.mnemo")]
        path: String,
    },
    /// List the restorable snapshots (one per committed flush).
    Snapshots {
        /// Path to the `.mnemo` file.
        path: String,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Restore the database to a past snapshot (point-in-time recovery).
    Restore {
        /// Path to the `.mnemo` file.
        path: String,
        /// Restore to this transaction id (see `snapshots`).
        #[arg(long, conflicts_with = "to_time")]
        to_txn: Option<u64>,
        /// Restore to the latest snapshot at or before this unix timestamp.
        #[arg(long)]
        to_time: Option<i64>,
        #[arg(long)]
        passphrase: Option<String>,
    },
}

fn passphrase(arg: &Option<String>) -> std::result::Result<String, String> {
    if let Some(p) = arg {
        return Ok(p.clone());
    }
    std::env::var("MNEMO_PASSPHRASE").map_err(|_| {
        "no passphrase: pass --passphrase or set MNEMO_PASSPHRASE".to_string()
    })
}

fn parse_vector(s: &str) -> std::result::Result<Vec<f32>, String> {
    s.split(',')
        .map(|t| t.trim().parse::<f32>().map_err(|e| format!("bad float '{t}': {e}")))
        .collect()
}

fn run() -> std::result::Result<(), String> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { path, dimensions, passphrase: pp } => {
            let pp = passphrase(&pp)?;
            let cfg = MnemoConfig { dimensions, ..Default::default() };
            let mut db = Mnemo::create(&path, &pp, cfg).map_err(fmt)?;
            db.close().map_err(fmt)?;
            println!("created {path} ({dimensions} dimensions, encrypted)");
        }
        Command::Info { path, passphrase: pp } => {
            let pp = passphrase(&pp)?;
            let mut db = Mnemo::open(&path, &pp).map_err(fmt)?;
            let s = db.stats().map_err(fmt)?;
            println!("path:        {path}");
            println!("memories:    {}", s.memories);
            println!("tombstoned:  {}", s.deleted);
            println!("dimensions:  {}", s.dimensions);
            println!("file bytes:  {}", s.file_bytes);
            println!("encrypted:   {}", s.encrypted);
            println!(
                "wal region:  {} pages ({} KiB)",
                s.wal_pages,
                s.wal_pages * 8
            );
            println!("created at:  {}", s.created_at);
            println!("agents:      {}", s.agents.join(", "));
            println!("snapshots:   {}", db.snapshots().len());
            match s.index {
                Some(ix) => {
                    println!(
                        "ann index:   {} vectors, {} partitions, {} PQ subspaces",
                        ix.vectors, ix.partitions, ix.subspaces
                    );
                    println!(
                        "  defaults:  n_probe={}, n_rerank={}",
                        ix.n_probe, ix.n_rerank
                    );
                }
                None => println!("ann index:   none (recall uses exact scan)"),
            }
        }
        Command::Rekey { path, passphrase: pp, new_passphrase } => {
            let pp = passphrase(&pp)?;
            let mut db = Mnemo::open(&path, &pp).map_err(fmt)?;
            db.rekey(&new_passphrase, mnemo::KdfParams::secure()).map_err(fmt)?;
            db.close().map_err(fmt)?;
            println!("rekeyed {path}");
        }
        Command::Compact { path, passphrase: pp } => {
            let pp = passphrase(&pp)?;
            let report = Mnemo::compact_file(&path, &pp).map_err(fmt)?;
            println!("compacted {path}: {} -> {} live memories", report.before, report.after);
        }
        Command::Verify { path, passphrase: pp } => {
            let pp = passphrase(&pp)?;
            let mut db = Mnemo::open(&path, &pp).map_err(fmt)?;
            let n = db.verify().map_err(fmt)?;
            println!("verified {n} records — all pages decrypt and decode");
        }
        Command::Import { path, file, passphrase: pp } => {
            let pp = passphrase(&pp)?;
            let mut db = Mnemo::open(&path, &pp).map_err(fmt)?;
            let text = std::fs::read_to_string(&file)
                .map_err(|e| format!("cannot read {file}: {e}"))?;
            let mut count = 0usize;
            for (lineno, line) in text.lines().enumerate() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let m = memory_from_json(line)
                    .map_err(|e| format!("line {}: {e}", lineno + 1))?;
                db.remember(m).map_err(fmt)?;
                count += 1;
            }
            db.flush().map_err(fmt)?;
            println!("imported {count} memories into {path}");
        }
        Command::Index {
            path,
            passphrase: pp,
            drop,
            partitions,
            subspaces,
            n_probe,
            n_rerank,
        } => {
            let pp = passphrase(&pp)?;
            let mut db = Mnemo::open(&path, &pp).map_err(fmt)?;
            if drop {
                db.drop_index();
                db.flush().map_err(fmt)?;
                println!("dropped ANN index on {path}");
            } else {
                let cfg = mnemo::IndexConfig {
                    n_partitions: partitions,
                    pq_subspaces: subspaces,
                    n_probe,
                    n_rerank,
                    ..Default::default()
                };
                let info = db.build_index_with(cfg).map_err(fmt)?;
                db.flush().map_err(fmt)?;
                println!(
                    "built ANN index on {path}: {} vectors, {} partitions, {} PQ subspaces",
                    info.vectors, info.partitions, info.subspaces
                );
            }
        }
        Command::Search { path, query, top_k, passphrase: pp } => {
            let pp = passphrase(&pp)?;
            let q = parse_vector(&query)?;
            let mut db = Mnemo::open(&path, &pp).map_err(fmt)?;
            let hits = db.search(&q, top_k, mnemo::Metric::Cosine).map_err(fmt)?;
            if hits.is_empty() {
                println!("no results");
            }
            for (m, sim) in hits {
                println!("{sim:.4}  [{}]  {}", m.memory_type.as_str(), m.content);
            }
        }
        Command::Demo { path } => demo(&path).map_err(fmt)?,
        Command::Snapshots { path, passphrase: pp } => {
            let pp = passphrase(&pp)?;
            let db = Mnemo::open(&path, &pp).map_err(fmt)?;
            let snaps = db.snapshots();
            if snaps.is_empty() {
                println!("no snapshots yet (nothing has been flushed)");
            } else {
                println!("{:<8}  {:<12}  {}", "txn", "memories", "committed (unix)");
                for s in snaps {
                    println!(
                        "{:<8}  {:<12}  {}",
                        s.txn_id, s.memory_count, s.created_at
                    );
                }
            }
        }
        Command::Restore { path, to_txn, to_time, passphrase: pp } => {
            let pp = passphrase(&pp)?;
            let mut db = Mnemo::open(&path, &pp).map_err(fmt)?;
            let info = match (to_txn, to_time) {
                (Some(txn), _) => db.restore_to(txn).map_err(fmt)?,
                (None, Some(t)) => db.restore_to_time(t).map_err(fmt)?,
                (None, None) => {
                    return Err("restore needs --to-txn or --to-time".to_string())
                }
            };
            println!(
                "restored {path} to snapshot txn {} ({} memories)",
                info.txn_id, info.memory_count
            );
        }
    }
    Ok(())
}

/// Build a [`Memory`] from one JSONL line.
fn memory_from_json(line: &str) -> std::result::Result<Memory, String> {
    use serde_json::Value;
    let v: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let content = v.get("content").and_then(Value::as_str)
        .ok_or("missing string field 'content'")?;
    let vec_json = v.get("vector").and_then(Value::as_array)
        .ok_or("missing array field 'vector'")?;
    let vector: Vec<f32> = vec_json.iter()
        .map(|x| x.as_f64().map(|f| f as f32).ok_or("vector element not a number"))
        .collect::<std::result::Result<_, _>>()?;
    let mt = v.get("memory_type").and_then(Value::as_str)
        .and_then(MemoryType::parse)
        .unwrap_or(MemoryType::Semantic);
    let mut m = Memory::new(content, mt, vector);
    if let Some(a) = v.get("agent_id").and_then(Value::as_str) {
        m = m.with_agent(a);
    }
    if let Some(i) = v.get("importance").and_then(Value::as_f64) {
        m = m.with_importance(i as f32);
    }
    Ok(m)
}

fn fmt(e: mnemo::MnemoError) -> String {
    e.to_string()
}

/// Self-contained end-to-end demonstration.
fn demo(path: &str) -> Result<()> {
    let _ = std::fs::remove_file(path);
    let pp = "demo-passphrase";
    let cfg = MnemoConfig { dimensions: 4, ..Default::default() };

    println!("creating encrypted database at {path} ...");
    let mut db = Mnemo::create(path, pp, cfg)?;

    let seeds = [
        ("user prefers concise answers", MemoryType::Semantic, [0.9, 0.1, 0.0, 0.1], 0.9),
        ("user is based in Berlin", MemoryType::Semantic, [0.8, 0.2, 0.1, 0.0], 0.7),
        ("ran the deploy script at 14:00", MemoryType::Episodic, [0.1, 0.9, 0.1, 0.0], 0.4),
        ("to reset state, call clear() then reload", MemoryType::Procedural, [0.0, 0.1, 0.9, 0.2], 0.6),
        ("scratch: temp calculation result", MemoryType::Working, [0.2, 0.2, 0.2, 0.9], 0.2),
    ];
    for (content, mt, v, imp) in seeds {
        db.remember(
            Memory::new(content, mt, v.to_vec())
                .with_agent("assistant")
                .with_importance(imp),
        )?;
    }
    db.flush()?;
    println!("stored {} memories, flushed to disk\n", db.len());

    let info = db.build_index()?;
    db.flush()?;
    println!(
        "built ANN index: {} vectors, {} partitions, {} PQ subspaces",
        info.vectors, info.partitions, info.subspaces
    );

    println!("\nrecall for a query close to 'user preferences' (index-accelerated):");
    let req = RecallRequest::new(vec![0.85, 0.15, 0.05, 0.05]).top_k(3);
    for h in db.recall(&req)? {
        println!(
            "  score={:.3}  sim={:.3}  [{}]  {}",
            h.score, h.similarity, h.memory.memory_type.as_str(), h.memory.content
        );
    }

    println!("\nreopening to confirm persistence and encryption ...");
    db.close()?;
    let db = Mnemo::open(path, pp)?;
    println!("reopened: {} memories survive the round-trip", db.len());

    let raw = std::fs::read(path).unwrap_or_default();
    let needle = b"user prefers concise answers";
    let leaked = raw.windows(needle.len()).any(|w| w == needle);
    println!("plaintext 'user prefers concise answers' present in file bytes: {leaked}");

    println!("\ndemo complete.");
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
