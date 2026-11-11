//! RB-002.4's T7, the half that needs a file with a WAL to recover (the
//! Swift suite can't make one without failpoints): the stores' probe, as the
//! binding runs it. A wrong key first, then the right one: today the first
//! read-only open threw `Invalid` (`NeedsWriteOpen`) and the store gave up
//! for good. After: `key_opens` says `false`, then `true`, and the read-write
//! open recovers all 15. Its own test binary because failpoints are
//! process-wide.

use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig as EngineConfig};
use mnemo_uniffi::{generate_key, key_opens, MnemoDb, MnemoError};

#[test]
fn t7_the_key_probe_finds_the_key_of_a_file_with_a_wal_to_recover() {
    let dir = tempfile::tempdir().unwrap();
    let key: [u8; 32] = generate_key().try_into().unwrap();
    let cfg = EngineConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    let mut fixture = None;
    for n in 0..40 {
        let path = dir.path().join(format!("fx-{n}.mnemo"));
        let mut db = Mnemo::create_with_key(&path, &key, cfg).unwrap();
        for i in 0..15 {
            if i == 10 {
                db.flush().unwrap();
                mnemo_engine::__failpoints::set_writes_until_fail(-1);
            }
            let v = vec![i as f32 + 1.0, 0.5, 0.25, 1.0];
            db.remember(Memory::new(format!("m {i}"), MemoryType::Semantic, v))
                .unwrap();
        }
        mnemo_engine::__failpoints::set_writes_until_fail(n);
        let _ = db.flush();
        mnemo_engine::__failpoints::set_writes_until_fail(-1);
        drop(db);
        let p = path.display().to_string();
        if matches!(
            MnemoDb::open_read_only_with_key(p.clone(), key.to_vec()),
            Err(MnemoError::NeedsWriteOpen { .. })
        ) {
            fixture = Some(p);
            break;
        }
    }
    let path = fixture.expect("a file with a WAL to recover");

    // The probe, as the stores run it: each stored key in turn.
    let wrong = generate_key();
    let candidates = [wrong, key.to_vec()];
    let found = candidates
        .iter()
        .position(|k| key_opens(path.clone(), k.clone()).unwrap())
        .expect("one of the keys opens it");
    assert_eq!(found, 1, "the wrong key is skipped, not fatal");
    let db = MnemoDb::open_with_key(path, candidates[found].clone()).unwrap();
    assert_eq!(db.len().unwrap(), 15);
    db.close().unwrap();
}
