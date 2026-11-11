//! EMBED-ID-1: every file records the embedder identity on first use and
//! refuses a different one after that (manifest or not, read-only handles
//! check without writing), and the record follows the file through
//! compaction, the rekeyed exports and the import.
//!
//! Every test runs on both on-disk formats, as `v8::<name>` and
//! `v9::<name>`; the body lives in `tests/suites/embed_id1.rs`.

// The v8 sunset (v0.13): a v8 file can't be created or opened read-write,
// so the suite runs on v9 only. The v8 coverage is the read tests on the
// committed fixtures, in `tests/v8_fixture.rs`.
mod v9 {
    const FORMAT: u16 = 9;
    include!("suites/embed_id1.rs");
}
