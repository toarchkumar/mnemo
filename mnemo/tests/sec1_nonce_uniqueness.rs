//! PR SEC-1 (v0.5.2): page-nonce uniqueness invariants.
//!
//! Before SEC-1, page nonces were `page_no || write_counter` — unique
//! within a file but **reused across copies of a file**. If you copied
//! a `.mnemo` file to another host and both the original and the copy
//! got written to (two laptops syncing a file; a restored older backup
//! resuming operation), each copy's writer produced pages whose nonces
//! collided with the other's at the same `(page_no, write_counter)`
//! coordinates, under the same DEK (`random_dek()` is only called at
//! create). That breaks AES-GCM confidentiality (XOR of plaintexts
//! recoverable) and tamper-evidence (GHASH key recoverable → page
//! forgery possible).
//!
//! The fix is a per-session random prefix: every writable open draws
//! a fresh 64-bit `session_random`, and the nonce becomes
//! `session_random || session_counter`. Cross-session collisions now
//! require two sessions to independently draw the same 64-bit random
//! value (negligible). This file pins the three invariants the planner
//! asked for:
//!
//! 1. **No nonce in both copies.** Fork a file, write to both forks,
//!    scan every page in both — the raw-nonce sets must be disjoint.
//! 2. **Nonces differ across reopen sessions.** Open, write, close,
//!    reopen, write — the two sessions' nonces must not overlap.
//! 3. **v0.5.1 fixtures still open.** A file produced by the old
//!    deterministic-nonce code must still decrypt, since decrypt reads
//!    the stored nonce rather than recomputing one. (Simulated here by
//!    using the current code to produce a file and reopening it — the
//!    on-disk schema didn't change in SEC-1, so the equivalence is
//!    structural, not just empirical.)
//!
//! FMT-9f: every test runs on both on-disk formats, as `v8::<name>` and
//! `v9::<name>`; the body lives in `tests/suites/sec1_nonce_uniqueness.rs`.

mod v8 {
    const FORMAT: u16 = 8;
    include!("suites/sec1_nonce_uniqueness.rs");
}

mod v9 {
    const FORMAT: u16 = 9;
    include!("suites/sec1_nonce_uniqueness.rs");
}
