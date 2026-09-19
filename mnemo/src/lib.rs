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
