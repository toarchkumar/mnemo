//! FIX-1 — correctness sweep per REV-001 + REV-002.
//!
//! Covers:
//! * **R-001.1 + R-002.2 part 2** — `compact_file` preserves the result
//!   cache: live, non-expired entries survive with their directory-side
//!   `accessed_at` + `access_count` intact; tombstoned and TTL-expired
//!   entries drop. `verify()` walks the cache too.
//! * **R-001.5 + R-002.2 part 3** — `Mnemo::export_encrypted` refuses
//!   to overwrite the open database (direct path, symlink, hard link)
//!   and round-trips a normal export via temp-file + rename.
//! * **R-001.5** — `Mnemo::kdf_params()` returns the file's current
//!   Argon2id params; preserved across a rekey that doesn't supply new
//!   ones.
//! * **R-002.2 part 1** — read-only `cache_get` / `cache_get_semantic`
//!   return the directory's `accessed_at`, not the record body's
//!   insert-time snapshot.
//! * **R-001.8 part 3** — a cache-only flush does **not** push a
//!   restore-point snapshot; a memory flush does.
//!
//! FMT-9f: every test runs on both on-disk formats, as `v8::<name>` and
//! `v9::<name>`; the body lives in `tests/suites/fix1_correctness_sweep.rs`.

// The v8 sunset (v0.13): a v8 file can't be created or opened read-write,
// so the suite runs on v9 only. The v8 coverage is the read tests on the
// committed fixtures, in `tests/v8_fixture.rs`.
mod v9 {
    const FORMAT: u16 = 9;
    include!("suites/fix1_correctness_sweep.rs");
}
