#![cfg(any(test, feature = "failpoints"))]
//! DUR-1 — torn-header recovery after WAL-logged header commits.
//!
//! Every post-`create` page-0 write (WAL relocation, rekey) now goes
//! through the WAL first: commit a one-frame transaction `(0, header)`
//! into the current WAL region, checkpoint page 0, and only then
//! invalidate the predecessor region. On reopen, a page-0 that fails to
//! **parse** (`BadMagic`, `UnsupportedVersion`, `HeaderChecksum`)
//! triggers a file-wide streaming scan for valid committed header
//! frames; the highest-`txn_id` candidate that unwraps the DEK and
//! validates the seal under the passphrase replaces page 0.
//!
//! Per the Planner's 2026-11-01 Option B decision, a `validate_seal`
//! failure is **always** reported as `HeaderTampered` and never
//! auto-healed from the WAL (healing would erase tamper-evidence; a
//! real crash cannot produce "CRC valid + seal invalid" because the
//! full 294-byte header lies inside one 512-byte sector).
//!
//! The tests here cover the sector-level crash + tamper shapes the
//! spec calls out (R-002.1 part 4):
//!
//! * sector 0 zeroed
//! * sector 0 random garbage
//! * bytes 0..6 corrupted (MAGIC / version)
//! * bytes 242..294 stale with CRC intact → `HeaderTampered`
//! * sealed field altered + CRC recomputed → `HeaderTampered`
//! * torn page 0 **during** a WAL relocation
//! * torn page 0 **after** a WAL relocation (zeroed sector, >64-page
//!   relocation)
//! * torn page 0 during `rekey`, including on a never-flushed file
//!
//! The failpoint-driven tests arm the pager's `WRITES_UNTIL_FAIL`
//! counter so a specific write in the relocation / rekey sequence
//! raises an `io::Error`. They assert that reopen *always* recovers to a
//! consistent state — either the pre-op state or the post-op state,
//! never a third state and never a "DEK is gone" state.
//!
//! Requires the `failpoints` feature. Guarded test-side by
//! `#[cfg(any(test, feature = "failpoints"))]` to match the engine.
//!
//! FMT-9f: every test runs on both on-disk formats, as `v8::<name>` and
//! `v9::<name>`; the body lives in `tests/suites/dur1_torn_header.rs`.

// The v8 sunset (v0.13): a v8 file can't be created or opened read-write,
// so the suite runs on v9 only. The v8 coverage is the read tests on the
// committed fixtures, in `tests/v8_fixture.rs`.
mod v9 {
    const FORMAT: u16 = 9;
    include!("suites/dur1_torn_header.rs");
}
