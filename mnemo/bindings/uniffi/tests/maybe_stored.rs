//! RB-016.3 item 1 (v0.20.0): after a commit that failed past its commit
//! point, the engine refuses every later flush on that handle with
//! `MaybeStored`. The binding reports it as its own variant and drops the
//! handle without flushing, so the file's lock is free and a fresh open
//! finishes the commit. In its own test binary because the failpoint
//! switch is process-wide.

use mnemo_uniffi::{generate_key, MnemoConfig, MnemoDb, MnemoError, RememberOptions};
use std::sync::Arc;

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

fn config() -> MnemoConfig {
    MnemoConfig {
        dimensions: 4,
        format_version: 0,
        vector_cache_budget_bytes: 16 << 20,
        cache_pages: 256,
    }
}

fn note(db: &MnemoDb, i: u32) {
    db.remember(
        format!("note {i}"),
        vec![i as f32 + 1.0, 0.5, 0.25, 1.0],
        opts(),
    )
    .unwrap();
}

/// A file with 10 committed notes and an 11th whose commit fails at the
/// checkpoint's sync (the third of the flush's syncs, after the commit).
fn after_a_failed_commit(path: &str, key: &[u8]) -> Arc<MnemoDb> {
    let db = MnemoDb::create_with_key(path.to_string(), key.to_vec(), config()).unwrap();
    (0..10).for_each(|i| note(&db, i));
    db.flush().unwrap();
    note(&db, 10);
    mnemo_engine::__failpoints::set_syncs_until_fail(2);
    let first = db.flush();
    mnemo_engine::__failpoints::set_syncs_until_fail(-1);
    assert!(matches!(first, Err(MnemoError::Io { .. })), "{first:?}");
    db
}

#[test]
fn a_refused_flush_is_maybe_stored_and_drops_the_handle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.mnemo").display().to_string();
    let key = generate_key();
    let db = after_a_failed_commit(&path, &key);

    note(&db, 11);
    let refused = db.flush();
    let Err(e @ MnemoError::MaybeStored { .. }) = refused else {
        panic!("{refused:?}")
    };
    assert!(e.is_retryable());
    assert!(e.to_string().contains("reopen"), "{e}");
    // Dropped without flushing: later calls are `Closed`, and closing is a
    // no-op.
    assert!(matches!(db.len(), Err(MnemoError::Closed)));
    db.close().unwrap();

    // The lock is free while `db` is still alive, and the open finishes the
    // commit: the 11th note is there, the one written after it isn't.
    let again = MnemoDb::open_with_key(path, key).unwrap();
    assert_eq!(again.len().unwrap(), 11);
    again.close().unwrap();
    drop(db);
}

#[test]
fn close_after_a_failed_commit_drops_the_handle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.mnemo").display().to_string();
    let key = generate_key();
    let db = after_a_failed_commit(&path, &key);

    let closed = db.close();
    assert!(
        matches!(closed, Err(MnemoError::MaybeStored { .. })),
        "{closed:?}"
    );
    assert!(matches!(db.len(), Err(MnemoError::Closed)));
    db.close().unwrap();

    let again = MnemoDb::open_with_key(path, key).unwrap();
    assert_eq!(again.len().unwrap(), 11);
    again.close().unwrap();
    drop(db);
}
