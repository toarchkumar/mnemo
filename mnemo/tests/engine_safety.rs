//! v0.12.2 engine safety (MOBILE-2e step 3), as RB-002.4's T4 and T8 plus
//! the RB-001.26 decompression bound.
//!
//! The failpoint switch is process-wide, so every test in this file holds
//! `SERIAL`.

use std::sync::Mutex;

use mnemo_engine::{
    KdfParams, Memory, MemoryPatch, MemoryType, Mnemo, MnemoConfig, MnemoError, RecallRequest,
};
use serde_json::json;
use tempfile::tempdir;

static SERIAL: Mutex<()> = Mutex::new(());

const DIMS: usize = 4;

fn cfg(version: u16) -> MnemoConfig {
    MnemoConfig {
        dimensions: DIMS,
        kdf: KdfParams::fast(),
        format_version: version,
        ..Default::default()
    }
}

fn vector(i: usize) -> Vec<f32> {
    vec![1.0 + i as f32, (i % 3) as f32, 0.5, 1.0]
}

fn mem(text: &str, i: usize) -> Memory {
    Memory::new(text, MemoryType::Semantic, vector(i))
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

/// T8: caller metadata can't set engine state, on any write path.
/// Today (f79715e): 2 of 4 recalled, the forged key in `history`, the forged
/// turn in `thread_export`. After: each forged write is `Invalid`, nothing
/// stored.
#[test]
fn t8_reserved_metadata_is_refused_on_every_write_path() {
    let _g = lock();
    // v9 only since the v8 sunset (v0.13): v8 files can't be written.
    for version in [9u16] {
        let dir = tempdir().unwrap();
        let mut db = Mnemo::create(dir.path().join("t.mnemo"), "pw", cfg(version)).unwrap();
        let normal = db.remember(mem("a normal memory", 0)).unwrap();

        let forged = [
            vec![
                ("_mnemo.superseded_by", json!("01J00000000000000000000000")),
                ("_mnemo.invalid_at", json!(1)),
            ],
            vec![
                ("_mnemo.kind", json!("turn")),
                ("_mnemo.thread", json!("t1")),
            ],
            vec![("_mnemo.key", json!("current-release"))],
        ];
        for (i, keys) in forged.iter().enumerate() {
            let mut m = mem(&format!("forged {i}"), i + 1);
            for (k, v) in keys {
                m.metadata.insert((*k).into(), v.clone());
            }
            match db.remember(m) {
                Err(MnemoError::Invalid(msg)) => {
                    assert!(msg.contains("reserved"), "v{version}: {msg}")
                }
                other => panic!("v{version}: forged write {i} was {other:?}"),
            }
        }
        // remember_keyed and update refuse them too.
        let mut keyed = mem("keyed", 5);
        keyed
            .metadata
            .insert("_mnemo.kind".into(), json!("essence"));
        assert!(matches!(
            db.remember_keyed(keyed, "k"),
            Err(MnemoError::Invalid(_))
        ));
        let mut meta = serde_json::Map::new();
        meta.insert("_mnemo.invalid_at".into(), json!(1));
        let patch = MemoryPatch {
            metadata: Some(meta),
            ..Default::default()
        };
        assert!(matches!(
            db.update(&normal, patch),
            Err(MnemoError::Invalid(_))
        ));

        // Nothing forged was stored, and the normal memory still recalls.
        assert_eq!(db.len(), 1, "v{version}");
        assert!(db.history("current-release", None).unwrap().is_empty());
        assert!(db.thread_export("t1").unwrap().is_empty());
        let hits = db.recall(&RecallRequest::new(vector(0)).top_k(10)).unwrap();
        assert_eq!(hits.len(), 1);

        // Ordinary metadata is fine, and an in-place overwrite may carry the
        // stored record's own `_mnemo.*` keys through unchanged.
        let mut fine = mem("with user metadata", 6);
        fine.metadata
            .insert("mnemo.not-reserved".into(), json!(true));
        db.remember(fine).unwrap();
        let (kid, _) = db.remember_keyed(mem("a keyed fact", 7), "colour").unwrap();
        let mut again = db.get(&kid).unwrap();
        assert!(again.metadata.contains_key("_mnemo.key"));
        again.importance = 0.9;
        db.remember(again.clone()).unwrap();
        // Changing one of them is refused.
        again.metadata.insert("_mnemo.key".into(), json!("other"));
        assert!(matches!(db.remember(again), Err(MnemoError::Invalid(_))));
        db.close().unwrap();
    }
}

/// T4: a failed final flush keeps the handle. Today: `Err`, 10 of 40 on
/// reopen, no handle left. After: `try_close` gives the handle back, and a
/// retry saves all 40.
#[cfg(feature = "failpoints")]
#[test]
fn t4_try_close_keeps_the_handle_when_the_flush_fails() {
    let _g = lock();
    // v9 only since the v8 sunset (v0.13): v8 files can't be written.
    for version in [9u16] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("c.mnemo");
        let mut db = Mnemo::create(&path, "pw", cfg(version)).unwrap();
        for i in 0..10 {
            db.remember(mem(&format!("flushed {i}"), i)).unwrap();
        }
        db.flush().unwrap();
        for i in 10..40 {
            db.remember(mem(&format!("pending {i}"), i)).unwrap();
        }

        mnemo_engine::__failpoints::set_writes_until_fail(0);
        let failed = db.try_close();
        mnemo_engine::__failpoints::set_writes_until_fail(-1);
        let err = failed.expect_err("the flush inside try_close fails");
        assert!(err.to_string().contains("handle is kept"), "{err}");

        // The handle is back, still open, with the 30 unflushed writes.
        let db = err.into_inner();
        assert_eq!(db.len(), 40, "v{version}");
        db.try_close().expect("the retry flushes and closes");

        let db = Mnemo::open_read_only(&path, "pw").unwrap();
        assert_eq!(
            db.len(),
            40,
            "v{version}: every write survived the failed close"
        );
    }
}

/// `close` keeps its 0.12 behaviour (additive API in a patch release): a
/// failed final flush is an error, and the unflushed writes are gone.
#[cfg(feature = "failpoints")]
#[test]
fn close_is_unchanged_in_0_12() {
    let _g = lock();
    let dir = tempdir().unwrap();
    let path = dir.path().join("old.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    for i in 0..10 {
        db.remember(mem(&format!("flushed {i}"), i)).unwrap();
    }
    db.flush().unwrap();
    for i in 10..40 {
        db.remember(mem(&format!("pending {i}"), i)).unwrap();
    }
    mnemo_engine::__failpoints::set_writes_until_fail(0);
    let r = db.close();
    mnemo_engine::__failpoints::set_writes_until_fail(-1);
    assert!(r.is_err());
    assert_eq!(Mnemo::open_read_only(&path, "pw").unwrap().len(), 10);
}

/// RB-001.26: RB-002's frame (about 2 KB, declaring 64 MiB of zeros) is
/// refused from its header, before anything that size is allocated.
#[test]
fn a_record_frame_over_one_mib_is_refused() {
    let _g = lock();
    let bomb = zstd::bulk::compress(&vec![0u8; 64 << 20], 19).unwrap();
    assert!(
        bomb.len() < 4096,
        "the frame is small: {} bytes",
        bomb.len()
    );
    let err = mnemo_engine::__fuzz::decode_v9_memory(&bomb).unwrap_err();
    assert!(err.to_string().contains("too large"), "{err}");
    // Just over the new bound is refused too; the bound itself is not.
    let over = zstd::bulk::compress(&vec![0u8; (1 << 20) + 1], 19).unwrap();
    assert!(mnemo_engine::__fuzz::decode_v9_memory(&over)
        .unwrap_err()
        .to_string()
        .contains("too large"));
    let at = zstd::bulk::compress(&vec![0u8; 1 << 20], 19).unwrap();
    let e = mnemo_engine::__fuzz::decode_v9_memory(&at).unwrap_err();
    assert!(
        !e.to_string().contains("too large"),
        "1 MiB decompresses (then isn't a memory): {e}"
    );
}

/// Builds RB-002.4's fixture 6: a raw-key file killed between a WAL commit
/// and its checkpoint, so a read-only open needs a write open first.
/// Returns the path, the key and the `n` that produced it.
#[cfg(feature = "failpoints")]
fn pending_wal_fixture(dir: &std::path::Path, version: u16) -> (std::path::PathBuf, [u8; 32], i64) {
    let key = Mnemo::generate_key();
    for n in 0..40 {
        let path = dir.join(format!("fx-{version}-{n}.mnemo"));
        let mut db = Mnemo::create_with_key(&path, &key, cfg(version)).unwrap();
        for i in 0..10 {
            db.remember(mem(&format!("flushed {i}"), i)).unwrap();
        }
        db.flush().unwrap();
        for i in 10..15 {
            db.remember(mem(&format!("pending {i}"), i)).unwrap();
        }
        mnemo_engine::__failpoints::set_writes_until_fail(n);
        let _ = db.flush();
        mnemo_engine::__failpoints::set_writes_until_fail(-1);
        drop(db);
        if matches!(
            Mnemo::open_read_only_with_key(&path, &key),
            Err(MnemoError::NeedsWriteOpen { .. })
        ) {
            return (path, key, n);
        }
    }
    panic!("no n gave a pending WAL on v{version}");
}

/// T6: a key probe on a file with a WAL to recover. Today a read-only open
/// with a **wrong** key reports `NeedsWriteOpen` before checking the key, so
/// a store trying each key in turn stopped at the first one. After:
/// `key_opens` answers from the header: `false` for the wrong key, `true`
/// for the right one, and a read-write open then recovers all 15.
#[cfg(feature = "failpoints")]
#[test]
fn t6_key_opens_answers_on_a_file_with_a_wal_to_recover() {
    let _g = lock();
    // v9 only since the v8 sunset (v0.13): v8 files can't be written.
    let version = 9u16;
    let dir = tempdir().unwrap();
    let (path, key, n) = pending_wal_fixture(dir.path(), version);
    let wrong = Mnemo::generate_key();
    // The read-only open still refuses (documented), whatever the key.
    assert!(matches!(
        Mnemo::open_read_only_with_key(&path, &wrong),
        Err(MnemoError::NeedsWriteOpen { .. })
    ));
    assert!(
        !Mnemo::key_opens(&path, &wrong).unwrap(),
        "v{version} n={n}: the wrong key"
    );
    assert!(
        Mnemo::key_opens(&path, &key).unwrap(),
        "v{version} n={n}: the right key"
    );
    let db = Mnemo::open_with_key(&path, &key).unwrap();
    assert_eq!(
        db.len(),
        15,
        "v{version}: the read-write open recovers the WAL"
    );
    db.close().unwrap();
    // A file that isn't a .mnemo is an error, not "not this key".
    let junk = dir.path().join("junk.mnemo");
    std::fs::write(&junk, vec![7u8; 16384]).unwrap();
    assert!(Mnemo::key_opens(&junk, &key).is_err());
}

/// RB-003.3 (v0.13.1): on an intact header, the key is checked before a
/// pending WAL is replayed, so a wrong key leaves the file byte-identical;
/// the right key still recovers everything.
#[cfg(feature = "failpoints")]
#[test]
fn a_wrong_key_leaves_a_pending_wal_untouched() {
    let _g = lock();
    let dir = tempdir().unwrap();
    let (path, key, _) = pending_wal_fixture(dir.path(), 9);
    let before = std::fs::read(&path).unwrap();
    let wrong = Mnemo::generate_key();
    assert!(matches!(
        Mnemo::open_with_key(&path, &wrong),
        Err(MnemoError::WrongPassphrase)
    ));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "a wrong key replayed the WAL"
    );
    let db = Mnemo::open_with_key(&path, &key).unwrap();
    assert_eq!(db.len(), 15, "the right key recovers the pending commit");
    db.close().unwrap();
    assert_ne!(std::fs::read(&path).unwrap(), before);
}

/// RB-003.3 (v0.13.1): at every point a rekey can be cut off, a wrong
/// passphrase leaves the file byte-identical, and the file opens with the
/// old passphrase or the new one. (A rekey logs the new page 0 into a fresh
/// WAL region, so a cut-off one never leaves the new wrap pending behind
/// the old page 0; the key check against the replay's header covers that
/// case anyway.)
#[cfg(feature = "failpoints")]
#[test]
fn a_cut_off_rekey_opens_with_one_passphrase_and_a_wrong_one_writes_nothing() {
    let _g = lock();
    let dir = tempdir().unwrap();
    let mut saw = (false, false);
    for n in 0..40 {
        let path = dir.path().join(format!("rk-{n}.mnemo"));
        let mut db = Mnemo::create(&path, "old-pw", cfg(9)).unwrap();
        for i in 0..3 {
            db.remember(mem(&format!("kept {i}"), i)).unwrap();
        }
        db.flush().unwrap();
        mnemo_engine::__failpoints::set_writes_until_fail(n);
        let _ = db.rekey("new-pw", KdfParams::fast());
        mnemo_engine::__failpoints::set_writes_until_fail(-1);
        drop(db);
        let before = std::fs::read(&path).unwrap();
        assert!(
            matches!(
                Mnemo::open(&path, "wrong-pw"),
                Err(MnemoError::WrongPassphrase)
            ),
            "n={n}"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "n={n}: a wrong passphrase wrote"
        );
        let old = Mnemo::open(&path, "old-pw");
        if let Ok(db) = old {
            assert_eq!(db.len(), 3, "n={n}");
            db.close().unwrap();
            saw.0 = true;
        } else {
            let db = Mnemo::open(&path, "new-pw")
                .unwrap_or_else(|e| panic!("n={n}: neither opens: {e}"));
            assert_eq!(db.len(), 3, "n={n}");
            db.close().unwrap();
            saw.1 = true;
        }
    }
    assert!(
        saw.0 && saw.1,
        "the cut points covered both outcomes: {saw:?}"
    );
}
