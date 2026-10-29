//! # Mnemo
//!
//! Mnemo is an **encrypted, single-file, portable agent-memory engine**.
//!
//! A whole memory store — vectors, content, metadata, and the multi-signal
//! recall machinery an agent needs — lives in one file you can copy, back up,
//! or hand to another process. The file is encrypted at rest with a two-tier
//! key hierarchy: an Argon2id key-encryption key (KEK) derived from a
//! passphrase wraps a random data-encryption key (DEK), and the DEK encrypts
//! every page with AES-256-GCM.
//!
//! ## Quick start
//!
//! ```no_run
//! use mnemo::{Mnemo, MnemoConfig, Memory, MemoryType, RecallRequest};
//!
//! # fn main() -> mnemo::Result<()> {
//! let cfg = MnemoConfig { dimensions: 3, ..Default::default() };
//! let mut db = Mnemo::create("agent.mnemo", "correct horse battery", cfg)?;
//!
//! db.remember(
//!     Memory::new("the user prefers dark mode", MemoryType::Semantic, vec![0.1, 0.2, 0.9])
//!         .with_agent("assistant-1")
//!         .with_importance(0.8),
//! )?;
//! db.flush()?;
//!
//! let hits = db.recall(&RecallRequest::new(vec![0.1, 0.2, 0.9]).top_k(5))?;
//! for h in hits {
//!     println!("{:.3}  {}", h.score, h.memory.content);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## What is and is not built
//!
//! This crate implements a real, tested core: the encrypted single-file
//! storage engine, the crypto layer, the agent-memory model, multi-signal
//! recall, an IVF+PQ approximate-nearest-neighbour index, a write-ahead log,
//! snapshot-based point-in-time recovery, a bounded LRU page cache, and the
//! [`Session`] conversation wrapper. Exact brute-force search remains
//! available as the ground-truth baseline; a built index makes
//! [`Mnemo::recall`] sub-linear. Each [`Mnemo::flush`] is one atomic,
//! WAL-committed transaction and a restorable snapshot — [`Mnemo::restore_to`]
//! rewinds the database to any past transaction. Python and TypeScript
//! language bindings are the one documented roadmap item — see the README.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod cache;
mod crypto;
mod embed;
mod embed_cmd;
mod error;
mod format;
mod index;
pub mod mcp;
mod memory;
mod pager;
mod result_cache;
mod session;
mod store;
mod wal;

pub use crypto::KdfParams;
pub use embed::{Embedder, EmbedderIdentity};
pub use embed_cmd::{EmbedCmd, DEFAULT_EMBED_TIMEOUT};
pub use error::{MnemoError, Result};
pub use index::{IndexConfig, IndexInfo};
pub use memory::{Memory, MemoryType, Metric, Scope, ScoreWeights};
pub use result_cache::{
    CacheBudget, CacheFlushPolicy, CachePutOpts, CacheStats, CachedValue, SemanticCachePutOpts,
    DEFAULT_BATCH_MAX_AGE, DEFAULT_BATCH_MAX_DIRTY, DEFAULT_MAX_BYTES, DEFAULT_MAX_ENTRIES,
    DEFAULT_SEMANTIC_THRESHOLD,
};
pub use session::{Role, Session, Turn};
pub use store::{
    CompactReport, Mnemo, MnemoConfig, RecallRequest, RecallResult, SnapshotInfo, Stats,
};

/// Re-export of [`ulid::Ulid`], the identifier type used for memories.
pub use ulid::Ulid;

// --- Fsync counter -----------------------------------------------------
//
// Instrumentation plumbing for the Phase 6 top-3 cost #2 fix: the
// planner wants to know how many `sync_all` calls each `flush()` issues
// before deciding whether to batch any of them. Every fsync call site
// bumps a thread-local counter; `Mnemo::flush` snapshots it before and
// after, exposing the delta on `Stats::fsyncs_last_flush`. Thread-local
// so handles on separate threads don't poison each other's counts; the
// engine's one-writer-at-a-time locking means a single handle sees a
// coherent stream.

mod fsync_counter {
    use std::cell::Cell;

    thread_local! {
        static COUNT: Cell<u64> = const { Cell::new(0) };
    }

    /// Call once per `sync_all`.
    pub(crate) fn bump() {
        COUNT.with(|c| c.set(c.get().wrapping_add(1)));
    }

    /// Monotonic counter snapshot for this thread.
    pub(crate) fn count() -> u64 {
        COUNT.with(|c| c.get())
    }
}

pub(crate) use fsync_counter::{bump as bump_fsync, count as fsync_count};

/// Build-time cryptographic backend information.
///
/// Populated at compile time from the active `cfg` flags. On ARMv8
/// builds compiled with `--cfg aes_armv8 --cfg polyval_armv8` (set by
/// the repo's `.cargo/config.toml` and both CI workflows) the `aes`
/// and `polyval` crates emit hardware AES + PMULL instructions; without
/// those flags they fall back to a constant-time software path that
/// runs at roughly 5× the cost per page.
///
/// Published through [`build_info`], exposed by `mnemo info` on the
/// CLI and `mnemo.build_info()` from the Python binding. The point is
/// verification — nobody should have to profile a release build to find
/// out whether it's running software AES.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildInfo {
    /// Package version (`CARGO_PKG_VERSION`), e.g. `"0.5.1"`.
    pub version: &'static str,
    /// Target triple this binary was built for, e.g.
    /// `"aarch64-apple-darwin"`.
    pub target: &'static str,
    /// Human-readable AES backend. One of:
    /// - `"armv8 hardware"` — ARM with `cfg(aes_armv8)`
    /// - `"armv8 software"` — ARM without the cfg; constant-time fallback
    /// - `"x86_64 AES-NI (runtime detect)"` — x86_64 with runtime
    ///   AES-NI detection in the `aes` crate
    /// - `"portable software"` — any other target
    pub aes_backend: &'static str,
    /// Human-readable PMULL/CLMUL backend used by GHASH (part of AES-GCM).
    /// Same shape as `aes_backend` but for `polyval`.
    pub polyval_backend: &'static str,
}

/// Report this build's cryptographic backend configuration.
///
/// Cheap — all fields are `&'static str` populated at compile time.
/// Safe to call from a hot path if, for some reason, that's useful.
pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION"),
        target: TARGET_TRIPLE,
        aes_backend: aes_backend_name(),
        polyval_backend: polyval_backend_name(),
    }
}

/// Target triple baked in via the `TARGET` env var that `cargo` sets
/// for the host compile. Falls back to `"unknown"` if the var isn't
/// set at build time (shouldn't happen under cargo, but a build.rs-free
/// implementation keeps this crate build-script-free, so we use a
/// cfg-based dispatch as the second-best).
const TARGET_TRIPLE: &str = {
    // `env!("TARGET")` isn't set by cargo for crates; it's set by rustc
    // only for `std` builds. We approximate from `target_arch` /
    // `target_os` cfgs so this stays build-script-free. Good enough
    // for an info line; precise triples are a `cargo` or build.rs job.
    if cfg!(all(target_arch = "aarch64", target_os = "macos")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_arch = "aarch64", target_os = "linux")) {
        "aarch64-unknown-linux-gnu"
    } else if cfg!(all(target_arch = "x86_64", target_os = "macos")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_arch = "x86_64", target_os = "linux")) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_arch = "x86_64", target_os = "windows")) {
        "x86_64-pc-windows-msvc"
    } else {
        "unknown"
    }
};

const fn aes_backend_name() -> &'static str {
    if cfg!(aes_armv8) {
        "armv8 hardware"
    } else if cfg!(target_arch = "aarch64") {
        // Software fallback is the dangerous case this whole mechanism
        // exists to surface — flag it clearly so `mnemo info` makes it
        // easy to notice a build picked up the slow path by mistake.
        "armv8 software (constant-time fallback)"
    } else if cfg!(target_arch = "x86_64") {
        // `aes` 0.8.4 on x86_64 picks AES-NI vs the software path at
        // runtime via the `cpufeatures` crate, so we can't tell from
        // cfg alone which path a given invocation takes. In practice
        // every x86_64 CPU shipped since ~2010 has AES-NI.
        "x86_64 AES-NI (runtime detect)"
    } else {
        "portable software"
    }
}

const fn polyval_backend_name() -> &'static str {
    if cfg!(polyval_armv8) {
        "armv8 PMULL hardware"
    } else if cfg!(target_arch = "aarch64") {
        "armv8 software (constant-time fallback)"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64 CLMUL (runtime detect)"
    } else {
        "portable software"
    }
}

/// Test-only crash-injection surface used by the Phase 1.3 proptest
/// (`mnemo/tests/proptest_store.rs`). Compiled in under `cfg(test)`
/// and the `failpoints` feature only; absent from release builds.
///
/// **Not part of the stable public API.** The double-underscore
/// convention makes the "internal" contract obvious at the call site.
#[cfg(any(test, feature = "failpoints"))]
#[doc(hidden)]
pub mod __failpoints {
    /// Arm the pager and WAL commit path to fail on the Nth
    /// subsequent write. `n = 0` fails the very next write; `n = -1`
    /// (or any negative) disables. One-shot — disarms on trigger.
    ///
    /// Every write is checked before the byte hits the file, so a
    /// triggered failure is guaranteed torn-BEFORE, never
    /// torn-AFTER — critical for the "either pre-flush or post-flush"
    /// invariant the property test asserts.
    pub fn set_writes_until_fail(n: i64) {
        crate::pager::failpoints::set_writes_until_fail(n)
    }
}

/// Internal parse surfaces exposed for cargo-fuzz targets (Phase 1.2).
///
/// **Not part of the stable public API.** Each function feeds an
/// arbitrary byte slice into a parse/decode path that a hostile
/// `.mnemo` file (or a hostile MCP client) can send us, and returns
/// `Result<()>` so a fuzz target only asserts "no panic." Marked
/// `#[doc(hidden)]` so `cargo doc` doesn't advertise it; do not import
/// from application code.
#[doc(hidden)]
pub mod __fuzz {
    /// Parse a `.mnemo` header page (page 0, plaintext) from arbitrary
    /// bytes. This is the pre-passphrase attack surface — a hostile
    /// file hits it before any DEK exists.
    pub fn parse_header(bytes: &[u8]) -> crate::Result<()> {
        crate::format::Header::from_page(bytes).map(|_| ())
    }

    /// Replay the WAL scan over an arbitrary byte region. Delegates
    /// to [`crate::wal::recover_bytes`]; a corrupt / truncated /
    /// adversarial region must return `Err` or `Ok(None/Some(_))`,
    /// never panic.
    pub fn wal_recover_bytes(region: &[u8], wal_seq: u64) -> crate::Result<()> {
        crate::wal::recover_bytes(region, wal_seq).map(|_| ())
    }

    /// Decode a `Memory` record body from arbitrary bytes.
    pub fn decode_memory(bytes: &[u8]) -> crate::Result<()> {
        rmp_serde::from_slice::<crate::Memory>(bytes)
            .map(|_| ())
            .map_err(|e| crate::MnemoError::Serialize(e.to_string()))
    }

    /// Decode a `CacheEntry` record body from arbitrary bytes. Type is
    /// `pub(crate)`, so this wrapper hides it behind a unit return.
    pub fn decode_cache_entry(bytes: &[u8]) -> crate::Result<()> {
        rmp_serde::from_slice::<crate::result_cache::CacheEntry>(bytes)
            .map(|_| ())
            .map_err(|e| crate::MnemoError::Serialize(e.to_string()))
    }

    /// Decode a cache-directory page-run body (a `Vec<CacheDirectoryEntry>`)
    /// from arbitrary bytes.
    pub fn decode_cache_directory(bytes: &[u8]) -> crate::Result<()> {
        rmp_serde::from_slice::<Vec<crate::result_cache::CacheDirectoryEntry>>(bytes)
            .map(|_| ())
            .map_err(|e| crate::MnemoError::Serialize(e.to_string()))
    }

    /// Parse a single MCP stdio JSON-RPC request line. Malformed
    /// input returns `Err(String)`; the fuzz target only asserts
    /// no panic and that the server would not die on this line.
    pub fn parse_mcp_request_line(line: &str) -> Result<(), String> {
        crate::mcp::__fuzz_parse_request_line(line)
    }
}
