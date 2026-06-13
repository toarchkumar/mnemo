//! `mnemo` — command-line interface to the Mnemo encrypted memory engine.
//!
//! Passphrases are read from `--passphrase` or, if omitted, the
//! `MNEMO_PASSPHRASE` environment variable. Passing secrets on a command line
//! is insecure (they land in shell history and process listings); prefer the
//! environment variable, and for real use prefer the library API.

use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use mnemo::{
    Memory, MemoryType, Metric, Mnemo, MnemoConfig, RecallRequest, RecallResult, Result, Ulid,
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
    ///
    /// By default a starter "scaffold" manifest memory is inserted so the
    /// new file is self-describing from birth (visible via `mnemo about`).
    /// Pass `--no-manifest` to skip it for an entirely empty file.
    Init {
        /// Path to the new `.mnemo` file.
        path: String,
        /// Embedding dimensionality.
        #[arg(long, default_value_t = 768)]
        dimensions: usize,
        /// Skip the auto-generated scaffold manifest (create an empty file).
        #[arg(long)]
        no_manifest: bool,
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
    /// Self-describing briefing — print the database's onboarding memories.
    ///
    /// Surfaces memories tagged `metadata.area = "onboarding"` (most important
    /// first), prefixed by a one-line stats summary. This is how a fresh
    /// agent — yours or someone else's — gets oriented to a `.mnemo` file
    /// using only the file itself and its passphrase, with no external docs.
    ///
    /// A canonical entry tagged `metadata.topic = "manifest"` is treated as
    /// the headline orientation point.
    About {
        /// Path to the `.mnemo` file.
        path: String,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
        /// Print only the manifest memory (topic=manifest), no other onboarding entries.
        #[arg(long)]
        manifest_only: bool,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Fetch a single memory by its ULID.
    Get {
        /// Path to the `.mnemo` file.
        path: String,
        /// ULID of the memory to fetch.
        id: String,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
        /// Show agent, session, timestamps, importance, scope, and metadata.
        #[arg(long)]
        verbose: bool,
        /// Include the embedding vector (json only; tables always omit it).
        #[arg(long)]
        vector: bool,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Browse all live memories.
    List {
        /// Path to the `.mnemo` file.
        path: String,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
        /// Restrict to these memory types (comma-separated:
        /// `episodic,semantic,procedural,working`).
        #[arg(long, value_name = "T[,T...]")]
        r#type: Option<String>,
        /// Restrict to a single agent ID.
        #[arg(long)]
        agent: Option<String>,
        /// Maximum rows to print (default: all).
        #[arg(long)]
        limit: Option<usize>,
        /// Skip this many rows before printing.
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Sort order: `created`, `importance`, or `id`.
        #[arg(long, default_value = "created")]
        sort: String,
        /// Include the embedding vector (json/jsonl only).
        #[arg(long)]
        vector: bool,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Multi-signal ranked retrieval (similarity + recency + importance + frequency).
    ///
    /// Unlike `search`, recall blends four signals, honours type and agent
    /// filters, uses the ANN index if one has been built, and updates the
    /// returned memories' access stats (persisted on the next `flush`).
    Recall {
        /// Path to the `.mnemo` file.
        path: String,
        /// Query vector as comma-separated floats (must match the DB dimensions).
        #[arg(long)]
        query: String,
        #[arg(long, default_value_t = 10)]
        top_k: usize,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
        /// Restrict to these memory types (comma-separated:
        /// `episodic,semantic,procedural,working`).
        #[arg(long, value_name = "T[,T...]")]
        r#type: Option<String>,
        /// Restrict to one agent's view (their private memories + shared).
        #[arg(long)]
        agent: Option<String>,
        /// Similarity metric: `cosine` (default), `l2`, or `dot`.
        #[arg(long, default_value = "cosine")]
        metric: String,
        /// Override IVF partitions probed (ignored without an index).
        #[arg(long)]
        n_probe: Option<usize>,
        /// Override candidates reranked exactly (ignored without an index).
        #[arg(long)]
        n_rerank: Option<usize>,
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

fn parse_ulid(s: &str) -> std::result::Result<Ulid, String> {
    Ulid::from_string(s.trim()).map_err(|e| format!("bad ULID '{s}': {e}"))
}

fn parse_memory_types(s: &str) -> std::result::Result<Vec<MemoryType>, String> {
    s.split(',')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| MemoryType::parse(t).ok_or_else(|| format!("unknown memory type '{t}'")))
        .collect()
}

fn parse_metric(s: &str) -> std::result::Result<Metric, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "cosine" | "cos" => Ok(Metric::Cosine),
        "l2" | "euclidean" => Ok(Metric::L2),
        "dot" | "ip" => Ok(Metric::Dot),
        other => Err(format!("unknown metric '{other}' (use cosine, l2, or dot)")),
    }
}

/// Output format shared by the exploration commands.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "lowercase")]
enum OutputFormat {
    /// Human-readable text rows.
    Table,
    /// One JSON document per response.
    Json,
    /// JSON Lines (one record per line) — pipeline-friendly.
    Jsonl,
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            OutputFormat::Table => "table",
            OutputFormat::Json => "json",
            OutputFormat::Jsonl => "jsonl",
        })
    }
}

/// Truncate a string to `max` chars (not bytes), suffixing `…` when cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Serialize a memory as JSON, optionally stripping the embedding.
fn memory_to_json(m: &Memory, include_vector: bool) -> serde_json::Value {
    let mut v = serde_json::to_value(m).unwrap_or_else(|_| serde_json::json!({}));
    if !include_vector {
        if let Some(obj) = v.as_object_mut() {
            obj.remove("vector");
        }
    }
    v
}

/// Print a single memory in the requested format.
fn print_memory(m: &Memory, format: OutputFormat, verbose: bool, include_vector: bool) {
    match format {
        OutputFormat::Table => {
            let id = m.id.to_string();
            let imp = format!("imp={:.2}", m.importance);
            if verbose {
                println!("{}  [{}]  agent={}  {}", id, m.memory_type.as_str(), m.agent_id, imp);
                println!("  content : {}", m.content);
                if let Some(sid) = &m.session_id {
                    println!("  session : {sid}");
                }
                println!(
                    "  scope   : {}",
                    match m.scope {
                        mnemo::Scope::Private => "private",
                        mnemo::Scope::Shared => "shared",
                    }
                );
                println!(
                    "  times   : created={}  accessed={}  access_count={}",
                    m.created_at, m.accessed_at, m.access_count
                );
                if let Some(ttl) = m.ttl_secs {
                    println!("  ttl_secs: {ttl}");
                }
                if !m.metadata.is_empty() {
                    let meta = serde_json::Value::Object(m.metadata.clone());
                    println!("  meta    : {meta}");
                }
            } else {
                println!(
                    "{}  [{}]  agent={}  {}  {}",
                    id,
                    m.memory_type.as_str(),
                    m.agent_id,
                    imp,
                    truncate(&m.content, 80),
                );
            }
        }
        OutputFormat::Json => {
            let v = memory_to_json(m, include_vector);
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".into()));
        }
        OutputFormat::Jsonl => {
            let v = memory_to_json(m, include_vector);
            println!("{}", serde_json::to_string(&v).unwrap_or_else(|_| "{}".into()));
        }
    }
}

/// Print a list of memories with a header line in table mode.
fn print_memories(items: &[Memory], total: usize, format: OutputFormat, include_vector: bool) {
    match format {
        OutputFormat::Table => {
            if items.is_empty() {
                println!("no memories");
                return;
            }
            println!("showing {} of {} memories", items.len(), total);
            for m in items {
                print_memory(m, OutputFormat::Table, false, false);
            }
        }
        OutputFormat::Json => {
            let arr: Vec<_> = items.iter().map(|m| memory_to_json(m, include_vector)).collect();
            let doc = serde_json::json!({ "total": total, "count": arr.len(), "memories": arr });
            println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".into()));
        }
        OutputFormat::Jsonl => {
            for m in items {
                println!(
                    "{}",
                    serde_json::to_string(&memory_to_json(m, include_vector))
                        .unwrap_or_else(|_| "{}".into())
                );
            }
        }
    }
}

/// Print recall hits with the score/similarity columns up front.
fn print_recall_hits(hits: &[RecallResult], format: OutputFormat) {
    match format {
        OutputFormat::Table => {
            if hits.is_empty() {
                println!("no results");
                return;
            }
            for h in hits {
                println!(
                    "score={:.3}  sim={:.3}  [{}]  {}  {}",
                    h.score,
                    h.similarity,
                    h.memory.memory_type.as_str(),
                    h.memory.id,
                    truncate(&h.memory.content, 80),
                );
            }
        }
        OutputFormat::Json => {
            let arr: Vec<_> = hits
                .iter()
                .map(|h| {
                    serde_json::json!({
                        "score": h.score,
                        "similarity": h.similarity,
                        "memory": memory_to_json(&h.memory, false),
                    })
                })
                .collect();
            let doc = serde_json::json!({ "count": arr.len(), "hits": arr });
            println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".into()));
        }
        OutputFormat::Jsonl => {
            for h in hits {
                let line = serde_json::json!({
                    "score": h.score,
                    "similarity": h.similarity,
                    "memory": memory_to_json(&h.memory, false),
                });
                println!("{}", serde_json::to_string(&line).unwrap_or_else(|_| "{}".into()));
            }
        }
    }
}

fn run() -> std::result::Result<(), String> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { path, dimensions, no_manifest, passphrase: pp } => {
            let pp = passphrase(&pp)?;
            let cfg = MnemoConfig { dimensions, ..Default::default() };
            let mut db = Mnemo::create(&path, &pp, cfg).map_err(fmt)?;
            if !no_manifest {
                let manifest = Memory::scaffold_manifest(dimensions);
                db.remember(manifest).map_err(fmt)?;
                db.flush().map_err(fmt)?;
            }
            db.close().map_err(fmt)?;
            if no_manifest {
                println!("created {path} ({dimensions} dimensions, encrypted, no manifest)");
            } else {
                println!(
                    "created {path} ({dimensions} dimensions, encrypted) with scaffold manifest"
                );
                println!("  → run `mnemo about {path}` to view it");
                println!("  → replace it with one that records your embedder and conventions");
            }
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
        Command::About { path, format, manifest_only, passphrase: pp } => {
            let pp = passphrase(&pp)?;
            let mut db = Mnemo::open(&path, &pp).map_err(fmt)?;
            let stats = db.stats().map_err(fmt)?;
            let snapshots = db.snapshots().len();
            let mut entries = db.about().map_err(fmt)?;
            if manifest_only {
                entries.retain(|m| {
                    m.metadata
                        .get("topic")
                        .and_then(|v| v.as_str())
                        .map(|s| s.eq_ignore_ascii_case("manifest"))
                        .unwrap_or(false)
                });
            }
            match format {
                OutputFormat::Json | OutputFormat::Jsonl => {
                    let manifest = entries.iter().find(|m| {
                        m.metadata
                            .get("topic")
                            .and_then(|v| v.as_str())
                            .map(|s| s.eq_ignore_ascii_case("manifest"))
                            .unwrap_or(false)
                    });
                    let doc = serde_json::json!({
                        "path": path,
                        "stats": {
                            "memories": stats.memories,
                            "dimensions": stats.dimensions,
                            "file_bytes": stats.file_bytes,
                            "encrypted": stats.encrypted,
                            "snapshots": snapshots,
                            "has_index": stats.index.is_some(),
                        },
                        "manifest": manifest.map(|m| memory_to_json(m, false)),
                        "onboarding": entries
                            .iter()
                            .map(|m| memory_to_json(m, false))
                            .collect::<Vec<_>>(),
                    });
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".into())
                    );
                }
                OutputFormat::Table => {
                    // One-line stats header — what kind of file you're looking at.
                    println!(
                        "# {}  ({} memories · {}-dim · {} · {} snapshots{})",
                        path,
                        stats.memories,
                        stats.dimensions,
                        if stats.encrypted { "encrypted" } else { "plaintext" },
                        snapshots,
                        if stats.index.is_some() { " · ANN index built" } else { "" },
                    );
                    if entries.is_empty() {
                        println!();
                        println!("(no onboarding memories — this database has no self-description.)");
                        println!("To make a database self-describing, store a memory with");
                        println!("  metadata = {{\"area\": \"onboarding\", \"topic\": \"manifest\"}}");
                        println!("that introduces the project, embedder, and conventions.");
                    } else {
                        println!();
                        println!("## Onboarding briefing ({} entries, most important first)", entries.len());
                        for m in &entries {
                            let topic = m
                                .metadata
                                .get("topic")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let is_scaffold = m
                                .metadata
                                .get("scaffold")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            let head = if topic.eq_ignore_ascii_case("manifest") {
                                if is_scaffold {
                                    format!(
                                        "MANIFEST (scaffold — please replace)  (importance={:.2})",
                                        m.importance
                                    )
                                } else {
                                    format!("MANIFEST  (importance={:.2})", m.importance)
                                }
                            } else if topic.is_empty() {
                                format!("(importance={:.2})", m.importance)
                            } else {
                                format!("[{}]  (importance={:.2})", topic, m.importance)
                            };
                            println!();
                            println!("### {head}");
                            println!("{}", m.content);
                        }
                        println!();
                        println!("## Quick start");
                        println!("  mnemo list   {path}                 # browse all live memories");
                        println!("  mnemo recall {path} --query VEC     # multi-signal recall");
                        println!("  mnemo get    {path} <ulid> --verbose # fetch one memory");
                    }
                }
            }
        }
        Command::Get { path, id, format, verbose, vector, passphrase: pp } => {
            let pp = passphrase(&pp)?;
            let ulid = parse_ulid(&id)?;
            let mut db = Mnemo::open(&path, &pp).map_err(fmt)?;
            let m = db.get(&ulid).map_err(fmt)?;
            print_memory(&m, format, verbose, vector);
        }
        Command::List {
            path,
            format,
            r#type,
            agent,
            limit,
            offset,
            sort,
            vector,
            passphrase: pp,
        } => {
            let pp = passphrase(&pp)?;
            let types = match r#type {
                Some(s) => Some(parse_memory_types(&s)?),
                None => None,
            };
            let sort = sort.trim().to_ascii_lowercase();
            if !matches!(sort.as_str(), "created" | "importance" | "id") {
                return Err(format!(
                    "unknown sort '{sort}' (use created, importance, or id)"
                ));
            }
            let mut db = Mnemo::open(&path, &pp).map_err(fmt)?;
            let mut all = db.memories().map_err(fmt)?;

            // Filter.
            if let Some(ts) = &types {
                all.retain(|m| ts.contains(&m.memory_type));
            }
            if let Some(a) = &agent {
                all.retain(|m| &m.agent_id == a);
            }
            let total_after_filter = all.len();

            // Friendly nudge for unbounded browses on large stores.
            if limit.is_none() && total_after_filter > 10_000 {
                eprintln!(
                    "warning: {} memories — consider --limit; printing all anyway",
                    total_after_filter
                );
            }

            // Sort.
            match sort.as_str() {
                "created" => all.sort_by_key(|m| m.created_at),
                "importance" => {
                    all.sort_by(|a, b| b.importance.total_cmp(&a.importance));
                }
                "id" => all.sort_by_key(|m| m.id),
                _ => unreachable!(),
            }

            // Paginate.
            let start = offset.min(all.len());
            let end = match limit {
                Some(n) => (start + n).min(all.len()),
                None => all.len(),
            };
            let page: Vec<Memory> = all[start..end].to_vec();
            print_memories(&page, total_after_filter, format, vector);
        }
        Command::Recall {
            path,
            query,
            top_k,
            format,
            r#type,
            agent,
            metric,
            n_probe,
            n_rerank,
            passphrase: pp,
        } => {
            let pp = passphrase(&pp)?;
            let q = parse_vector(&query)?;
            let metric = parse_metric(&metric)?;
            let mut req = RecallRequest::new(q).top_k(top_k).metric(metric);
            if let Some(s) = r#type {
                req = req.types(parse_memory_types(&s)?);
            }
            if let Some(a) = agent {
                req = req.agent(a);
            }
            if let Some(n) = n_probe {
                req = req.n_probe(n);
            }
            if let Some(n) = n_rerank {
                req = req.n_rerank(n);
            }
            let mut db = Mnemo::open(&path, &pp).map_err(fmt)?;
            let hits = db.recall(&req).map_err(fmt)?;
            print_recall_hits(&hits, format);
        }
        Command::Demo { path } => demo(&path).map_err(fmt)?,
        Command::Snapshots { path, passphrase: pp } => {
            let pp = passphrase(&pp)?;
            let db = Mnemo::open(&path, &pp).map_err(fmt)?;
            let snaps = db.snapshots();
            if snaps.is_empty() {
                println!("no snapshots yet (nothing has been flushed)");
            } else {
                println!("{:<8}  {:<12}  committed (unix)", "txn", "memories");
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
