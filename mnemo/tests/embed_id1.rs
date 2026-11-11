//! EMBED-ID-1: every file records the embedder identity on first use and
//! refuses a different one after that (manifest or not, read-only handles
//! check without writing), and the record follows the file through
//! compaction, the rekeyed exports and the import.
//!
//! Every test runs on both on-disk formats, as `v8::<name>` and
//! `v9::<name>`; the body lives in `tests/suites/embed_id1.rs`.

mod v8 {
    const FORMAT: u16 = 8;
    include!("suites/embed_id1.rs");
}

mod v9 {
    const FORMAT: u16 = 9;
    include!("suites/embed_id1.rs");
}
