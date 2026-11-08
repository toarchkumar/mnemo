//! FMT-9c: big runs written outside the WAL, the file shrinking from its
//! tail, and snapshot retention. Every test asks for v9 explicitly.

use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, MnemoError, Ulid};
use tempfile::tempdir;

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 8,
        kdf: KdfParams::fast(),
        format_version: 9,
        ..Default::default()
    }
}

fn mem(i: usize) -> Memory {
    let v: Vec<f32> = (0..8)
        .map(|k| ((i * 7 + k * 3) % 11) as f32 + 0.5)
        .collect();
    Memory::new(format!("memory {i}"), MemoryType::Semantic, v)
}

fn file_len(p: &std::path::Path) -> u64 {
    std::fs::metadata(p).unwrap().len()
}

#[test]
fn a_big_catalog_does_not_grow_the_wal() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg()).unwrap();
    for i in 0..3000 {
        db.remember(mem(i)).unwrap();
    }
    db.flush().unwrap();
    // The catalog is tens of pages; it went straight to its home pages.
    assert_eq!(
        db.stats().unwrap().wal_pages,
        8,
        "the WAL stayed at its default"
    );
    db.remember(mem(9999)).unwrap();
    db.close().unwrap();
    drop(db);
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.verify().unwrap(), 3001);
}

#[test]
fn the_file_shrinks_when_its_tail_is_free() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg()).unwrap();
    db.set_max_snapshots(1);
    let small: Vec<Ulid> = (0..10).map(|i| db.remember(mem(i)).unwrap()).collect();
    db.flush().unwrap();
    // 2 KB records: their pages dwarf the catalog (whose tombstones stay).
    let big: Vec<Ulid> = (10..4000)
        .map(|i| {
            let mut m = mem(i);
            m.content = format!("{} {}", m.content, "b".repeat(2_000));
            db.remember(m).unwrap()
        })
        .collect();
    db.flush().unwrap();
    let grown = file_len(&path);
    for id in &big {
        db.delete(id).unwrap();
    }
    db.flush().unwrap();
    // Two more small commits let the frees clear quarantine and the tail
    // come off.
    for i in 0..3 {
        db.remember(mem(5000 + i)).unwrap();
        db.flush().unwrap();
    }
    let shrunk = file_len(&path);
    assert!(
        shrunk * 2 < grown,
        "the file shrank: {grown} -> {shrunk} bytes"
    );
    for id in &small {
        db.get(id).unwrap();
    }
    assert_eq!(db.verify().unwrap(), 13);
    db.close().unwrap();
    drop(db);
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.verify().unwrap(), 13);
}

#[test]
fn retention_bounds_pinned_space_under_churn() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg()).unwrap();
    let keep: Vec<Ulid> = (0..200).map(|i| db.remember(mem(i)).unwrap()).collect();
    db.flush().unwrap();
    let first = db.snapshots()[0].txn_id;
    // Churn: every round adds and then deletes 200 records. Snapshots are
    // all younger than 24 hours, so only the 25% pin cap prunes them.
    for round in 0..40 {
        let tmp: Vec<Ulid> = (0..200)
            .map(|i| db.remember(mem(1000 + round * 200 + i)).unwrap())
            .collect();
        db.flush().unwrap();
        for id in &tmp {
            db.delete(id).unwrap();
        }
        db.flush().unwrap();
    }
    let snaps = db.snapshots();
    assert!(
        snaps.len() < 80,
        "{} snapshots retained after 80 commits",
        snaps.len()
    );
    // What the retained snapshots keep from reuse stays within the cap: a
    // quarter of the live pages, or 128 pages for a small file.
    let oldest = snaps[0].txn_id;
    let (runs, _) = db.__free_list();
    let free: u64 = runs.iter().map(|r| r.2).sum();
    let pinned: u64 = runs.iter().filter(|r| r.0 > oldest).map(|r| r.2).sum();
    let live = db.__next_page() - free;
    assert!(
        pinned <= (live / 4).max(128),
        "{pinned} pinned of {live} live pages"
    );
    // Restoring past the window is a clear error.
    match db.restore_to(first) {
        Err(MnemoError::NotFound(msg)) => assert!(msg.contains("retention window"), "{msg}"),
        Err(e) => panic!("expected NotFound, got {e:?}"),
        Ok(_) => panic!("txn {first} should be past the retention window"),
    }
    for id in &keep {
        db.get(id).unwrap();
    }
    assert_eq!(db.verify().unwrap(), 200);
}

/// The big runs are written before the commit, outside the WAL: a crash at
/// any write leaves the pre- or the post-commit state.
#[cfg(feature = "failpoints")]
#[test]
fn a_crash_during_out_of_wal_runs_is_pre_or_post() {
    let dir = tempdir().unwrap();
    let base = dir.path().join("base.mnemo");
    {
        let mut db = Mnemo::create(&base, "pw", cfg()).unwrap();
        for i in 0..3000 {
            db.remember(mem(i)).unwrap();
        }
        db.close().unwrap();
    }
    let state = |p: &std::path::Path| {
        let mut db = Mnemo::open(p, "pw").unwrap();
        (db.len(), db.verify().unwrap())
    };
    let ops = |db: &mut Mnemo| -> mnemo_engine::Result<()> {
        for i in 0..50 {
            db.remember(mem(10_000 + i))?;
        }
        db.flush()
    };
    let pre = state(&base);
    let mut saw_pre = false;
    let mut saw_post = false;
    for fail_at in (0..90).step_by(3) {
        let p = dir.path().join(format!("c{fail_at}.mnemo"));
        std::fs::copy(&base, &p).unwrap();
        let mut db = Mnemo::open(&p, "pw").unwrap();
        mnemo_engine::__failpoints::set_writes_until_fail(fail_at);
        let res = ops(&mut db);
        mnemo_engine::__failpoints::set_writes_until_fail(-1);
        drop(db);
        let got = state(&p);
        assert!(
            got == pre || got == (pre.0 + 50, pre.1 + 50),
            "fail_at {fail_at}: {got:?}"
        );
        if got == pre {
            saw_pre = true;
        } else {
            saw_post = true;
        }
        if res.is_ok() {
            break;
        }
    }
    assert!(saw_pre && saw_post);
}

