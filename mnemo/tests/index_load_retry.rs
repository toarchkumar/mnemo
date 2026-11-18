//! Task v0191-1b (v0.19.1): an I/O error while the ANN index loads says
//! nothing about the run, so it fails that call and the next call on the
//! same handle tries the load again. Only a damaged run (RB-015.1,
//! `tests/index_damage.rs`) loads as no index.
//!
//! The read failpoint is process-wide, so this test keeps a binary of its
//! own. Needs the `failpoints` feature (CI's `cargo test --features
//! failpoints`).
#![cfg(feature = "failpoints")]

use mnemo_engine::{
    IndexCheck, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, MnemoError, RecallMode,
    RecallRequest,
};
use tempfile::tempdir;

const PW: &str = "index-retry-passphrase";

fn vector(i: usize) -> Vec<f32> {
    let x = i as f32;
    vec![
        1.0 + (x * 0.37).sin(),
        (x * 0.11).cos(),
        (x * 0.07).sin(),
        0.5 + (x * 0.013).cos(),
    ]
}

fn recall(db: &mut Mnemo, i: usize) -> mnemo_engine::Result<String> {
    Ok(
        db.recall(&RecallRequest::new(vector(i)).top_k(1).track_access(false))?[0]
            .memory
            .content
            .clone(),
    )
}

#[test]
fn an_io_error_on_the_first_load_is_retried_by_the_next_call() {
    std::env::set_var("MNEMO_DEV_ANN_MIN_FACTS", "0");
    let dir = tempdir().unwrap();
    let p = dir.path().join("t.mnemo");
    {
        let cfg = MnemoConfig {
            dimensions: 4,
            kdf: KdfParams::fast(),
            ..Default::default()
        };
        let mut db = Mnemo::create(&p, PW, cfg).unwrap();
        for i in 0..1000 {
            db.remember(Memory::new(
                format!("note {i}"),
                MemoryType::Semantic,
                vector(i),
            ))
            .unwrap();
        }
        assert!(db.build_index().unwrap().partitions > 0);
        db.close().unwrap();
    }

    let mut db = Mnemo::open(&p, PW).unwrap();
    let (base, pages) = db.__index_runs()[0];
    assert!(pages > 0);
    // The first read of the base run's middle page fails, once.
    mnemo_engine::__failpoints::set_page_read_fails(base + pages / 2, 1);
    let first = recall(&mut db, 42);
    mnemo_engine::__failpoints::set_page_read_fails(0, 0);
    assert!(matches!(first, Err(MnemoError::Io(_))), "{first:?}");
    assert!(db.has_index(), "the index is still to load");
    assert!(
        db.notices().iter().all(|n| !n.contains("damaged")),
        "an I/O error isn't damage: {:?}",
        db.notices()
    );

    // The next call loads it, and recalls through it.
    assert_eq!(recall(&mut db, 42).unwrap(), "note 42");
    assert!(db.has_index());
    assert_eq!(db.stats().unwrap().recall_mode, RecallMode::Indexed);
    assert!(db.stats().unwrap().index.is_some());
    assert!(matches!(
        db.verify_index().unwrap(),
        IndexCheck::Ok { vectors: 1000, .. }
    ));
}
