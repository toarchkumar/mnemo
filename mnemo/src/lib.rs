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
mod memory;
mod pager;
mod session;
mod store;
mod wal;

pub use crypto::KdfParams;
pub use error::{MnemoError, Result};
pub use index::{IndexConfig, IndexInfo};
pub use memory::{Memory, MemoryType, Metric, Scope, ScoreWeights};
pub use session::{Role, Session, Turn};
pub use store::{
    CompactReport, Mnemo, MnemoConfig, RecallRequest, RecallResult, SnapshotInfo, Stats,
};

/// Re-export of [`ulid::Ulid`], the identifier type used for memories.
pub use ulid::Ulid;
