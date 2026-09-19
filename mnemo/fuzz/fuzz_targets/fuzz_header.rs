//! Fuzz target: `.mnemo` header (page 0) parse.
//!
//! Page 0 is the **pre-passphrase attack surface** — a hostile `.mnemo`
//! file's bytes hit this parse before any DEK exists, so a panic here
//! is a DoS a malicious file can trigger without knowing the passphrase.
//! Phase 1.2 requires that `Header::from_page` return `Err` (or `Ok`)
//! on every arbitrary byte slice, never panic.
//!
//! Covers the v8 layout: `Header::from_page` now reads the three
//! cache-directory u64 fields at bytes 270–293 when `version >= 8`,
//! plus the v7 seal nonce/tag at 242–269. Both windows are within
//! `PAGE_SIZE = 8192` so no OOB risk, but fuzz confirms.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // `Header::from_page` returns `Err(BadMagic)` for buffers shorter
    // than PAGE_SIZE, so short inputs are also valid test cases — no
    // pre-length filter here.
    let _ = mnemo::__fuzz::parse_header(data);
});
