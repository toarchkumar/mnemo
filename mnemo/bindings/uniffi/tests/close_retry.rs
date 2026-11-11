//! RB-002.4's T5 (v0.12.2): `MnemoDb.close()` with a failing final flush.
//! Today (f79715e): `close` errors, then `flush` is `Closed`, and a reopen
//! finds 10 of 40. After: the handle stays, `flush` retries, and all 40
//! are saved. In its own test binary because the failpoint switch is
//! process-wide.

use mnemo_uniffi::{generate_key, MnemoConfig, MnemoDb, MnemoError, RememberOptions};

fn opts() -> RememberOptions {
    RememberOptions {
        memory_type: None,
        importance: None,
        metadata_json: None,
        key: None,
        ttl_secs: None,
        agent_id: None,
    }
}

#[test]
fn t5_a_failed_close_keeps_the_handle_and_flush_retries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t5.mnemo").display().to_string();
    let key = generate_key();
    let config = MnemoConfig {
        dimensions: 4,
        format_version: 0,
        vector_cache_budget_bytes: 16 << 20,
        cache_pages: 256,
    };
    let db = MnemoDb::create_with_key(path.clone(), key.clone(), config).unwrap();
    let write = |i: u32| {
        db.remember(
            format!("note {i}"),
            vec![i as f32 + 1.0, 0.5, 0.25, 1.0],
            opts(),
        )
        .unwrap();
    };
    (0..10).for_each(write);
    db.flush().unwrap();
    (10..40).for_each(write);

    mnemo_engine::__failpoints::set_writes_until_fail(0);
    let closed = db.close();
    mnemo_engine::__failpoints::set_writes_until_fail(-1);
    assert!(matches!(closed, Err(MnemoError::Io { .. })), "{closed:?}");

    // The handle is still there: flush retries and saves everything.
    assert_eq!(db.len().unwrap(), 40);
    db.flush().expect("the retry flushes");
    db.close().unwrap();
    assert!(matches!(db.len(), Err(MnemoError::Closed)));

    let again = MnemoDb::open_read_only_with_key(path, key).unwrap();
    assert_eq!(again.len().unwrap(), 40);
}
