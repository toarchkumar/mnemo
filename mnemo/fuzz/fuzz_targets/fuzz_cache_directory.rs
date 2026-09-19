//! Fuzz target: cache-directory page-run decode (Phase 10.1).
//!
//! The cache directory is a `Vec<CacheDirectoryEntry>` serialized as
//! MessagePack, stored in its own encrypted page run, pointed at by
//! `Header::cache_start/pages/len` in v8. `Mnemo::open` calls
//! `rmp_serde::from_slice` on the concatenated payload — a corrupt
//! directory must return `Err(MnemoError::Serialize)`, never panic.
//!
//! `CacheDirectoryEntry` gained two `#[serde(default)]`-tolerant
//! fields in PR 4 (Phase 10.2 semantic cache: `vector`, `model`) —
//! this target exercises the tolerant-decode path against arbitrary
//! bytes as well.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = mnemo::__fuzz::decode_cache_directory(data);
});
