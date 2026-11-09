//! UniFFI bindings for the MNemo engine (MOBILE-1a): the Rust side of the
//! Swift and Kotlin packages.
//!
//! A curated API, not the whole engine: create and open (passphrase or raw
//! key, read-only), remember and recall (with keywords, filters and byte
//! budgets), updates and key history, the read surfaces, thread archives,
//! the exact-key result cache, compaction, and passphrase export and
//! import for moving a key-bound file between devices. Everything is on
//! one object, [`MnemoDb`], plus a few free functions.
//!
//! Defaults are set for phones: a 16 MiB vector column budget and a 2 MiB
//! page cache (the engine's own defaults are for desktops).
//!
//! Errors are one enum, [`MnemoError`]. No panic crosses the FFI: UniFFI
//! turns a panic into an internal error, and a handle whose lock was
//! poisoned by one reports `Corrupt` on every later call.

uniffi::setup_scaffolding!();

mod db;
mod embedder;
mod error;
mod types;

pub use db::{compact_file, compact_file_with_key, generate_key, import_with_passphrase, MnemoDb};
pub use embedder::MnemoEmbedder;
pub use error::{EmbedderError, MnemoError};
pub use types::*;

#[cfg(test)]
mod tests;
