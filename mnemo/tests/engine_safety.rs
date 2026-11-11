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
    for version in [8u16, 9] {
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
    for version in [8u16, 9] {
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
