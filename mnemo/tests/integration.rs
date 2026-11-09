//! Integration tests for Mnemo.
//!
//! These exercise the public API end-to-end. `KdfParams::fast()` keeps Argon2
//! cheap so the suite runs quickly; production code should use the default
//! (secure) parameters.
//!
//! FMT-9f: every test runs on both on-disk formats, as `v8::<name>` and
//! `v9::<name>`; the body lives in `tests/suites/integration.rs`.

mod v8 {
    const FORMAT: u16 = 8;
    include!("suites/integration.rs");
}

mod v9 {
    const FORMAT: u16 = 9;
    include!("suites/integration.rs");
}
