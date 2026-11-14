//! `mnemo` — command-line interface to the Mnemo encrypted memory engine.
//!
//! Passphrases are read from `--passphrase` or, if omitted, the
//! `MNEMO_PASSPHRASE` environment variable. Passing secrets on a command line
//! is insecure (they land in shell history and process listings); prefer the
//! environment variable, and for real use prefer the library API.

use std::process::ExitCode;

#[path = "mnemo/export.rs"]
mod export;
#[path = "mnemo/inspect.rs"]
mod inspect;

use clap::{Parser, Subcommand, ValueEnum};
use mnemo_engine::{
    Memory, MemoryType, Metric, Mnemo, MnemoConfig, RecallRequest, RecallResult, Result, Ulid,
};

/// Encrypted, single-file agent-memory engine.
#[derive(Parser)]
#[command(name = "mnemo", version, about, long_about = None)]
struct Cli {
    /// How long to wait for the brain's lock when another process holds it
    /// (12.4 shared mode): `10s`, `500ms`, `2m`, or a number of seconds.
    /// Reads run alongside other readers, and readers come first. A hook
    /// that only reads should pass `--wait 2s`; one that saves keeps the
    /// default, since busy readers can hold a write longer.
    #[arg(long, global = true, value_name = "DURATION", default_value = "10s",
          value_parser = parse_wait)]
    wait: std::time::Duration,
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
        /// Passphrase (else `MNEMO_PASSPHRASE`, or `--ask-passphrase`).
        /// With none of the three, `init` generates a key instead.
        #[arg(long, conflicts_with_all = ["ask_passphrase", "key_file"])]
        passphrase: Option<String>,
        /// Ask for a passphrase twice, without echo, instead of generating
        /// a key (what `init` did before v0.15).
        #[arg(long, conflicts_with = "key_file")]
        ask_passphrase: bool,
        /// Generate a key and write it to this new file (mode 0600), instead
        /// of keeping it in the keychain.
        #[arg(long, value_name = "PATH")]
        key_file: Option<String>,
    },
    /// Show a brain's recovery code, from the key store.
    ///
    /// It's shown only to a person at a terminal: stdout and stdin must be
    /// terminals, no agent harness may be driving it, and the code appears
    /// on the controlling terminal after you press Enter, then is cleared.
    RecoveryCode {
        /// Path to the `.mnemo` file.
        path: String,
    },
    /// Print database statistics.
    Info {
        /// Path to the `.mnemo` file.
        path: String,
        /// Unlock with a raw key (32 bytes, or 64 hex digits) instead of a
        /// passphrase.
        #[arg(long, value_name = "PATH", conflicts_with = "passphrase")]
        key_file: Option<String>,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Re-encrypt the data key under a new passphrase.
    ///
    /// The new passphrase comes from (in priority order) `--new-passphrase`,
    /// the `MNEMO_NEW_PASSPHRASE` env var, or an interactive double-prompt
    /// without echo. The old passphrase follows the same priority chain as
    /// every other command.
    ///
    /// A plain rekey changes the secret that opens this file from now on;
    /// anyone who had the file and the old secret can still read it and
    /// later writes. Use `--rotate` to revoke (after a suspected passphrase
    /// leak, say). Rotation can't erase old ciphertext that flash storage
    /// still holds in blocks it hasn't reused.
    Rekey {
        /// Path to the `.mnemo` file.
        path: String,
        /// Unlock with a raw key (32 bytes, 64 hex digits or a recovery
        /// code) instead of a passphrase.
        #[arg(long, value_name = "PATH", conflicts_with = "passphrase")]
        key_file: Option<String>,
        #[arg(long)]
        passphrase: Option<String>,
        /// The new passphrase. Omit to be prompted twice without echo.
        #[arg(long, conflicts_with = "new_key")]
        new_passphrase: Option<String>,
        /// Generate a new key instead of a new passphrase, keep it in the
        /// key store, and show its recovery code (at a terminal).
        #[arg(long)]
        new_key: bool,
        /// Also keep a new passphrase in the key store.
        #[arg(long, conflicts_with = "new_key")]
        store: bool,
        /// Also rotate the data key: rekey, then compact, which rewrites the
        /// file under a fresh data key. The old passphrase then opens no
        /// copy of the file made from now on, and no earlier copy opens with
        /// the new one. Takes as long as `compact` (history is dropped).
        #[arg(long)]
        rotate: bool,
    },
    /// Rebuild the file, dropping tombstoned and expired memories. Writes
    /// v9, so this is also how a v8 (or older) file is converted: since
    /// v0.13 a v8 file opens read-only until it's compacted.
    Compact {
        /// Path to the `.mnemo` file.
        path: String,
        /// Unlock with a raw key (32 bytes, or 64 hex digits) instead of a
        /// passphrase.
        #[arg(long, value_name = "PATH", conflicts_with = "passphrase")]
        key_file: Option<String>,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Decrypt and re-verify every live record.
    Verify {
        /// Path to the `.mnemo` file.
        path: String,
        /// Unlock with a raw key (32 bytes, or 64 hex digits) instead of a
        /// passphrase.
        #[arg(long, value_name = "PATH", conflicts_with = "passphrase")]
        key_file: Option<String>,
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
    /// Store a single memory from text, embedding it via an external
    /// command (Phase 3). The command is spawned once; it reads one
    /// JSON string per line from stdin and writes one JSON float
    /// array per line to stdout. See `--embed-cmd` for the full
    /// contract.
    Remember {
        /// Path to the `.mnemo` file.
        path: String,
        /// Content to remember.
        #[arg(long)]
        text: String,
        /// Memory type: `semantic`, `episodic`, `procedural`, or
        /// `working`.
        #[arg(long, default_value = "semantic")]
        r#type: String,
        /// Optional importance weight (0.0–1.0; defaults to 0.5).
        #[arg(long)]
        importance: Option<f32>,
        /// Optional agent id to scope the memory to.
        #[arg(long)]
        agent: Option<String>,
        /// Optional JSON object of extra metadata key/values.
        #[arg(long)]
        metadata: Option<String>,
        /// Phase 12.1: optional upsert key. When set, this `remember`
        /// behaves as a keyed write: if a live memory already holds this
        /// `(scope_owner, key)` slot, the prior record is marked
        /// `_mnemo.superseded_by` and a new record is inserted; if the
        /// incoming content, type, importance and user metadata match
        /// the live record exactly, the write is a no-op and nothing is
        /// changed. Prefer `--key` for facts that replace earlier ones
        /// (versions, current status, user preferences). Prints the id
        /// plus one of `added`/`superseded`/`unchanged` on stdout.
        #[arg(long)]
        key: Option<String>,
        /// Embedder command. Shell-style quoted but **not** executed
        /// via a shell — e.g. `"python3 embedder.py"` or `"my-embed
        /// --model=foo"`. Protocol: child reads one JSON string per
        /// line from stdin, writes one JSON array of floats per line
        /// to stdout, flushes after each reply.
        #[arg(long)]
        embed_cmd: String,
        /// Embedder identity recorded on the scaffold manifest. The
        /// first `remember`/`recall` on a fresh file writes this
        /// name; later calls on the same file must pass the same
        /// name or `set_embedder` refuses with an identity mismatch.
        #[arg(long)]
        embed_name: String,
        /// Whether the embedder returns unit-normalized vectors
        /// (defaults to `true`; set `--embed-normalize=false` for
        /// embedders that return raw dot-space vectors).
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        embed_normalize: bool,
        /// Per-request timeout for the embedder (seconds; default 60).
        /// A silent-but-alive child trips this deadline deterministically
        /// (PR 10b.2). A short timeout bounds CLI/MCP latency when the
        /// embedder misbehaves.
        #[arg(long, default_value_t = 60)]
        embed_timeout: u64,
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
        #[arg(long, conflicts_with = "rebuild")]
        drop: bool,
        /// Rebuild the ANN index from scratch using the auto-defaults
        /// for the current corpus size (re-clusters centroids and
        /// retrains the PQ codebook, undoing the cluster drift that
        /// accumulates as memories are inserted against fixed
        /// centroids). Required to pick up the ANN-1.1 defaults on an
        /// index built by v0.5.x — the shipped BP.1 fix wires this up
        /// so the v0.6.0 upgrade note's `mnemo index --rebuild` /
        /// `rebuild_index()` commands actually exist. Mutually
        /// exclusive with `--drop`.
        #[arg(long, conflicts_with = "drop")]
        rebuild: bool,
        /// IVF partitions (0 = auto, ~sqrt of memory count).
        #[arg(long, default_value_t = 0)]
        partitions: usize,
        /// PQ subspaces (0 = auto, ~8 dims each).
        #[arg(long, default_value_t = 0)]
        subspaces: usize,
        /// Partitions probed per query (0 = auto; computed from the
        /// partition count on every recall).
        #[arg(long, default_value_t = 0)]
        n_probe: usize,
        /// Candidates reranked exactly per query (0 = auto; scales
        /// with the live vector count).
        #[arg(long, default_value_t = 0)]
        n_rerank: usize,
    },
    /// Run an exact nearest-neighbour search.
    ///
    /// Provide the query vector via exactly one of `--query` (inline) or
    /// `--query-file` (file path; use `-` for stdin). Files may contain a
    /// JSON array of floats, comma-separated floats, or whitespace /
    /// newline-separated floats — the parser auto-detects.
    Search {
        /// Path to the `.mnemo` file.
        path: String,
        /// Query vector as comma-separated floats, e.g. `0.1,0.2,0.9`.
        /// Mutually exclusive with `--query-file`.
        #[arg(long, conflicts_with = "query_file")]
        query: Option<String>,
        /// Path to a file containing the query vector (`-` for stdin).
        /// Accepts a JSON array, comma-separated floats, or one float
        /// per line. Mutually exclusive with `--query`.
        #[arg(long, conflicts_with = "query")]
        query_file: Option<String>,
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
        /// Unlock with a raw key (32 bytes, or 64 hex digits) instead of a
        /// passphrase.
        #[arg(long, value_name = "PATH", conflicts_with = "passphrase")]
        key_file: Option<String>,
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
        /// Unlock with a raw key (32 bytes, or 64 hex digits) instead of a
        /// passphrase.
        #[arg(long, value_name = "PATH", conflicts_with = "passphrase")]
        key_file: Option<String>,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Export the live memories as readable plaintext (Markdown or JSON
    /// Lines). Opens the file read-only. Writes to stdout unless `-o` is
    /// given, and warns on stderr that the output isn't encrypted. Vectors
    /// are left out; `--git` makes the output diff-clean (ordered by key,
    /// then id; no thread turns and no access stats).
    Export {
        /// Path to the `.mnemo` file.
        path: String,
        /// Output format.
        #[arg(long, value_enum, default_value_t = export::ExportFormat::Markdown)]
        format: export::ExportFormat,
        /// Restrict to these memory types (comma-separated:
        /// `episodic,semantic,procedural,working`).
        #[arg(long, value_name = "T[,T...]")]
        r#type: Option<String>,
        /// Restrict to a single agent ID.
        #[arg(long)]
        agent: Option<String>,
        /// Deterministic, diff-clean output for version control: the same
        /// unchanged file always exports to the same bytes.
        #[arg(long)]
        git: bool,
        /// Write to this file instead of stdout. It's replaced whole (a new
        /// file renamed over it), and owner-only on Unix even if it existed
        /// with a wider mode.
        #[arg(long, short = 'o')]
        output: Option<String>,
        /// Unlock with a raw key (32 bytes, or 64 hex digits) instead of a
        /// passphrase.
        #[arg(long, value_name = "PATH", conflicts_with = "passphrase")]
        key_file: Option<String>,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Browse the file in a local, read-only web view.
    ///
    /// Reads the file once (read-only) and closes it, so a writer is never
    /// blocked; "Reload" in the page reads it again. Serves on 127.0.0.1
    /// only, at a URL carrying a random token, and only GET: nothing in the
    /// view can change the file. Press Ctrl-C to stop.
    Inspect {
        /// Path to the `.mnemo` file.
        path: String,
        /// Port on 127.0.0.1 (default: a random free one).
        #[arg(long)]
        port: Option<u16>,
        /// Open the page in the default browser. The browser gets a
        /// private local page that forwards to the URL (deleted once it has
        /// loaded), not the URL itself. On Linux, a browser installed as a
        /// snap or a flatpak (Ubuntu's default Firefox is a snap) has its
        /// own private /tmp and can't read that page: copy the printed link
        /// instead.
        #[arg(long)]
        open: bool,
        /// Stop after this long without a request from the page: `30m`,
        /// `90s`, `1h`, or `0` for never. The server then wipes the secret
        /// and exits.
        #[arg(long, default_value = "30m", value_name = "DURATION")]
        idle_timeout: String,
        /// Unlock with a raw key (32 bytes, or 64 hex digits) instead of a
        /// passphrase.
        #[arg(long, value_name = "PATH", conflicts_with = "passphrase")]
        key_file: Option<String>,
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
        /// Query vector as comma-separated floats. Mutually exclusive with
        /// `--query-file` and `--text`. Must match the DB's dimensionality.
        #[arg(long, conflicts_with_all = ["query_file", "text"])]
        query: Option<String>,
        /// Path to a file containing the query vector (`-` for stdin).
        /// Accepts a JSON array, comma-separated floats, or one float
        /// per line. Mutually exclusive with `--query` and `--text`.
        #[arg(long, conflicts_with_all = ["query", "text"])]
        query_file: Option<String>,
        /// Query text to embed via `--embed-cmd` and `--embed-name`
        /// (Phase 3). Without an embedder, a v9 file matches it by keyword
        /// instead (keyword-only recall); a v8 file needs the embedder.
        /// Mutually exclusive with `--query`/`--query-file`.
        #[arg(long, conflicts_with_all = ["query", "query_file"])]
        text: Option<String>,
        /// FMT-9d (v9 files): identifiers or exact terms to match (a
        /// version, a PR number, a function name), not the question. Added
        /// to vector recall as a BM25 term (hybrid recall); on its own it
        /// is keyword-only recall.
        #[arg(long)]
        keywords: Option<String>,
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
        /// Skip access-tracking (does not bump `accessed_at` /
        /// `access_count`). Opens the file with a shared OS lock so the
        /// command can run alongside a writer holding the exclusive lock.
        #[arg(long)]
        no_track: bool,
        /// Phase 12.6: restrict recall to this session_id.
        #[arg(long)]
        session: Option<String>,
        /// Phase 12.6: repeatable metadata filter, applied before
        /// scoring. Shorthand forms, matched left-to-right:
        ///
        /// * `key=value`           — metadata[key] equals value
        /// * `key:exists`          — metadata[key] is present
        /// * `key:in=a,b,c`        — metadata[key] is one of the comma-split values
        /// * `key:gte=NUM` / `key:lte=NUM` — numeric lower / upper bound
        ///
        /// Values are parsed as JSON when the leading char is one of
        /// `[`, `{`, `"`, `-`, `0..9`, `t`, `f`, or `n` (true / false /
        /// null); otherwise treated as a string. Multiple `--filter`s
        /// AND together. For complex / nested filters, use the Python
        /// or MCP APIs.
        #[arg(long = "filter", value_name = "EXPR")]
        filter: Vec<String>,
        /// Phase 12.8: cap the total UTF-8 bytes of returned `content`
        /// fields. Stops adding hits once the running sum would exceed
        /// this many bytes. The top-1 hit is always returned, even if
        /// it alone is over budget. A trailing "(truncated: N hits
        /// dropped by --max-bytes)" line goes to stderr whenever the
        /// budget cuts something. Roughly 4 bytes per token for
        /// English text.
        #[arg(long)]
        max_bytes: Option<usize>,
        /// Embedder command for `--text` (same contract as
        /// `mnemo remember --embed-cmd`). Ignored when `--query` /
        /// `--query-file` is used.
        #[arg(long, requires = "text")]
        embed_cmd: Option<String>,
        /// Embedder identity for `--text`; must match the manifest.
        #[arg(long, requires = "text")]
        embed_name: Option<String>,
        /// Whether the embedder returns unit-normalized vectors.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        embed_normalize: bool,
        /// Per-request timeout for the embedder (seconds; default 60).
        #[arg(long, default_value_t = 60)]
        embed_timeout: u64,
        /// Unlock with a raw key (32 bytes, or 64 hex digits) instead of a
        /// passphrase.
        #[arg(long, value_name = "PATH", conflicts_with = "passphrase")]
        key_file: Option<String>,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Create a small database and exercise the full lifecycle.
    Demo {
        /// Where to write the demo database.
        #[arg(long, default_value = "demo.mnemo")]
        path: String,
    },
    /// Edit a memory in place (Phase 12.1). Keeps the ULID, `created_at`
    /// and access stats; any of `--text`, `--importance`, `--metadata`,
    /// `--ttl-secs` may change. `--text` re-embeds through the registered
    /// embedder (`--embed-cmd` + `--embed-name` required). An identical
    /// patch is a no-op and prints `unchanged`.
    Update {
        /// Path to the `.mnemo` file.
        path: String,
        /// ULID of the memory to update.
        id: String,
        /// New content text; re-embeds through the embedder you supply
        /// via `--embed-cmd` + `--embed-name`.
        #[arg(long)]
        text: Option<String>,
        /// New importance in `[0, 1]`.
        #[arg(long)]
        importance: Option<f32>,
        /// Replace user-visible metadata with this JSON object. Reserved
        /// `_mnemo.*` keys on the record are preserved. Pass `{}` to
        /// clear user metadata.
        #[arg(long)]
        metadata: Option<String>,
        /// Set the TTL (seconds). Pass `--clear-ttl` to drop any
        /// existing TTL instead.
        #[arg(long, conflicts_with = "clear_ttl")]
        ttl_secs: Option<i64>,
        /// Clear any TTL on this memory.
        #[arg(long)]
        clear_ttl: bool,
        /// Embedder command for `--text` (same contract as
        /// `mnemo remember --embed-cmd`).
        #[arg(long, requires = "text")]
        embed_cmd: Option<String>,
        /// Embedder identity for `--text`; must match the manifest.
        #[arg(long, requires = "text")]
        embed_name: Option<String>,
        /// Whether the embedder returns unit-normalized vectors.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        embed_normalize: bool,
        /// Per-request embedder timeout in seconds.
        #[arg(long, default_value_t = 60)]
        embed_timeout: u64,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Walk the full chain for a keyed slot (Phase 12.1), newest first.
    /// Entry 0 is the live record (or empty if the slot is empty);
    /// subsequent entries are the superseded history. Slot resolution
    /// matches `mnemo remember --key`: without `--agent` or `--shared`,
    /// walks the default agent's private slot (v0.8.1 bugfix); pass
    /// `--shared` for the shared slot.
    History {
        /// Path to the `.mnemo` file.
        path: String,
        /// Keyed slot name (passed to `mnemo remember --key` /
        /// `remember_keyed`).
        #[arg(long)]
        key: String,
        /// Query the private slot for this agent. Omit for the default
        /// agent's private slot (pre-v0.8.1 this flag was required for
        /// a working chain; it's now optional).
        #[arg(long, conflicts_with = "shared")]
        agent: Option<String>,
        /// Query the shared slot (one per file across every agent)
        /// instead of a private slot. Mutually exclusive with `--agent`.
        #[arg(long, conflicts_with = "agent")]
        shared: bool,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Phase 12.7a thread archive: ingest a transcript into a
    /// thread's archive layer, export it back, or forget it. The
    /// archive is encrypted at rest like any record; `thread forget`
    /// soft-deletes (compact to reclaim space permanently).
    Thread {
        #[command(subcommand)]
        cmd: ThreadCmd,
    },
    /// Phase 12.7b essence (timeline + sections). `render` is the
    /// default subcommand. Section ops take `--section <name>` from
    /// {intent, conventions, decisions, state, open, next, lessons};
    /// timeline entries use `append`.
    Essence {
        #[command(subcommand)]
        cmd: EssenceCmd,
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
    /// Serve the `.mnemo` file as a Model Context Protocol server. Reads
    /// JSON-RPC 2.0 requests line-by-line from stdin, writes responses
    /// to stdout, and logs diagnostics to stderr. Passphrase must be
    /// provided via `MNEMO_PASSPHRASE` (no CLI-flag fallback — that
    /// would leak into shell history and process listings).
    Serve {
        /// Path to the `.mnemo` file.
        path: String,
        /// Speak the Model Context Protocol (stdio transport). Required
        /// today — the flag exists so future transports (HTTP, SSE, ...)
        /// can co-opt `serve` without breaking scripts.
        #[arg(long)]
        mcp: bool,
        /// Embedder command. When set, the MCP `remember`/`recall`/
        /// `cache_put_semantic`/`cache_get_semantic` tools accept a
        /// `text` param in addition to `vector`; the embedder is
        /// started once at serve start and reused for every call.
        /// Same contract as `mnemo remember --embed-cmd`.
        #[arg(long, requires = "embed_name")]
        embed_cmd: Option<String>,
        /// Embedder identity (recorded on the scaffold manifest).
        #[arg(long, requires = "embed_cmd")]
        embed_name: Option<String>,
        /// Whether the embedder returns unit-normalized vectors.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        embed_normalize: bool,
        /// Per-request timeout for the embedder (seconds; default 60).
        /// Prevents a stuck child from freezing the MCP agent.
        #[arg(long, default_value_t = 60)]
        embed_timeout: u64,
        /// 12.4 shared mode: hold no lock between tool calls, so other
        /// servers, the CLI and Python can use the file meanwhile; each tool
        /// call takes the lock for itself, waiting up to `--wait`. Off by
        /// default: without it the server holds the file for its whole life,
        /// as before.
        #[arg(long)]
        shared: bool,
        /// With `--shared`: use shared mode on a network or synced folder
        /// anyway, where the lock may not reach every process.
        #[arg(long, requires = "shared")]
        allow_network_fs: bool,
    },
    /// Result-cache management (Phase 10.1). One `.mnemo` file is
    /// both an agent-memory store AND a durable key/value cache;
    /// these commands manipulate the cache side.
    Cache {
        #[command(subcommand)]
        op: CacheOp,
    },
}

/// Subcommands under `mnemo thread` (Phase 12.7a archive layer).
#[derive(Subcommand)]
enum ThreadCmd {
    /// Ingest a transcript file into a thread's archive. Idempotent
    /// by per-turn `turn_id`: running it again on a longer transcript
    /// appends only the new turns. Requires an embedder (`--embed-cmd`
    /// + `--embed-name`) because the ingest batch-embeds every turn.
    Ingest {
        /// Path to the `.mnemo` file.
        path: String,
        /// Thread id to append under. New if no prior ingest used it.
        #[arg(long)]
        thread: String,
        /// Transcript file (one JSON object per line). `-` reads stdin.
        #[arg(long)]
        transcript: String,
        /// Transcript format: `jsonl` (the generic shape) or
        /// `claude-code-jsonl` (`~/.claude/projects/<p>/<s>.jsonl`).
        #[arg(long, default_value = "jsonl")]
        format: String,
        /// Embedder command (same contract as `mnemo remember --embed-cmd`).
        #[arg(long)]
        embed_cmd: String,
        /// Embedder identity recorded on the scaffold manifest.
        #[arg(long)]
        embed_name: String,
        /// Whether the embedder returns unit-normalized vectors.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        embed_normalize: bool,
        /// Per-request embedder timeout in seconds.
        #[arg(long, default_value_t = 60)]
        embed_timeout: u64,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Export a thread's archive back to a JSON-lines transcript. The
    /// output shape matches the generic `--format jsonl` input, so a
    /// round trip (ingest → export) is byte-equal for turns that
    /// weren't truncated or chunked.
    Export {
        /// Path to the `.mnemo` file.
        path: String,
        /// Thread id to export.
        #[arg(long)]
        thread: String,
        /// Output file. Default: stdout.
        #[arg(long, short = 'o')]
        output: Option<String>,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Soft-delete every record in a thread's archive. Compact later
    /// to reclaim space permanently.
    Forget {
        /// Path to the `.mnemo` file.
        path: String,
        /// Thread id to forget.
        #[arg(long)]
        thread: String,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Phase 12.7c: start a new thread that continues from a previous
    /// one. Writes a link record (`_mnemo.continues`); `mnemo essence
    /// --thread <new>` then pulls the prior thread's open items,
    /// `next` and last-N timeline entries ahead of its own essence.
    Start {
        /// Path to the `.mnemo` file.
        path: String,
        /// New thread id.
        new: String,
        /// Previous thread id to continue from.
        #[arg(long)]
        from: String,
        #[arg(long)]
        passphrase: Option<String>,
    },
}

/// Subcommands under `mnemo essence` (Phase 12.7b).
#[derive(Subcommand)]
enum EssenceCmd {
    /// Render the deterministic markdown view of a thread's essence
    /// (or the project layer with `--project`). Prints to stdout.
    Render {
        /// Path to the `.mnemo` file.
        path: String,
        /// Thread id. Omit with `--project` for the project layer.
        #[arg(long, conflicts_with = "project")]
        thread: Option<String>,
        /// Render the project layer instead of a thread.
        #[arg(long, conflicts_with = "thread")]
        project: bool,
        /// Byte budget for the render (default 6144).
        #[arg(long, default_value_t = 6144)]
        budget: usize,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Add a new section item (intent/conventions/decisions/state/
    /// open/next/lessons). Status starts at `open`.
    Add {
        /// Path to the `.mnemo` file.
        path: String,
        /// Section name.
        #[arg(long)]
        section: String,
        /// Item text (≤ 280 bytes).
        #[arg(long)]
        text: String,
        /// Thread id. Omit with `--project` for the project layer.
        #[arg(long, conflicts_with = "project")]
        thread: Option<String>,
        /// Project-scoped item (every thread's render shows it).
        #[arg(long, conflicts_with = "thread")]
        project: bool,
        /// Optional refs: comma-separated turn ids, memory ids, commits, paths.
        #[arg(long)]
        refs: Option<String>,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Append a new timeline entry (append-only, ≤ 600 bytes).
    Append {
        /// Path to the `.mnemo` file.
        path: String,
        /// Entry text (≤ 600 bytes).
        #[arg(long)]
        text: String,
        /// Thread id. Omit with `--project` for the project-level
        /// timeline (rare — most milestones belong to a thread).
        #[arg(long, conflicts_with = "project")]
        thread: Option<String>,
        #[arg(long, conflicts_with = "thread")]
        project: bool,
        #[arg(long)]
        refs: Option<String>,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Update an existing section item's text in place.
    Update {
        /// Path to the `.mnemo` file.
        path: String,
        /// ULID of the section item to update.
        id: String,
        /// New text (≤ 280 bytes).
        #[arg(long)]
        text: String,
        /// Optional replacement refs.
        #[arg(long)]
        refs: Option<String>,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Mark a section item resolved (`open → resolved`).
    Resolve {
        /// Path to the `.mnemo` file.
        path: String,
        /// ULID of the section item to resolve.
        id: String,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Soft-delete a section item. Compact to reclaim space.
    Remove {
        /// Path to the `.mnemo` file.
        path: String,
        /// ULID of the section item to remove.
        id: String,
        #[arg(long)]
        passphrase: Option<String>,
    },
}

/// Subcommands under `mnemo cache`. Each operates on a `--ns` namespace
/// inside the given `.mnemo` file. Passphrase resolves via `--passphrase`
/// or `MNEMO_PASSPHRASE` (same rules as top-level commands).
#[derive(Subcommand)]
enum CacheOp {
    /// Look up a key; prints the value to stdout on hit, non-zero
    /// exit on miss.
    Get {
        /// Path to the `.mnemo` file.
        path: String,
        /// Cache namespace.
        #[arg(long)]
        ns: String,
        /// Cache key (SHA-256 hashed by the engine).
        #[arg(long)]
        key: String,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Insert or overwrite an entry. Either `--value <string>` (UTF-8)
    /// or `--file <path>` (raw bytes) must be provided.
    Put {
        /// Path to the `.mnemo` file.
        path: String,
        #[arg(long)]
        ns: String,
        #[arg(long)]
        key: String,
        /// Inline UTF-8 value. Mutually exclusive with `--file`.
        #[arg(long, conflicts_with = "file")]
        value: Option<String>,
        /// Path to a file whose bytes become the value. Mutually
        /// exclusive with `--value`.
        #[arg(long, conflicts_with = "value")]
        file: Option<String>,
        /// Free-form content-type label recorded on the entry.
        #[arg(long, default_value = "text")]
        content_type: String,
        /// TTL in seconds. Omit for no expiry.
        #[arg(long)]
        ttl: Option<u64>,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Tombstone an entry. Space reclaimed by `mnemo compact`.
    Delete {
        /// Path to the `.mnemo` file.
        path: String,
        #[arg(long)]
        ns: String,
        #[arg(long)]
        key: String,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Print cache statistics as JSON. `--ns` restricts to one
    /// namespace; without it the whole cache is summarized.
    Stats {
        /// Path to the `.mnemo` file.
        path: String,
        #[arg(long)]
        ns: Option<String>,
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Tombstone entries in bulk. `--ns` restricts to one namespace.
    /// `--expired` restricts to TTL-expired entries only; otherwise
    /// every live entry in the scope is tombstoned.
    Purge {
        /// Path to the `.mnemo` file.
        path: String,
        #[arg(long)]
        ns: Option<String>,
        #[arg(long)]
        expired: bool,
        #[arg(long)]
        passphrase: Option<String>,
    },
}

/// Resolve the passphrase from (in priority order) the `--passphrase` flag,
/// the `MNEMO_PASSPHRASE` env var, or an interactive TTY prompt without
/// echo. Non-interactive paths (env var, `--passphrase`) keep working so
/// scripts and CI don't break.
///
/// When `--passphrase` is used the passphrase ends up in shell history and
/// process listings; we warn once to stderr to make that obvious.
/// FMT-9f: the hint `info` and `about` print for a v8 file.
fn upgrade_hint(path: &str, format_version: u16) -> Option<String> {
    (format_version < 9).then(|| {
        format!(
            "v{format_version} file: run `mnemo compact {path}` to upgrade to v9 \
             (upgrade every tool that reads it first)"
        )
    })
}

fn passphrase(arg: &Option<String>) -> std::result::Result<String, String> {
    if let Some(p) = arg {
        eprintln!(
            "warning: passing the passphrase as `--passphrase` is insecure; \
             it lands in shell history and process listings. Prefer the \
             MNEMO_PASSPHRASE env var, or omit both to be prompted."
        );
        return Ok(p.clone());
    }
    if let Ok(p) = std::env::var("MNEMO_PASSPHRASE") {
        return Ok(p);
    }
    // No flag, no env var. If stdin is a TTY, prompt without echo. If it
    // isn't (e.g. a CI runner without a real terminal), bail with the
    // historical error message so existing scripts that forgot to set
    // MNEMO_PASSPHRASE get the same diagnostic they used to.
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Err("no passphrase: pass --passphrase or set MNEMO_PASSPHRASE".to_string());
    }
    rpassword::prompt_password("Passphrase: ")
        .map_err(|e| format!("failed to read passphrase: {e}"))
}

/// How long a command waits for the brain's lock (`--wait`, default 10 s).
static WAIT: std::sync::OnceLock<std::time::Duration> = std::sync::OnceLock::new();

/// 12.4: the lock layer's options for this command: `--wait` as the
/// deadline.
fn wait_opts() -> mnemo_engine::SharedOptions {
    let mut o = mnemo_engine::SharedOptions::default();
    if let Some(w) = WAIT.get() {
        o.deadline = *w;
    }
    o
}

/// `--wait`'s value: `10s`, `500ms`, `2m`, or a bare number of seconds.
fn parse_wait(s: &str) -> std::result::Result<std::time::Duration, String> {
    let s = s.trim();
    let (num, unit) = match s.find(|c: char| c.is_ascii_alphabetic()) {
        Some(i) => s.split_at(i),
        None => (s, "s"),
    };
    let n: f64 = num
        .trim()
        .parse()
        .map_err(|_| format!("not a duration: {s:?} (try 10s, 500ms or 2m)"))?;
    let secs = match unit.trim() {
        "ms" => n / 1000.0,
        "s" => n,
        "m" => n * 60.0,
        other => return Err(format!("unknown unit {other:?} in {s:?} (use ms, s or m)")),
    };
    if !secs.is_finite() || secs < 0.0 {
        return Err(format!("not a duration: {s:?}"));
    }
    Ok(std::time::Duration::from_secs_f64(secs.min(1e9)))
}

/// 12.4: a read-only open through the lock layer: it waits up to `--wait`
/// for a writer instead of failing at once, and runs alongside other
/// readers.
fn open_r(path: &str, s: &inspect::Secret) -> mnemo_engine::Result<Mnemo> {
    open_ro(path, s)
}

/// 12.4: a read-write open through the lock layer, for one command's
/// write: it waits up to `--wait` with writer preference, and names `op`
/// to a shared caller that times out meanwhile.
fn open_w(path: &str, s: &inspect::Secret, op: &str) -> mnemo_engine::Result<Mnemo> {
    open_rw(path, s, op)
}

/// A started embedder and the vectors it made.
type Embedded = (mnemo_engine::EmbedCmd, Vec<Vec<f32>>);

/// 12.4 (slow work first, unlocked): start the embedder and embed `texts`
/// before the brain's lock is taken, so a slow embedder never keeps other
/// sessions waiting. The identity's width comes from the file's header (no
/// key, no lock); when that can't be read (a torn header), `None`, and the
/// caller embeds after its write open, as before.
fn embed_first(
    path: &str,
    cmd: &str,
    name: &str,
    normalize: bool,
    timeout: u64,
    texts: &[&str],
) -> std::result::Result<Option<Embedded>, String> {
    use mnemo_engine::Embedder;
    let Ok(dimensions) = Mnemo::peek_dimensions(path) else {
        return Ok(None);
    };
    let embedder = new_embedder(cmd, name, normalize, timeout, dimensions)?;
    let vectors = embedder.embed(texts).map_err(fmt)?;
    Ok(Some((embedder, vectors)))
}

fn new_embedder(
    cmd: &str,
    name: &str,
    normalize: bool,
    timeout: u64,
    dimensions: usize,
) -> std::result::Result<mnemo_engine::EmbedCmd, String> {
    let identity = mnemo_engine::EmbedderIdentity {
        name: name.to_string(),
        dimensions,
        normalize,
    };
    Ok(mnemo_engine::EmbedCmd::new(cmd, identity)
        .map_err(fmt)?
        .with_timeout(std::time::Duration::from_secs(timeout)))
}

/// A read-only open with either secret (waiting, as [`open_r`]).
fn open_ro(path: &str, s: &inspect::Secret) -> mnemo_engine::Result<Mnemo> {
    match s {
        inspect::Secret::Passphrase(p) => Mnemo::open_read_only_waiting(path, p, wait_opts()),
        inspect::Secret::Key(k) => Mnemo::open_read_only_waiting_with_key(path, k, wait_opts()),
    }
    .map(warn_notices)
}

/// 12.3: say what the handle has to tell its owner (a passphrase on raw-key
/// KDF parameters), once, on stderr.
fn warn_notices(db: Mnemo) -> Mnemo {
    for n in db.notices() {
        eprintln!("warning: {n}");
    }
    db
}

/// 12.3: where this command's secret came from, and the next source of the
/// same kind that's present, for `info` and for a wrong secret's error.
static SOURCE: std::sync::OnceLock<(String, Option<String>)> = std::sync::OnceLock::new();

/// 12.3 (the keys note, section 6): the secret for the existing brain at
/// `path`. A flag names its source outright. Otherwise the chain picks by
/// the file's kind: `MNEMO_KEY`, `MNEMO_KEY_FILE`, `MNEMO_KEY_COMMAND` or
/// the key store for a key; `MNEMO_PASSPHRASE` or the key store for a
/// passphrase, then the prompt at a terminal.
fn secret_for(
    path: &str,
    pp: &Option<String>,
    key_file: &Option<String>,
) -> std::result::Result<inspect::Secret, String> {
    use mnemo_engine::keys::{self, Resolution, SecretKind};
    if let Some(k) = key_file {
        let _ = SOURCE.set((format!("--key-file ({k})"), None));
        return Ok(inspect::Secret::Key(inspect::read_key_file(k)?));
    }
    if pp.is_some() {
        let _ = SOURCE.set(("--passphrase".into(), None));
        return Ok(inspect::Secret::Passphrase(zeroize::Zeroizing::new(
            passphrase(pp)?,
        )));
    }
    let store = keys::default_store();
    match keys::resolve(path, store.as_deref(), keys::CLI_COMMAND_TIMEOUT).map_err(fmt)? {
        Resolution::Found(r) => {
            for n in &r.notes {
                eprintln!("note: {n}");
            }
            let _ = SOURCE.set((r.source.to_string(), r.next.map(|s| s.to_string())));
            Ok(r.secret)
        }
        Resolution::NoSource { kind, notes } => {
            for n in &notes {
                eprintln!("note: {n}");
            }
            match kind {
                SecretKind::Passphrase => {
                    let p = passphrase(&None)?;
                    let _ = SOURCE.set(("the prompt".into(), None));
                    Ok(inspect::Secret::Passphrase(zeroize::Zeroizing::new(p)))
                }
                SecretKind::Key => Err(format!(
                    "no key for {path}: it uses a key, and none of MNEMO_KEY, MNEMO_KEY_FILE, \
                     MNEMO_KEY_COMMAND or {} has one; pass --key-file, or --passphrase if it \
                     was given a passphrase",
                    store
                        .as_deref()
                        .map(|s| s.describe())
                        .unwrap_or_else(|| "a key store (none in this build yet)".into())
                )),
            }
        }
    }
}

/// A read-write open with either secret (waiting, as [`open_w`]).
fn open_rw(path: &str, s: &inspect::Secret, op: &str) -> mnemo_engine::Result<Mnemo> {
    match s {
        inspect::Secret::Passphrase(p) => Mnemo::open_exclusive(path, p, op, wait_opts()),
        inspect::Secret::Key(k) => Mnemo::open_exclusive_with_key(path, k, op, wait_opts()),
    }
    .map(warn_notices)
}

/// Like [`passphrase`] but for *new* passphrases on `init` / `rekey`:
/// double-prompts and rejects mismatches. Non-interactive paths (`--flag`,
/// env var) skip the double-prompt and use the value directly.
fn new_passphrase(arg: &Option<String>, env_var: &str) -> std::result::Result<String, String> {
    if let Some(p) = arg {
        eprintln!(
            "warning: passing the passphrase as a flag is insecure; it lands \
             in shell history and process listings. Prefer the {env_var} env \
             var, or omit both to be prompted twice."
        );
        return Ok(p.clone());
    }
    if let Ok(p) = std::env::var(env_var) {
        return Ok(p);
    }
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Err(format!("no passphrase: pass the flag or set {env_var}"));
    }
    prompt_new_passphrase()
}

/// Ask for a new passphrase twice, without echo (`init --ask-passphrase`,
/// and `new_passphrase` at a terminal).
fn prompt_new_passphrase() -> std::result::Result<String, String> {
    let first = rpassword::prompt_password("New passphrase: ")
        .map_err(|e| format!("failed to read passphrase: {e}"))?;
    let second = rpassword::prompt_password("Confirm passphrase: ")
        .map_err(|e| format!("failed to read passphrase: {e}"))?;
    if first != second {
        return Err("passphrases do not match".to_string());
    }
    Ok(first)
}

/// 12.3 (the keys note, section 7): show a new key's recovery code under the
/// display rule, or say why it wasn't shown and how to see it later.
fn show_recovery_code(path: &str, key: &[u8; 32]) {
    use mnemo_engine::keys;
    match keys::display::refusal_reason() {
        None => {
            if let Err(e) = keys::display::show(&keys::recovery::encode(key)) {
                eprintln!(
                    "note: the recovery code couldn't be shown ({e}); run `mnemo recovery-code \
                     {path}` in a terminal of your own"
                );
            }
        }
        Some(why) => eprintln!(
            "note: {} ({why})",
            keys::display::REFUSAL.replace("<file>", path)
        ),
    }
}

/// Resolve a query vector from CLI input, accepting exactly one of:
///
/// - `inline`: comma-separated floats from `--query`
/// - `file`: path to a file (or `-` for stdin) from `--query-file`,
///   containing a JSON array, comma-separated floats, or whitespace /
///   newline-separated floats (auto-detected)
///
/// Clap's `conflicts_with` enforces the "not both" half; this fn enforces
/// the "at least one" half. Wrap the `(query, query_file)` pair from any
/// CLI variant in one call: `let vec = resolve_query(query, query_file)?;`
fn resolve_query(
    inline: Option<String>,
    file: Option<String>,
) -> std::result::Result<Vec<f32>, String> {
    let text = match (inline, file) {
        (Some(s), None) => s,
        (None, Some(p)) => {
            if p == "-" {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin()
                    .read_to_string(&mut buf)
                    .map_err(|e| format!("reading stdin: {e}"))?;
                buf
            } else {
                std::fs::read_to_string(&p).map_err(|e| format!("reading {p}: {e}"))?
            }
        }
        (Some(_), Some(_)) => {
            // Clap's conflicts_with should catch this before we get here.
            return Err("pass only one of --query or --query-file".into());
        }
        (None, None) => {
            return Err("missing query: pass --query <floats> or --query-file <path|->".into());
        }
    };
    parse_query_text(&text)
}

/// Parse a query vector from a text blob. Tries JSON-array form first if
/// the input starts with `[`; otherwise splits on commas, whitespace, and
/// newlines and parses each non-empty token as `f32`.
fn parse_query_text(text: &str) -> std::result::Result<Vec<f32>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("query text was empty".into());
    }
    if trimmed.starts_with('[') {
        return serde_json::from_str::<Vec<f32>>(trimmed)
            .map_err(|e| format!("invalid JSON array of floats: {e}"));
    }
    trimmed
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(|t| {
            t.parse::<f32>()
                .map_err(|e| format!("bad float '{t}': {e}"))
        })
        .collect()
}

fn parse_ulid(s: &str) -> std::result::Result<Ulid, String> {
    Ulid::from_string(s.trim()).map_err(|e| format!("bad ULID '{s}': {e}"))
}

/// Phase 12.6: parse one `--filter` expression and append it to `f`.
///
/// Grammar (left-to-right first match wins):
///
/// * `key:exists`           → [`FilterTerm::Exists`]
/// * `key:in=a,b,c`         → [`FilterTerm::In`] over the comma-split values
/// * `key:gte=NUM`          → [`FilterTerm::Range`] with `min = Some(NUM)`
/// * `key:lte=NUM`          → [`FilterTerm::Range`] with `max = Some(NUM)`
/// * `key=value`            → [`FilterTerm::Eq`] with the value parsed below
///
/// Values are tried as JSON first (so numbers, booleans, null, arrays
/// and quoted strings round-trip through serde cleanly); anything else
/// is treated as a plain string.
fn parse_filter_expr(
    expr: &str,
    f: mnemo_engine::Filter,
) -> std::result::Result<mnemo_engine::Filter, String> {
    // `key:exists` — no `=` at all.
    if let Some(key) = expr.strip_suffix(":exists") {
        if key.is_empty() {
            return Err(format!("empty key in --filter '{expr}'"));
        }
        return Ok(f.metadata_exists(key.to_string()));
    }
    // Everything else needs a `=`.
    let (lhs, rhs) = expr
        .split_once('=')
        .ok_or_else(|| format!("bad --filter '{expr}': expected key=value or key:op=value"))?;

    // Split off a `:op` suffix on the lhs (e.g. `score:gte`).
    let (key, op) = match lhs.split_once(':') {
        Some((k, o)) => (k, Some(o)),
        None => (lhs, None),
    };
    if key.is_empty() {
        return Err(format!("empty key in --filter '{expr}'"));
    }

    match op {
        Some("in") => {
            let values: Vec<serde_json::Value> = rhs
                .split(',')
                .map(|s| parse_filter_value(s.trim()))
                .collect();
            Ok(f.metadata_in(key.to_string(), values))
        }
        Some("gte") => {
            let n: f64 = rhs
                .parse()
                .map_err(|e| format!("bad :gte number in --filter '{expr}': {e}"))?;
            Ok(f.metadata_range(key.to_string(), Some(n), None))
        }
        Some("lte") => {
            let n: f64 = rhs
                .parse()
                .map_err(|e| format!("bad :lte number in --filter '{expr}': {e}"))?;
            Ok(f.metadata_range(key.to_string(), None, Some(n)))
        }
        Some(other) => Err(format!(
            "unknown operator ':{other}' in --filter '{expr}' (want :exists, :in, :gte, :lte)"
        )),
        None => Ok(f.metadata_eq(key.to_string(), parse_filter_value(rhs))),
    }
}

/// Parse one filter value. Try JSON first so numbers, booleans, null and
/// quoted strings deserialize cleanly; otherwise fall back to a plain
/// string.
fn parse_filter_value(s: &str) -> serde_json::Value {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
        v
    } else {
        serde_json::Value::String(s.to_string())
    }
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
                println!(
                    "{}  [{}]  agent={}  {}",
                    id,
                    m.memory_type.as_str(),
                    m.agent_id,
                    imp
                );
                println!("  content : {}", m.content);
                if let Some(sid) = &m.session_id {
                    println!("  session : {sid}");
                }
                println!(
                    "  scope   : {}",
                    match m.scope {
                        mnemo_engine::Scope::Private => "private",
                        mnemo_engine::Scope::Shared => "shared",
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
            println!(
                "{}",
                serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".into())
            );
        }
        OutputFormat::Jsonl => {
            let v = memory_to_json(m, include_vector);
            println!(
                "{}",
                serde_json::to_string(&v).unwrap_or_else(|_| "{}".into())
            );
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
            let arr: Vec<_> = items
                .iter()
                .map(|m| memory_to_json(m, include_vector))
                .collect();
            let doc = serde_json::json!({ "total": total, "count": arr.len(), "memories": arr });
            println!(
                "{}",
                serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".into())
            );
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
            println!(
                "{}",
                serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".into())
            );
        }
        OutputFormat::Jsonl => {
            for h in hits {
                let line = serde_json::json!({
                    "score": h.score,
                    "similarity": h.similarity,
                    "memory": memory_to_json(&h.memory, false),
                });
                println!(
                    "{}",
                    serde_json::to_string(&line).unwrap_or_else(|_| "{}".into())
                );
            }
        }
    }
}

fn run() -> std::result::Result<(), String> {
    let cli = Cli::parse();
    let _ = WAIT.set(cli.wait);
    match cli.command {
        Command::Init {
            path,
            dimensions,
            no_manifest,
            passphrase: pp,
            ask_passphrase,
            key_file,
        } => {
            use mnemo_engine::keys;
            let cfg = MnemoConfig {
                dimensions,
                ..Default::default()
            };
            let mut write_manifest = |db: &mut Mnemo| -> Result<()> {
                if !no_manifest {
                    db.remember(Memory::scaffold_manifest(dimensions))?;
                    db.flush()?;
                }
                Ok(())
            };
            let env_pp = std::env::var("MNEMO_PASSPHRASE").is_ok_and(|v| !v.is_empty());
            let how = if pp.is_some() || ask_passphrase || env_pp {
                // A passphrase, as before v0.15. The double prompt catches
                // a typo, which would make the file unopenable; the flag and
                // the env var trust the caller.
                let pp = if ask_passphrase {
                    prompt_new_passphrase()?
                } else {
                    new_passphrase(&pp, "MNEMO_PASSPHRASE")?
                };
                let mut db = Mnemo::create(&path, &pp, cfg).map_err(fmt)?;
                write_manifest(&mut db).map_err(fmt)?;
                db.close().map_err(fmt)?;
                "encrypted with a passphrase".to_string()
            } else if let Some(kf) = &key_file {
                // 12.3: a generated key, in a new file the user named.
                if std::fs::symlink_metadata(kf).is_ok() {
                    return Err(format!("{kf} already exists; nothing was created"));
                }
                let mut persist =
                    |_salt: &[u8; 16], key: &[u8; 32]| keys::write_new_key_file(kf, key);
                keys::create_generated(&path, cfg, &mut write_manifest, &mut persist)
                    .map_err(fmt)?;
                format!("encrypted with a generated key, written to {kf} (mode 0600)")
            } else {
                // 12.3: a generated key, kept in the key store.
                let Some(store) = keys::default_store() else {
                    return Err(format!(
                        "no keychain is available here, so `mnemo init` can't keep a generated \
                         key, and nothing was created. To protect {path} with a passphrase, as \
                         v0.14 did: `mnemo init --ask-passphrase {path}` (or set \
                         MNEMO_PASSPHRASE). To write a generated key to a new file (mode 0600): \
                         `mnemo init --key-file <PATH> {path}`"
                    ));
                };
                let label = path.clone();
                let mut persist = |salt: &[u8; 16], key: &[u8; 32]| {
                    let entry = keys::StoredSecret::Key(zeroize::Zeroizing::new(*key));
                    keys::store_and_check(&*store, salt, &entry, &label)
                };
                let key = keys::create_generated(&path, cfg, &mut write_manifest, &mut persist)
                    .map_err(fmt)?;
                println!(
                    "created {path} ({dimensions} dimensions), encrypted with a generated key"
                );
                println!("  → the key is in {}", store.describe());
                show_recovery_code(&path, &key);
                if !no_manifest {
                    println!("  → run `mnemo about {path}` to view its scaffold manifest");
                }
                return Ok(());
            };
            if no_manifest {
                println!("created {path} ({dimensions} dimensions, {how}, no manifest)");
            } else {
                println!("created {path} ({dimensions} dimensions, {how}) with scaffold manifest");
                println!("  → run `mnemo about {path}` to view it");
                println!("  → replace it with one that records your embedder and conventions");
            }
        }
        Command::RecoveryCode { path } => {
            use mnemo_engine::keys::{self, StoredSecret};
            let store = keys::default_store().ok_or_else(|| {
                "no keychain is available here, so there's no stored key to show".to_string()
            })?;
            let info = keys::file_info(&path).map_err(fmt)?;
            let entry = store
                .get(&info.salt)
                .map_err(fmt)?
                .ok_or_else(|| format!("{} has no entry for {path}", store.describe()))?;
            let key = match entry {
                StoredSecret::Key(k) => k,
                StoredSecret::Passphrase(_) => {
                    return Err(format!(
                        "{path} uses a passphrase; there's no recovery code"
                    ))
                }
            };
            if !Mnemo::key_opens(&path, &key).map_err(fmt)? {
                return Err(format!(
                    "the key {} holds for {path} doesn't open it, so its code isn't shown",
                    store.describe()
                ));
            }
            if let Some(why) = keys::display::refusal_reason() {
                return Err(format!(
                    "{} ({why})",
                    keys::display::REFUSAL.replace("<file>", &path)
                ));
            }
            keys::display::show(&keys::recovery::encode(&key))
                .map_err(|e| format!("showing the recovery code: {e}"))?;
        }
        Command::Info {
            path,
            key_file,
            passphrase: pp,
        } => {
            // Build info first — printed before the file open so it's
            // visible even if the open fails (common when verifying a
            // fresh install of mnemo). Mirrors the Python
            // `mnemo.build_info()` shape.
            let bi = mnemo_engine::build_info();
            println!("mnemo:       v{} ({})", bi.version, bi.target);
            println!("aes:         {}", bi.aes_backend);
            println!("polyval:     {}", bi.polyval_backend);
            let secret = secret_for(&path, &pp, &key_file)?;
            let mut db = open_ro(&path, &secret).map_err(fmt)?;
            let s = db.stats().map_err(fmt)?;
            println!("path:        {path}");
            // 12.3: where the secret came from, and the KDF's grade.
            if let Some((src, _)) = SOURCE.get() {
                println!("key source:  {src}");
            }
            if let Some(note) = mnemo_engine::keys::kdf_grade_note(db.kdf_params(), secret.kind()) {
                println!("kdf:         {note}");
            }
            println!("format:      v{}", s.format_version);
            if let Some(hint) = upgrade_hint(&path, s.format_version) {
                println!("             {hint}");
            }
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
            println!("recall:      {}", s.recall_mode.as_str());
            println!("created at:  {}", s.created_at);
            println!("agents:      {}", s.agents.join(", "));
            // INSP-1 (RB-001.25): what a restore can reach right now.
            match s.oldest_restorable_at {
                Some(t) => println!(
                    "snapshots:   {} retained, oldest {} ({} bytes pinned)",
                    s.snapshots_retained,
                    export::rfc3339(t),
                    s.snapshot_pinned_bytes
                ),
                None => println!("snapshots:   none"),
            }
            // Phase 6 instrumentation: how many fsyncs the most recent
            // flush issued. `0` on a read-only open since open() never
            // flushes; shown as a dash in that case for clarity.
            if s.fsyncs_last_flush == 0 {
                println!("fsyncs/flush: — (no flush seen on this handle)");
            } else {
                println!("fsyncs/flush: {}", s.fsyncs_last_flush);
            }
            // Phase 10.1 result-cache summary — one line even when
            // the cache is empty, for symmetry with the other fields.
            {
                let cs = db.cache_stats(None);
                println!(
                    "cache:       {} entries, {} bytes (hits {}, misses {}, evict {})",
                    cs.entries, cs.bytes, cs.hits, cs.misses, cs.evictions
                );
            }
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
        Command::Rekey {
            path,
            key_file,
            passphrase: pp,
            new_passphrase: new,
            new_key,
            store: keep,
            rotate,
        } => {
            use mnemo_engine::keys::{self, Secret};
            let old = secret_for(&path, &pp, &key_file)?;
            let new: Secret = if new_key {
                Secret::Key(zeroize::Zeroizing::new(Mnemo::generate_key()))
            } else {
                Secret::Passphrase(zeroize::Zeroizing::new(new_passphrase(
                    &new,
                    "MNEMO_NEW_PASSPHRASE",
                )?))
            };
            // 12.4 (RB-007.4): through the lock layer, so shared handles'
            // calls can't keep the rekey out, and theirs see "rekey".
            let mut db = open_rw(&path, &old, "rekey").map_err(fmt)?;
            // RB-005.4 item 4: REKEY-1 makes an error leave the old secret in
            // force, so say so. The one exception (the old header couldn't
            // be written back) says "may open with either secret" itself.
            let old_word = match old.kind() {
                keys::SecretKind::Passphrase => "passphrase",
                keys::SecretKind::Key => "key",
            };
            let not_changed = |msg: String| {
                if msg.contains("may open with either secret") {
                    msg
                } else {
                    format!("{msg}; the {old_word} was not changed")
                }
            };
            let mut stored_note = None;
            if new_key || keep {
                // 12.3 (the keys note, section 4): the new secret goes into
                // the key store before the commit.
                let Some(store) = keys::default_store() else {
                    return Err(
                        "--new-key and --store keep the new secret in a keychain, and none \
                                is available here; nothing was changed"
                            .into(),
                    );
                };
                let mut before_commit = || -> Result<()> {
                    if let Secret::Key(k) = &new {
                        show_recovery_code(&path, k);
                    }
                    Ok(())
                };
                match keys::rekey_stored(&mut db, &new, &*store, &path, &mut before_commit) {
                    Ok(note) => stored_note = Some(note),
                    Err(e) => {
                        return Err(format!("{}; {}", not_changed(fmt(e.error)), e.entries));
                    }
                }
            } else if let Secret::Passphrase(p) = &new {
                // 12.3 (the keys note, section 1, rule 1): never below
                // `secure()`, nor below what the file had.
                let kdf = db.kdf_params().for_passphrase();
                db.rekey(p, kdf).map_err(|e| not_changed(fmt(e)))?;
            }
            // REKEY-1: the rekey took effect; part of the old wrap's cleanup
            // didn't. `--rotate` rewrites the file, which covers it.
            let left = db.rekey_cleanup_error().map(str::to_owned);
            db.close().map_err(fmt)?;
            if let (Some(left), false) = (&left, rotate) {
                eprintln!(
                    "warning: the new secret is in force, but part of the old key \
                     wrap's cleanup failed ({left}); run `mnemo rekey --rotate` to be sure \
                     the old secret opens nothing"
                );
            }
            if let Some(note) = stored_note {
                println!("  → {note}");
            }
            if rotate {
                let r = match &new {
                    Secret::Passphrase(p) => Mnemo::compact_file_waiting(&path, p, wait_opts()),
                    Secret::Key(k) => Mnemo::compact_file_waiting_with_key(&path, k, wait_opts()),
                }
                .map_err(fmt)?;
                println!(
                    "rekeyed and rotated {path}: a fresh data key, {} memories kept",
                    r.after
                );
            } else {
                println!("rekeyed {path}");
                println!(
                    "note: the data key is unchanged; after a leak, run \
                     `mnemo rekey --rotate` to revoke the old secret"
                );
            }
        }
        Command::Compact {
            path,
            key_file,
            passphrase: pp,
        } => {
            let secret = secret_for(&path, &pp, &key_file)?;
            let report = match &secret {
                inspect::Secret::Passphrase(p) => {
                    Mnemo::compact_file_waiting(&path, p, wait_opts())
                }
                inspect::Secret::Key(k) => {
                    Mnemo::compact_file_waiting_with_key(&path, k, wait_opts())
                }
            }
            .map_err(fmt)?;
            println!(
                "compacted {path} (v9): {} -> {} live memories",
                report.before, report.after
            );
            if report.zero_vector_manifests + report.zero_vector_memories > 0 {
                println!(
                    "  all-zero vectors: {} legacy manifest memories given the sentinel \
                     vector, {} other memories carried unchanged",
                    report.zero_vector_manifests, report.zero_vector_memories
                );
            }
        }
        Command::Verify {
            path,
            key_file,
            passphrase: pp,
        } => {
            let secret = secret_for(&path, &pp, &key_file)?;
            let mut db = open_ro(&path, &secret).map_err(fmt)?;
            let report = db.verify_report().map_err(fmt)?;
            println!(
                "verified {} records — all pages decrypt and decode",
                report.records
            );
            if report.zero_vector_memories > 0 {
                eprintln!(
                    "warning: {} memories have an all-zero vector (written before v0.5's \
                     zero-vector rule); they score 0.0 on every vector query. `mnemo compact` \
                     carries them and gives a legacy manifest the sentinel vector.",
                    report.zero_vector_memories
                );
            }
        }
        Command::Import {
            path,
            file,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            // Read the input before taking the lock, then one exclusive hold
            // through the lock layer (12.4, RB-007.4) for the whole import.
            let text =
                std::fs::read_to_string(&file).map_err(|e| format!("cannot read {file}: {e}"))?;
            let mut db = open_w(&path, &pp, "import").map_err(fmt)?;
            let mut count = 0usize;
            for (lineno, line) in text.lines().enumerate() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let m = memory_from_json(line).map_err(|e| format!("line {}: {e}", lineno + 1))?;
                db.remember(m).map_err(fmt)?;
                count += 1;
            }
            db.flush().map_err(fmt)?;
            println!("imported {count} memories into {path}");
        }
        Command::Remember {
            path,
            text,
            r#type,
            importance,
            agent,
            metadata,
            key,
            embed_cmd,
            embed_name,
            embed_normalize,
            embed_timeout,
            passphrase: pp,
        } => {
            // `Memory` is already imported at the file top; keep only
            // the Phase-3 bits we add here to avoid Rust 1.75's
            // redundant-import warning (which `-D warnings` makes fatal).
            use mnemo_engine::{EmbedCmd, EmbedderIdentity};
            let pp = secret_for(&path, &pp, &None)?;
            let mt = parse_memory_types(&r#type)?
                .pop()
                .ok_or_else(|| format!("unknown memory type: {type:?}", r#type = r#type))?;
            // 12.4: embed first, then take the write lock (the embedder can
            // take seconds; other sessions shouldn't wait behind it). The
            // identity check and the width check still run under the lock,
            // before anything is written.
            let (mut db, vector) = match embed_first(
                &path,
                &embed_cmd,
                &embed_name,
                embed_normalize,
                embed_timeout,
                &[text.as_str()],
            )? {
                Some((embedder, mut vectors)) => {
                    let vector = vectors
                        .pop()
                        .ok_or_else(|| "embedder returned no vector".to_string())?;
                    let mut db = open_w(&path, &pp, "remember").map_err(fmt)?;
                    db.set_embedder(embedder).map_err(fmt)?;
                    (db, vector)
                }
                // The header couldn't be read: the old order.
                None => {
                    let mut db = open_w(&path, &pp, "remember").map_err(fmt)?;
                    let identity = EmbedderIdentity {
                        name: embed_name,
                        dimensions: db.dimensions(),
                        normalize: embed_normalize,
                    };
                    let embedder = EmbedCmd::new(&embed_cmd, identity)
                        .map_err(fmt)?
                        .with_timeout(std::time::Duration::from_secs(embed_timeout));
                    db.set_embedder(embedder).map_err(fmt)?;
                    let vector = {
                        let embedder_ref = db
                            .embedder()
                            .ok_or_else(|| "no embedder registered".to_string())?;
                        let mut vectors = embedder_ref.embed(&[text.as_str()]).map_err(fmt)?;
                        vectors
                            .pop()
                            .ok_or_else(|| "embedder returned no vector".to_string())?
                    };
                    (db, vector)
                }
            };
            let mut memory = Memory::new(text, mt, vector);
            if let Some(i) = importance {
                memory = memory.with_importance(i);
            }
            if let Some(a) = agent {
                memory = memory.with_agent(a);
            }
            if let Some(meta_str) = metadata {
                let parsed: serde_json::Value = serde_json::from_str(&meta_str)
                    .map_err(|e| format!("invalid --metadata JSON: {e}"))?;
                let obj = parsed
                    .as_object()
                    .ok_or_else(|| "--metadata must be a JSON object".to_string())?;
                for (k, v) in obj {
                    memory.metadata.insert(k.clone(), v.clone());
                }
            }
            // (Scope defaults to Private; add `--shared` later if the
            // CLI needs to opt into Scope::Shared.)
            //
            // Phase 12.1: when `--key` is given, take the keyed-upsert
            // path (supersede / no-op / add), and print `id\toutcome`
            // so scripts can distinguish the three states. Without
            // `--key`, keep the original single-ULID stdout for
            // backward compatibility.
            match key {
                None => {
                    let id = db.remember(memory).map_err(fmt)?;
                    db.flush().map_err(fmt)?;
                    println!("{id}");
                }
                Some(k) => {
                    let (id, outcome) = db.remember_keyed(memory, &k).map_err(fmt)?;
                    db.flush().map_err(fmt)?;
                    let outcome_str = match outcome {
                        mnemo_engine::WriteOutcome::Added => "added",
                        mnemo_engine::WriteOutcome::Superseded => "superseded",
                        mnemo_engine::WriteOutcome::Unchanged => "unchanged",
                        // `Updated` isn't reachable on a keyed remember —
                        // that's the `mnemo update` path. Treat it as
                        // `superseded` so the enum match stays total.
                        mnemo_engine::WriteOutcome::Updated => "superseded",
                    };
                    println!("{id}\t{outcome_str}");
                }
            }
        }
        Command::Index {
            path,
            passphrase: pp,
            drop,
            rebuild,
            partitions,
            subspaces,
            n_probe,
            n_rerank,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            let mut db = open_w(&path, &pp, "index").map_err(fmt)?;
            if drop {
                db.drop_index();
                db.flush().map_err(fmt)?;
                println!("dropped ANN index on {path}");
            } else if rebuild {
                // BP.1: re-cluster + retrain under the current
                // `IndexConfig::default()` (the ANN-1.1 auto-formulas).
                // Explicit `--partitions` / `--subspaces` / `--n-probe`
                // / `--n-rerank` are rejected here so this flag has one
                // clear behavior — "give me the shipped defaults for
                // the file's current size". Users who want custom
                // parameters should use `mnemo index <file>` (without
                // --rebuild) with the per-flag overrides.
                if partitions != 0 || subspaces != 0 || n_probe != 0 || n_rerank != 0 {
                    return Err(fmt(mnemo_engine::MnemoError::Invalid(
                        "--rebuild uses the auto-defaults; drop \
                         --partitions / --subspaces / --n-probe / \
                         --n-rerank, or build without --rebuild to pin \
                         explicit values"
                            .into(),
                    )));
                }
                let info = db.rebuild_index().map_err(fmt)?;
                db.flush().map_err(fmt)?;
                println!(
                    "rebuilt ANN index on {path}: {} vectors, {} partitions, {} PQ subspaces",
                    info.vectors, info.partitions, info.subspaces
                );
            } else {
                let cfg = mnemo_engine::IndexConfig {
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
        Command::Search {
            path,
            query,
            query_file,
            top_k,
            passphrase: pp,
        } => {
            // Search is a pure catalog scan — no access-tracking, no
            // catalog writes. Safe on a read-only handle.
            let pp = secret_for(&path, &pp, &None)?;
            let q = resolve_query(query, query_file)?;
            let mut db = open_r(&path, &pp).map_err(fmt)?;
            let hits = db
                .search(&q, top_k, mnemo_engine::Metric::Cosine)
                .map_err(fmt)?;
            if hits.is_empty() {
                println!("no results");
            }
            for (m, sim) in hits {
                println!("{sim:.4}  [{}]  {}", m.memory_type.as_str(), m.content);
            }
        }
        Command::About {
            path,
            format,
            manifest_only,
            key_file,
            passphrase: pp,
        } => {
            let secret = secret_for(&path, &pp, &key_file)?;
            let mut db = open_ro(&path, &secret).map_err(fmt)?;
            let stats = db.stats().map_err(fmt)?;
            let snapshots = db.snapshots().len();
            let mut entries = db.about().map_err(fmt)?;
            // 12.7d: the project essence follows the manifest and the
            // onboarding memories (skipped with --manifest-only).
            let essence = if manifest_only {
                None
            } else {
                db.about_essence().map_err(fmt)?
            };
            let print_essence = || {
                if let Some(render) = &essence {
                    println!();
                    println!("## Project essence");
                    println!();
                    print!("{render}");
                }
            };
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
                            "format_version": stats.format_version,
                            "upgrade_hint": upgrade_hint(&path, stats.format_version),
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
                        "essence": essence.as_deref(),
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
                        if stats.encrypted {
                            "encrypted"
                        } else {
                            "plaintext"
                        },
                        snapshots,
                        if stats.index.is_some() {
                            " · ANN index built"
                        } else {
                            ""
                        },
                    );
                    if let Some(hint) = upgrade_hint(&path, stats.format_version) {
                        println!("# {hint}");
                    }
                    if entries.is_empty() {
                        println!();
                        println!(
                            "(no onboarding memories — this database has no self-description.)"
                        );
                        println!("To make a database self-describing, store a memory with");
                        println!(
                            "  metadata = {{\"area\": \"onboarding\", \"topic\": \"manifest\"}}"
                        );
                        println!("that introduces the project, embedder, and conventions.");
                        print_essence();
                    } else {
                        println!();
                        println!(
                            "## Onboarding briefing ({} entries, most important first)",
                            entries.len()
                        );
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
                        print_essence();
                        println!();
                        println!("## Quick start");
                        println!(
                            "  mnemo list   {path}                 # browse all live memories"
                        );
                        println!("  mnemo recall {path} --query VEC     # multi-signal recall");
                        println!("  mnemo get    {path} <ulid> --verbose # fetch one memory");
                    }
                }
            }
        }
        Command::Get {
            path,
            id,
            format,
            verbose,
            vector,
            passphrase: pp,
        } => {
            // `get` is a plain catalog lookup + record read — no writes.
            let pp = secret_for(&path, &pp, &None)?;
            let ulid = parse_ulid(&id)?;
            let mut db = open_r(&path, &pp).map_err(fmt)?;
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
            key_file,
            passphrase: pp,
        } => {
            let secret = secret_for(&path, &pp, &key_file)?;
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
            // `list` is a bulk read of the catalog + record bodies — no writes.
            let mut db = open_ro(&path, &secret).map_err(fmt)?;
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
        Command::Export {
            path,
            format,
            r#type,
            agent,
            git,
            output,
            key_file,
            passphrase: pp,
        } => {
            let secret = secret_for(&path, &pp, &key_file)?;
            let types = match r#type {
                Some(s) => Some(parse_memory_types(&s)?),
                None => None,
            };
            if let Some(o) = &output {
                if let (Ok(a), Ok(b)) = (std::fs::canonicalize(o), std::fs::canonicalize(&path)) {
                    if a == b {
                        return Err(format!("refusing to write the export over {path} itself"));
                    }
                }
            }
            let mut db = open_ro(&path, &secret).map_err(fmt)?;
            let summary = db.stats_summary();
            let facts = export::FileFacts {
                format_version: summary.format_version,
                dimensions: summary.dimensions,
            };
            let all = db.memories().map_err(fmt)?;
            drop(db);
            let opts = export::ExportOptions {
                format,
                git,
                types,
                agent,
            };
            let selected = export::select(all, &opts);
            let body = export::render(&selected, &opts, &facts);
            eprintln!(
                "warning: the export is plaintext, not encrypted; keep it as safe as \
                 the file's passphrase"
            );
            match output {
                Some(p) => {
                    export::write_private(&p, &body)?;
                    eprintln!("exported {} memories to {p}", selected.len());
                }
                None => {
                    use std::io::Write;
                    // A closed pipe (`| head`) isn't an error worth a panic.
                    let _ = std::io::stdout().lock().write_all(body.as_bytes());
                }
            }
        }
        Command::Inspect {
            path,
            port,
            open,
            idle_timeout,
            key_file,
            passphrase: pp,
        } => {
            let idle = inspect::parse_idle(&idle_timeout)?;
            let secret = secret_for(&path, &pp, &key_file)?;
            let loaded = inspect::load(&path, &secret)?;
            let listener = inspect::bind(port).map_err(|e| format!("binding 127.0.0.1: {e}"))?;
            let port = listener
                .local_addr()
                .map_err(|e| format!("binding 127.0.0.1: {e}"))?
                .port();
            let token = inspect::new_token();
            // The link carries a one-time code; the page exchanges it for
            // the token on its first load.
            let code = inspect::new_token();
            let url = format!("http://127.0.0.1:{port}/?c={code}");
            println!("inspecting {path} (read-only; the file is read once and closed)");
            println!("{url}");
            println!("the link opens one tab (it works once); press Ctrl-C to stop");
            {
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            let watch = inspect::Watch::new();
            if open {
                if let Err(e) = inspect::open_browser(&url, watch.clone()) {
                    eprintln!("warning: couldn't open a browser ({e}); open the URL above");
                } else if cfg!(target_os = "linux") {
                    // RB-004.5 item 5: a snap or flatpak browser can't read
                    // the forwarding page under /tmp (tested: snap Firefox).
                    eprintln!(
                        "note: if the browser says the file can't be found (a snap or \
                         flatpak browser can't read /tmp), copy the link above instead"
                    );
                }
            }
            let state = std::sync::Arc::new(std::sync::Mutex::new(inspect::State::new(
                path, secret, loaded,
            )));
            inspect::serve(listener, token, code, state, watch, idle)
                .map_err(|e| format!("serving: {e}"))?;
        }
        Command::Recall {
            path,
            query,
            query_file,
            text,
            keywords,
            top_k,
            format,
            r#type,
            agent,
            metric,
            n_probe,
            n_rerank,
            no_track,
            session,
            filter,
            max_bytes,
            embed_cmd,
            embed_name,
            embed_normalize,
            embed_timeout,
            key_file,
            passphrase: pp,
        } => {
            use mnemo_engine::{EmbedCmd, EmbedderIdentity};
            let secret = secret_for(&path, &pp, &key_file)?;
            if query.is_none() && query_file.is_none() && text.is_none() && keywords.is_none() {
                return Err(
                    "recall requires one of --query / --query-file / --text / --keywords".into(),
                );
            }

            // Open the `.mnemo` file for recall:
            //
            // * `--no-track` → `open_read_only` (shared flock,
            //   `track_access=false` on the request). Compatible with
            //   other readers, blocked only by exclusive writers.
            //
            // * default (tracking) → try `open` (exclusive flock) so
            //   recall can bump `accessed_at` / `access_count` in the
            //   catalog. If that fails because another holder has the
            //   shared or exclusive lock, print one line to stderr
            //   and fall back to `open_read_only` + `track_access=false`.
            //
            // Returns `(db, effective_no_track)` so the caller can
            // decide whether to flush after `recall`.
            let open_recall =
                |path: &str,
                 secret: &inspect::Secret,
                 no_track: bool|
                 -> std::result::Result<(Mnemo, bool), mnemo_engine::MnemoError> {
                    if no_track {
                        return open_ro(path, secret).map(|db| (db, true));
                    }
                    match open_rw(path, secret, "recall") {
                        Ok(db) => Ok((db, false)),
                        Err(mnemo_engine::MnemoError::Locked { .. }) => {
                            // R-004.4: open read-only first and print the
                            // notice only if that worked. An exclusive
                            // holder blocks the read-only open too, and
                            // then the `Locked` error is the whole story.
                            // One try (12.4): the write open already
                            // waited `--wait`, and a holder that kept it
                            // out almost always keeps a read out too, so a
                            // second full wait would double the delay.
                            let db = match secret {
                                inspect::Secret::Passphrase(p) => Mnemo::open_read_only(path, p),
                                inspect::Secret::Key(k) => Mnemo::open_read_only_with_key(path, k),
                            }?;
                            eprintln!(
                                "mnemo: another process holds the lock on {path}; \
                                 running recall read-only with --no-track"
                            );
                            Ok((db, true))
                        }
                        // The v8 sunset: a v8 file recalls read-only.
                        Err(mnemo_engine::MnemoError::FormatTooOld { version: 8 }) => {
                            let db = open_ro(path, secret)?;
                            eprintln!(
                                "mnemo: {path} is a v8 file, which opens read-only; running \
                                 recall with --no-track (`mnemo compact` converts it to v9)"
                            );
                            Ok((db, true))
                        }
                        Err(e) => Err(e),
                    }
                };

            // Resolve the query vector. The `--text` branch opens the
            // database up front so it can register an embedder on the
            // handle (`set_embedder`) and run it; it uses the same
            // lock-fallback as the vector branch. The vector branches
            // defer the open until after we've built `req`.
            // FMT-9d: `--keywords` adds a BM25 term; `--text` without an
            // embedder is keyword-only on v9.
            let mut keyword: Option<String> = keywords;
            let (q, mut db_opt, mut effective_no_track) = match text {
                Some(text) if embed_cmd.is_none() => {
                    // No embedder: keyword-only recall, which needs v9.
                    let (db, effective_no_track) =
                        open_recall(&path, &secret, no_track).map_err(fmt)?;
                    if db.stats_summary().format_version < 9 {
                        return Err("--text requires --embed-cmd and --embed-name \
                                    (or a v9 file, which can recall by keyword)"
                            .to_string());
                    }
                    if keyword.is_none() {
                        keyword = Some(text);
                    }
                    (Vec::new(), Some(db), effective_no_track)
                }
                Some(text) => {
                    let ecmd = embed_cmd.ok_or_else(|| {
                        "--text requires --embed-cmd and --embed-name".to_string()
                    })?;
                    let ename = embed_name.ok_or_else(|| {
                        "--text requires --embed-cmd and --embed-name".to_string()
                    })?;
                    let (mut db, effective_no_track) =
                        open_recall(&path, &secret, no_track).map_err(fmt)?;
                    let identity = EmbedderIdentity {
                        name: ename,
                        dimensions: db.dimensions(),
                        normalize: embed_normalize,
                    };
                    let embedder = EmbedCmd::new(&ecmd, identity)
                        .map_err(fmt)?
                        .with_timeout(std::time::Duration::from_secs(embed_timeout));
                    db.set_embedder(embedder).map_err(fmt)?;
                    let vector = {
                        let embedder_ref = db
                            .embedder()
                            .ok_or_else(|| "embedder vanished after set_embedder".to_string())?;
                        let mut vs = embedder_ref.embed(&[text.as_str()]).map_err(fmt)?;
                        vs.pop()
                            .ok_or_else(|| "embedder returned no vector".to_string())?
                    };
                    (vector, Some(db), effective_no_track)
                }
                None if query.is_none() && query_file.is_none() => (Vec::new(), None, no_track),
                None => (resolve_query(query, query_file)?, None, no_track),
            };

            let metric = parse_metric(&metric)?;
            let mut req = RecallRequest::new(q).top_k(top_k).metric(metric);
            if let Some(k) = keyword {
                req = req.keywords(k);
            }
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
            // Phase 12.6: parse --session and the repeatable --filter
            // expressions into a single `Filter`. Empty filter == no
            // constraints.
            let mut f = mnemo_engine::Filter::default();
            if let Some(s) = session {
                f = f.with_session(s);
            }
            for expr in filter {
                f = parse_filter_expr(&expr, f)?;
            }
            req = req.filter(f);
            if let Some(b) = max_bytes {
                req = req.max_bytes(b);
            }

            // If the vector branch didn't pre-open, open now with the
            // same fallback semantics.
            let mut db = match db_opt.take() {
                Some(db) => db,
                None => {
                    let (db, eff_no_track) = open_recall(&path, &secret, no_track).map_err(fmt)?;
                    effective_no_track = eff_no_track;
                    db
                }
            };
            if effective_no_track {
                req = req.track_access(false);
            }
            let resp = db.recall_budgeted(&req).map_err(fmt)?;
            // R-001.4: a tracking recall mutates the catalog's
            // `accessed_at` / `access_count` in memory. Flush before
            // exit so the bumps reach disk — otherwise the usage
            // saturation signal is reset to zero on the next CLI
            // invocation. The read-only / fallback path doesn't
            // mutate, so skip the flush and avoid the needless
            // `ReadOnly` error from a read-only handle.
            if !effective_no_track {
                db.flush().map_err(fmt)?;
            }
            print_recall_hits(&resp.hits, format);
            // Phase 12.8: if the max_bytes budget cut anything, say
            // so on stderr so pipelines / scripts know the result is
            // bounded. The stdout format stays unchanged so existing
            // parsers keep working.
            if resp.truncated {
                eprintln!(
                    "(truncated: hits dropped by --max-bytes; rerun with a larger \
                     budget or smaller --top-k to see more)"
                );
            }
        }
        Command::Demo { path } => demo(&path).map_err(fmt)?,
        Command::Update {
            path,
            id,
            text,
            importance,
            metadata,
            ttl_secs,
            clear_ttl,
            embed_cmd,
            embed_name,
            embed_normalize,
            embed_timeout,
            passphrase: pp,
        } => {
            use mnemo_engine::{EmbedCmd, EmbedderIdentity, MemoryPatch, WriteOutcome};
            let pp = secret_for(&path, &pp, &None)?;
            let ulid = parse_ulid(&id)?;

            let mut patch = MemoryPatch::default();
            if let Some(i) = importance {
                patch.importance = Some(i);
            }
            if let Some(meta_str) = metadata {
                let parsed: serde_json::Value = serde_json::from_str(&meta_str)
                    .map_err(|e| format!("invalid --metadata JSON: {e}"))?;
                let obj = parsed
                    .as_object()
                    .ok_or_else(|| "--metadata must be a JSON object".to_string())?;
                patch.metadata = Some(obj.clone());
            }
            if clear_ttl {
                patch.ttl_secs = Some(None);
            } else if let Some(ttl) = ttl_secs {
                patch.ttl_secs = Some(Some(ttl));
            }

            let (mut db, outcome) = match text {
                Some(t) => {
                    let ecmd = embed_cmd.ok_or_else(|| {
                        "--text requires --embed-cmd and --embed-name".to_string()
                    })?;
                    let ename = embed_name.ok_or_else(|| {
                        "--text requires --embed-cmd and --embed-name".to_string()
                    })?;
                    // 12.4: embed first, then take the write lock.
                    match embed_first(
                        &path,
                        &ecmd,
                        &ename,
                        embed_normalize,
                        embed_timeout,
                        &[t.as_str()],
                    )? {
                        Some((embedder, mut vectors)) => {
                            let vector = vectors.pop().ok_or_else(|| {
                                "embedder returned no vector for the input text".to_string()
                            })?;
                            let mut db = open_w(&path, &pp, "update").map_err(fmt)?;
                            db.set_embedder(embedder).map_err(fmt)?;
                            // As `update_text` does, with the vector already made.
                            if vector.len() != db.dimensions() {
                                return Err(fmt(mnemo_engine::MnemoError::DimensionMismatch {
                                    expected: db.dimensions(),
                                    got: vector.len(),
                                }));
                            }
                            patch.content = Some(t);
                            patch.vector = Some(vector);
                            let outcome = db.update(&ulid, patch).map_err(fmt)?;
                            (db, outcome)
                        }
                        None => {
                            let mut db = open_w(&path, &pp, "update").map_err(fmt)?;
                            let identity = EmbedderIdentity {
                                name: ename,
                                dimensions: db.dimensions(),
                                normalize: embed_normalize,
                            };
                            let embedder = EmbedCmd::new(&ecmd, identity)
                                .map_err(fmt)?
                                .with_timeout(std::time::Duration::from_secs(embed_timeout));
                            db.set_embedder(embedder).map_err(fmt)?;
                            let outcome = db.update_text(&ulid, &t, patch).map_err(fmt)?;
                            (db, outcome)
                        }
                    }
                }
                None => {
                    let mut db = open_w(&path, &pp, "update").map_err(fmt)?;
                    let outcome = db.update(&ulid, patch).map_err(fmt)?;
                    (db, outcome)
                }
            };
            db.flush().map_err(fmt)?;
            let outcome_str = match outcome {
                WriteOutcome::Updated => "updated",
                WriteOutcome::Unchanged => "unchanged",
                // Not reachable on `update`, but keep the match total.
                WriteOutcome::Added => "updated",
                WriteOutcome::Superseded => "updated",
            };
            println!("{id}\t{outcome_str}");
        }
        Command::History {
            path,
            key,
            agent,
            shared,
            format,
            passphrase: pp,
        } => {
            use mnemo_engine::KeyScope;
            let pp = secret_for(&path, &pp, &None)?;
            let mut db = open_r(&path, &pp).map_err(fmt)?;
            // v0.8.1 bugfix: default slot matches keyed remember with
            // the same arguments. No `--shared`, no `--agent` → default
            // agent's private slot.
            let scope = if shared {
                KeyScope::Shared
            } else {
                match agent {
                    Some(a) => KeyScope::Private(a),
                    None => KeyScope::private_default(),
                }
            };
            let chain = db.history_in(&key, scope).map_err(fmt)?;
            match format {
                OutputFormat::Table => {
                    if chain.is_empty() {
                        println!("no history for key={key}");
                    } else {
                        for (i, m) in chain.iter().enumerate() {
                            let state = if i == 0 { "live" } else { "superseded" };
                            println!(
                                "{state:<10} {id}  created_at={ct}  {content}",
                                id = m.id,
                                ct = m.created_at,
                                content = truncate(&m.content, 80),
                            );
                        }
                    }
                }
                OutputFormat::Json => {
                    let arr: Vec<_> = chain
                        .iter()
                        .enumerate()
                        .map(|(i, m)| {
                            let mut v = memory_to_json(m, false);
                            if let Some(obj) = v.as_object_mut() {
                                obj.insert(
                                    "state".into(),
                                    serde_json::Value::String(
                                        (if i == 0 { "live" } else { "superseded" }).into(),
                                    ),
                                );
                            }
                            v
                        })
                        .collect();
                    println!(
                        "{}",
                        serde_json::to_string_pretty(
                            &serde_json::json!({ "count": arr.len(), "chain": arr }),
                        )
                        .unwrap_or_else(|_| "{}".into())
                    );
                }
                OutputFormat::Jsonl => {
                    for (i, m) in chain.iter().enumerate() {
                        let mut v = memory_to_json(m, false);
                        if let Some(obj) = v.as_object_mut() {
                            obj.insert(
                                "state".into(),
                                serde_json::Value::String(
                                    (if i == 0 { "live" } else { "superseded" }).into(),
                                ),
                            );
                        }
                        println!(
                            "{}",
                            serde_json::to_string(&v).unwrap_or_else(|_| "{}".into())
                        );
                    }
                }
            }
        }
        Command::Thread { cmd } => handle_thread_cmd(cmd)?,
        Command::Essence { cmd } => handle_essence_cmd(cmd)?,
        Command::Snapshots {
            path,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            let db = open_r(&path, &pp).map_err(fmt)?;
            let snaps = db.snapshots();
            if snaps.is_empty() {
                println!("no snapshots yet (nothing has been flushed)");
            } else {
                println!("{:<8}  {:<12}  committed (unix)", "txn", "memories");
                for s in snaps {
                    println!("{:<8}  {:<12}  {}", s.txn_id, s.memory_count, s.created_at);
                }
            }
        }
        Command::Restore {
            path,
            to_txn,
            to_time,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            let mut db = open_w(&path, &pp, "restore").map_err(fmt)?;
            let info = match (to_txn, to_time) {
                (Some(txn), _) => db.restore_to(txn).map_err(fmt)?,
                (None, Some(t)) => db.restore_to_time(t).map_err(fmt)?,
                (None, None) => return Err("restore needs --to-txn or --to-time".to_string()),
            };
            println!(
                "restored {path} to snapshot txn {} ({} memories)",
                info.txn_id, info.memory_count
            );
        }
        Command::Serve {
            path,
            mcp,
            embed_cmd,
            embed_name,
            embed_normalize,
            embed_timeout,
            shared,
            allow_network_fs,
        } => {
            if !mcp {
                return Err("serve requires --mcp today (other transports may come later)".into());
            }
            // The MCP server pulls `MNEMO_PASSPHRASE` itself — passing
            // `passphrase()` here would open a TTY prompt and break the
            // stdio JSON-RPC handshake. Stderr is safe for diagnostics.
            let p = std::path::Path::new(&path);
            let embed_spec = match (embed_cmd, embed_name) {
                (Some(c), Some(n)) => Some((
                    c,
                    n,
                    embed_normalize,
                    std::time::Duration::from_secs(embed_timeout),
                )),
                _ => None,
            };
            let shared = shared.then(|| {
                let mut o = wait_opts();
                o.allow_network_fs = allow_network_fs;
                o
            });
            mnemo_engine::mcp::serve_stdio_with_options(p, embed_spec, shared).map_err(fmt)?;
        }
        Command::Cache { op } => run_cache_op(op)?,
    }
    Ok(())
}

/// Dispatch a `mnemo cache …` subcommand. Kept out of `run()` to hold
/// the main match down to one line per subcommand.
fn run_cache_op(op: CacheOp) -> std::result::Result<(), String> {
    use mnemo_engine::CachePutOpts;

    match op {
        CacheOp::Get {
            path,
            ns,
            key,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            // Get on a cache is a mutation (bumps access stats) unless
            // the handle is read-only. Use read-write here so the stats
            // update lands durably on the next user-driven flush. A v8
            // file opens read-only (the v8 sunset), so its stats stay.
            let mut db = match open_w(&path, &pp, "cache get") {
                Err(mnemo_engine::MnemoError::FormatTooOld { version: 8 }) => {
                    open_r(&path, &pp).map_err(fmt)?
                }
                other => other.map_err(fmt)?,
            };
            match db.cache_get(&ns, &key).map_err(fmt)? {
                Some(v) => {
                    // Print value to stdout verbatim. If the caller
                    // asked for binary content-type, they can pipe;
                    // the CLI does not decode.
                    use std::io::Write;
                    std::io::stdout()
                        .write_all(&v.value)
                        .map_err(|e| e.to_string())?;
                    // A trailing newline for terminal ergonomics when
                    // the content is text; skip if the value already
                    // ends in one to avoid double-newlines in scripts.
                    if !v.value.ends_with(b"\n") {
                        println!();
                    }
                    // Persist the access-stats bump.
                    db.flush().map_err(fmt)?;
                }
                None => {
                    return Err(format!("cache miss on ({ns}, {key})"));
                }
            }
        }
        CacheOp::Put {
            path,
            ns,
            key,
            value,
            file,
            content_type,
            ttl,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            let bytes = match (value, file) {
                (Some(v), None) => v.into_bytes(),
                (None, Some(f)) => std::fs::read(&f).map_err(|e| format!("read {f}: {e}"))?,
                (Some(_), Some(_)) => {
                    return Err("--value and --file are mutually exclusive".into())
                }
                (None, None) => {
                    return Err("cache put needs --value <string> or --file <path>".into())
                }
            };
            let opts = CachePutOpts {
                content_type,
                ttl_secs: ttl,
                cost_hint_ms: None,
            };
            let mut db = open_w(&path, &pp, "cache put").map_err(fmt)?;
            db.cache_put(&ns, &key, &bytes, opts).map_err(fmt)?;
            db.flush().map_err(fmt)?;
            println!("cached {} bytes at ({ns}, {key})", bytes.len());
        }
        CacheOp::Delete {
            path,
            ns,
            key,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            let mut db = open_w(&path, &pp, "cache delete").map_err(fmt)?;
            db.cache_delete(&ns, &key).map_err(fmt)?;
            db.flush().map_err(fmt)?;
            println!("deleted ({ns}, {key}) (space reclaimed on next compact)");
        }
        CacheOp::Stats {
            path,
            ns,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            // Read-only handle is fine — stats is a pure scan.
            let db = open_r(&path, &pp).map_err(fmt)?;
            let s = db.cache_stats(ns.as_deref());
            let obj = serde_json::json!({
                "namespace": ns,
                "entries": s.entries,
                "bytes": s.bytes,
                "hits": s.hits,
                "misses": s.misses,
                "hit_rate": s.hit_rate,
                "evictions": s.evictions,
            });
            println!("{}", serde_json::to_string_pretty(&obj).unwrap());
        }
        CacheOp::Purge {
            path,
            ns,
            expired,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            let mut db = open_w(&path, &pp, "cache purge").map_err(fmt)?;
            let n = db.cache_purge(ns.as_deref(), expired).map_err(fmt)?;
            db.flush().map_err(fmt)?;
            println!("purged {n} entr{}", if n == 1 { "y" } else { "ies" });
        }
    }
    Ok(())
}

/// Build a [`Memory`] from one JSONL line.
fn memory_from_json(line: &str) -> std::result::Result<Memory, String> {
    use serde_json::Value;
    let v: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let content = v
        .get("content")
        .and_then(Value::as_str)
        .ok_or("missing string field 'content'")?;
    let vec_json = v
        .get("vector")
        .and_then(Value::as_array)
        .ok_or("missing array field 'vector'")?;
    let vector: Vec<f32> = vec_json
        .iter()
        .map(|x| {
            x.as_f64()
                .map(|f| f as f32)
                .ok_or("vector element not a number")
        })
        .collect::<std::result::Result<_, _>>()?;
    let mt = v
        .get("memory_type")
        .and_then(Value::as_str)
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

fn fmt(e: mnemo_engine::MnemoError) -> String {
    // 12.3: a wrong secret names where it came from, and the next source of
    // the same kind that's present (no fall-through).
    if matches!(e, mnemo_engine::MnemoError::WrongPassphrase) {
        if let Some((src, next)) = SOURCE.get() {
            return match next {
                Some(n) => format!("{e} (the secret came from {src}; {n} is also set, and is used once {src} isn't)"),
                None => format!("{e} (the secret came from {src})"),
            };
        }
    }
    e.to_string()
}

/// Phase 12.7a: handle `mnemo thread {ingest|export|forget}`. Split
/// into its own function because the three subcommands share nothing
/// with the top-level dispatch beyond passphrase resolution.
fn handle_thread_cmd(cmd: ThreadCmd) -> std::result::Result<(), String> {
    use mnemo_engine::{read_transcript, TranscriptFormat};
    use std::io::{Read, Write};
    match cmd {
        ThreadCmd::Ingest {
            path,
            thread,
            transcript,
            format,
            embed_cmd,
            embed_name,
            embed_normalize,
            embed_timeout,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            let fmt_enum = TranscriptFormat::parse(&format).ok_or_else(|| {
                format!("unknown --format '{format}' (want 'jsonl' or 'claude-code-jsonl')")
            })?;
            // Read the transcript (stdin with `-`, else the file).
            let turns = if transcript == "-" {
                let mut buf = Vec::new();
                std::io::stdin()
                    .read_to_end(&mut buf)
                    .map_err(|e| format!("reading stdin: {e}"))?;
                read_transcript(&buf[..], fmt_enum).map_err(fmt)?
            } else {
                let file = std::fs::File::open(&transcript)
                    .map_err(|e| format!("opening {transcript}: {e}"))?;
                read_transcript(file, fmt_enum).map_err(fmt)?
            };

            // 12.4: plan under a short read (which turns are new, and the
            // chunk texts), embed them with no lock held, then write the
            // vectors under the write lock. If another process ingested in
            // between, the plan is made again under the lock and only the
            // texts that weren't embedded yet are embedded there.
            let (texts, dimensions) = {
                let mut db = open_r(&path, &pp).map_err(fmt)?;
                (
                    db.ingest_thread_texts(&thread, &turns).map_err(fmt)?,
                    db.dimensions(),
                )
            };
            let embedder = new_embedder(
                &embed_cmd,
                &embed_name,
                embed_normalize,
                embed_timeout,
                dimensions,
            )?;
            let pre: Vec<Vec<f32>> = if texts.is_empty() {
                Vec::new()
            } else {
                use mnemo_engine::Embedder;
                let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
                embedder.embed(&refs).map_err(fmt)?
            };
            let mut db = open_w(&path, &pp, "thread ingest").map_err(fmt)?;
            db.set_embedder(embedder).map_err(fmt)?;
            let now = db.ingest_thread_texts(&thread, &turns).map_err(fmt)?;
            let report = if now == texts {
                db.ingest_thread_with_vectors(&thread, &turns, &pre)
                    .map_err(fmt)?
            } else {
                let mut have: std::collections::HashMap<&str, &Vec<f32>> =
                    texts.iter().map(String::as_str).zip(pre.iter()).collect();
                let missing: Vec<&str> = now
                    .iter()
                    .map(String::as_str)
                    .filter(|t| !have.contains_key(t))
                    .collect();
                let extra = if missing.is_empty() {
                    Vec::new()
                } else {
                    db.embedder()
                        .ok_or_else(|| "no embedder registered".to_string())?
                        .embed(&missing)
                        .map_err(fmt)?
                };
                for (t, v) in missing.iter().zip(extra.iter()) {
                    have.insert(t, v);
                }
                let vectors = now
                    .iter()
                    .map(|t| have.get(t.as_str()).map(|v| (*v).clone()))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| "embedder returned too few vectors".to_string())?;
                db.ingest_thread_with_vectors(&thread, &turns, &vectors)
                    .map_err(fmt)?
            };
            db.flush().map_err(fmt)?;
            println!(
                "thread {thread}: added {added} record(s) ({chunked} chunked, {truncated} truncated); \
                 skipped {skipped} already-stored turn(s)",
                added = report.added_records,
                chunked = report.chunked_turns,
                truncated = report.truncated_turns,
                skipped = report.skipped_existing_turns,
            );
            Ok(())
        }
        ThreadCmd::Export {
            path,
            thread,
            output,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            let mut db = open_r(&path, &pp).map_err(fmt)?;
            // 12.7d: the shared ingest-shape export (seq order, chunks
            // joined), so the JSONL matches Python and MCP `thread_export`
            // and re-ingests cleanly.
            let turns = db.thread_export_turns(&thread).map_err(fmt)?;
            let lines: Vec<String> = turns
                .iter()
                .map(|t| serde_json::to_string(&t.to_json()).unwrap_or_else(|_| "{}".into()))
                .collect();
            let body = lines.join("\n") + "\n";
            match output {
                Some(p) => {
                    let mut f =
                        std::fs::File::create(&p).map_err(|e| format!("opening {p}: {e}"))?;
                    f.write_all(body.as_bytes())
                        .map_err(|e| format!("writing {p}: {e}"))?;
                    println!(
                        "exported {} turn(s) from thread '{thread}' to {p}",
                        turns.len()
                    );
                }
                None => {
                    print!("{body}");
                }
            }
            Ok(())
        }
        ThreadCmd::Forget {
            path,
            thread,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            let mut db = open_w(&path, &pp, "thread forget").map_err(fmt)?;
            let n = db.thread_forget(&thread).map_err(fmt)?;
            db.flush().map_err(fmt)?;
            println!(
                "forgot {n} record(s) in thread '{thread}' (run `mnemo compact {path}` to reclaim space)"
            );
            Ok(())
        }
        ThreadCmd::Start {
            path,
            new,
            from,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            let mut db = open_w(&path, &pp, "thread start").map_err(fmt)?;
            let id = db.thread_start(&new, &from).map_err(fmt)?;
            db.flush().map_err(fmt)?;
            println!(
                "thread '{new}' continues from '{from}' (link record {id}; try `mnemo essence {path} --thread {new}`)"
            );
            Ok(())
        }
    }
}

/// Phase 12.7b: handle `mnemo essence {render|add|append|update|resolve|remove}`.
fn handle_essence_cmd(cmd: EssenceCmd) -> std::result::Result<(), String> {
    use mnemo_engine::{EssenceScope, EssenceSection};
    let parse_refs = |s: Option<String>| -> Option<Vec<String>> {
        s.map(|raw| {
            raw.split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
        })
    };
    let resolve_scope =
        |thread: Option<String>, project: bool| -> std::result::Result<EssenceScope, String> {
            match (thread, project) {
                (Some(t), false) => Ok(EssenceScope::Thread(t)),
                (None, true) => Ok(EssenceScope::Project),
                (None, false) => {
                    Err("pass either --thread <id> or --project (clap also enforces this).".into())
                }
                (Some(_), true) => Err("--thread and --project are mutually exclusive.".into()),
            }
        };
    match cmd {
        EssenceCmd::Render {
            path,
            thread,
            project,
            budget,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            let scope = resolve_scope(thread, project)?;
            let mut db = open_r(&path, &pp).map_err(fmt)?;
            let out = db.essence(scope, budget).map_err(fmt)?;
            print!("{out}");
            Ok(())
        }
        EssenceCmd::Add {
            path,
            section,
            text,
            thread,
            project,
            refs,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            let section = EssenceSection::parse(&section).ok_or_else(|| {
                format!(
                    "unknown section '{section}' (want intent, conventions, decisions, state, open, next, lessons)"
                )
            })?;
            if section == EssenceSection::Timeline {
                return Err("use `mnemo essence append` for timeline entries".into());
            }
            let scope = resolve_scope(thread, project)?;
            let mut db = open_w(&path, &pp, "essence add").map_err(fmt)?;
            let id = db
                .essence_add(scope, section, &text, parse_refs(refs))
                .map_err(fmt)?;
            db.flush().map_err(fmt)?;
            println!("{id}\tadded");
            Ok(())
        }
        EssenceCmd::Append {
            path,
            text,
            thread,
            project,
            refs,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            let scope = resolve_scope(thread, project)?;
            let mut db = open_w(&path, &pp, "essence append").map_err(fmt)?;
            let id = db
                .essence_append_timeline(scope, &text, parse_refs(refs))
                .map_err(fmt)?;
            db.flush().map_err(fmt)?;
            println!("{id}\tappended");
            Ok(())
        }
        EssenceCmd::Update {
            path,
            id,
            text,
            refs,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            let ulid = parse_ulid(&id)?;
            let mut db = open_w(&path, &pp, "essence update").map_err(fmt)?;
            let outcome = db
                .essence_update(&ulid, &text, parse_refs(refs))
                .map_err(fmt)?;
            db.flush().map_err(fmt)?;
            let s = match outcome {
                mnemo_engine::WriteOutcome::Updated => "updated",
                mnemo_engine::WriteOutcome::Unchanged => "unchanged",
                _ => "updated",
            };
            println!("{id}\t{s}");
            Ok(())
        }
        EssenceCmd::Resolve {
            path,
            id,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            let ulid = parse_ulid(&id)?;
            let mut db = open_w(&path, &pp, "essence resolve").map_err(fmt)?;
            let outcome = db.essence_resolve(&ulid).map_err(fmt)?;
            db.flush().map_err(fmt)?;
            let s = match outcome {
                mnemo_engine::WriteOutcome::Updated => "resolved",
                mnemo_engine::WriteOutcome::Unchanged => "already-resolved",
                _ => "resolved",
            };
            println!("{id}\t{s}");
            Ok(())
        }
        EssenceCmd::Remove {
            path,
            id,
            passphrase: pp,
        } => {
            let pp = secret_for(&path, &pp, &None)?;
            let ulid = parse_ulid(&id)?;
            let mut db = open_w(&path, &pp, "essence remove").map_err(fmt)?;
            db.essence_remove(&ulid).map_err(fmt)?;
            db.flush().map_err(fmt)?;
            println!("{id}\tremoved");
            Ok(())
        }
    }
}

/// Self-contained end-to-end demonstration.
fn demo(path: &str) -> Result<()> {
    let _ = std::fs::remove_file(path);
    let pp = "demo-passphrase";
    let cfg = MnemoConfig {
        dimensions: 4,
        ..Default::default()
    };

    println!("creating encrypted database at {path} ...");
    let mut db = Mnemo::create(path, pp, cfg)?;

    let seeds = [
        (
            "user prefers concise answers",
            MemoryType::Semantic,
            [0.9, 0.1, 0.0, 0.1],
            0.9,
        ),
        (
            "user is based in Berlin",
            MemoryType::Semantic,
            [0.8, 0.2, 0.1, 0.0],
            0.7,
        ),
        (
            "ran the deploy script at 14:00",
            MemoryType::Episodic,
            [0.1, 0.9, 0.1, 0.0],
            0.4,
        ),
        (
            "to reset state, call clear() then reload",
            MemoryType::Procedural,
            [0.0, 0.1, 0.9, 0.2],
            0.6,
        ),
        (
            "scratch: temp calculation result",
            MemoryType::Working,
            [0.2, 0.2, 0.2, 0.9],
            0.2,
        ),
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
            h.score,
            h.similarity,
            h.memory.memory_type.as_str(),
            h.memory.content
        );
    }

    println!("\nreopening to confirm persistence and encryption ...");
    // `close` consumes the handle and releases the file and its lock.
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
    // Windows' default main-thread stack is 1 MiB, and this CLI's
    // `run()` is a single ~600-line match whose debug-mode frame
    // reserves space for every variant's locals at once. Rust 1.95 +
    // the extra `embed_timeout` field on three subcommands pushed us
    // over the edge on `windows-latest` (stack overflow in every CLI
    // test, exit code 0xC00000FD). Running the dispatch on a worker
    // thread with 8 MiB of stack sidesteps the whole class — portable,
    // no linker flags, no cfg guards. The spawn + join overhead is
    // ~1 ms, invisible against disk I/O.
    const RUN_STACK: usize = 8 * 1024 * 1024;
    let handle = match std::thread::Builder::new()
        .name("mnemo-cli".into())
        .stack_size(RUN_STACK)
        .spawn(run)
    {
        Ok(h) => h,
        Err(e) => {
            eprintln!("error: could not start cli worker thread: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = match handle.join() {
        Ok(r) => r,
        Err(_) => {
            eprintln!("error: cli worker thread panicked");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
