//! Fuzz target: MessagePack record decode.
//!
//! Record-page bodies are `rmp_serde::from_slice`'d into either a
//! `Memory` (memory catalog) or a `CacheEntry` (Phase 10.1 result
//! cache). The AAD-authenticated crypto layer means a page whose
//! ciphertext successfully decrypts is presumed non-adversarial, but
//! (a) a matching-passphrase adversary who can write pages still
//! reaches this decode, and (b) any panic in rmp_serde or serde-derive
//! machinery is a defense-in-depth issue we want to catch pre-release.
//!
//! Both types decode from the same byte slice per iteration — the
//! shapes are similar (both are structs of primitives + a bytes
//! payload) so a single corpus feeds both paths. `Memory::content` is
//! a String and `CacheEntry::value` is `Vec<u8>`, so wildly different
//! byte patterns exercise both.
//!
//! FMT-9e: a v9 thread-turn record may be a zstd frame, decompressed
//! (under a content-size bound) before the MessagePack decode. The input
//! is also fed with the zstd magic prepended, so the frame parser sees
//! fuzzed headers on every iteration.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = mnemo_engine::__fuzz::decode_memory(data);
    let _ = mnemo_engine::__fuzz::decode_cache_entry(data);
    let _ = mnemo_engine::__fuzz::decode_v9_memory(data);
    let mut framed = Vec::with_capacity(data.len() + 4);
    framed.extend_from_slice(&[0x28, 0xB5, 0x2F, 0xFD]);
    framed.extend_from_slice(data);
    let _ = mnemo_engine::__fuzz::decode_v9_memory(&framed);
});
