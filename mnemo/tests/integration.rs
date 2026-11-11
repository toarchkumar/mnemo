//! Integration tests for Mnemo.
//!
//! These exercise the public API end-to-end. `KdfParams::fast()` keeps Argon2
//! cheap so the suite runs quickly; production code should use the default
//! (secure) parameters.
//!
//! FMT-9f: every test runs on both on-disk formats, as `v8::<name>` and
//! `v9::<name>`; the body lives in `tests/suites/integration.rs`.

// The v8 sunset (v0.13): a v8 file can't be created or opened read-write,
// so the suite runs on v9 only. The v8 coverage is the read tests on the
// committed fixtures, in `tests/v8_fixture.rs`.
mod v9 {
    const FORMAT: u16 = 9;
    include!("suites/integration.rs");
}
