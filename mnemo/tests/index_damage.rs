//! RB-015.1 (v0.19.1): a damaged ANN index run. The index is derived data,
//! so a run that doesn't decrypt or decode loads as no index, like a layout
//! this build doesn't know: recall scans exactly and gets the right answer,
//! the handle says so once (`notices`), its next commit that writes rebuilds
//! the index, and `verify_index` reports it apart from the records.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::Command;

use mnemo_engine::{
    IndexCheck, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, RecallRequest, Ulid,
    INDEX_PERSIST_EVERY_CHANGES,
};
use tempfile::tempdir;

const PW: &str = "index-damage-passphrase";
const PAGE: u64 = mnemo_engine::__wal::PAGE_SIZE as u64;

fn vector(i: usize) -> Vec<f32> {
    let x = i as f32;
    vec![
        1.0 + (x * 0.37).sin(),
        (x * 0.11).cos(),
        (x * 0.07).sin(),
        0.5 + (x * 0.013).cos(),
    ]
}

/// A file of `n` notes with an index, then `deltas` commits that each write
/// an index delta run. Returns the notes' ids.
fn indexed(path: &Path, n: usize, deltas: usize) -> Vec<Ulid> {
    // Every test in this binary builds a small index.
    std::env::set_var("MNEMO_DEV_ANN_MIN_FACTS", "0");
    let cfg = MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    let mut db = Mnemo::create(path, PW, cfg).unwrap();
    let ids: Vec<Ulid> = (0..n)
        .map(|i| {
            db.remember(Memory::new(
                format!("note {i}"),
                MemoryType::Semantic,
                vector(i),
            ))
            .unwrap()
        })
        .collect();
    assert!(db.build_index().unwrap().partitions > 0);
    db.flush().unwrap();
    for i in 0..deltas {
        db.remember(Memory::new(
            format!("late {i}"),
            MemoryType::Semantic,
            vector(10_000 + i),
        ))
        .unwrap();
        db.flush().unwrap();
    }
    assert_eq!(db.__index_shape().0, deltas);
    db.close().unwrap();
    ids
}

/// `(first page, pages)` of the base run (0) or the newest delta run (1).
fn run_of(path: &Path, which: usize) -> (u64, u64) {
    let db = Mnemo::open_read_only(path, PW).unwrap();
    let r = db.__index_runs()[which];
    assert!(r.1 > 0, "no such run");
    r
}

/// Flip one byte of page `page` at `offset` on disk.
fn flip(path: &Path, page: u64, offset: u64) {
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let at = page * PAGE + offset;
    let mut b = [0u8; 1];
    f.seek(SeekFrom::Start(at)).unwrap();
    f.read_exact(&mut b).unwrap();
    b[0] ^= 0x5a;
    f.seek(SeekFrom::Start(at)).unwrap();
    f.write_all(&b).unwrap();
    f.sync_all().unwrap();
}

fn top(db: &mut Mnemo, i: usize) -> String {
    db.recall(&RecallRequest::new(vector(i)).top_k(1).track_access(false))
        .unwrap()[0]
        .memory
        .content
        .clone()
}

fn damage_notice(db: &Mnemo) -> Option<String> {
    db.notices()
        .iter()
        .find(|n| n.contains("index is damaged"))
        .cloned()
}

#[test]
fn a_damaged_base_page_recalls_exactly_and_the_next_write_rebuilds() {
    // The first page, a middle one, the last, and the last page's tag.
    for (pick, offset) in [(0, 64), (1, 4000), (2, 64), (2, PAGE - 1)] {
        let dir = tempdir().unwrap();
        let p = dir.path().join("t.mnemo");
        indexed(&p, 3000, 0);
        let (start, pages) = run_of(&p, 0);
        assert!(pages >= 3, "a run of {pages} pages");
        let page = match pick {
            0 => start,
            1 => start + pages / 2,
            _ => start + pages - 1,
        };
        flip(&p, page, offset);

        let mut db = Mnemo::open(&p, PW).unwrap();
        assert!(damage_notice(&db).is_none(), "nothing is said at open");
        // Every recall works and is right, the first one too.
        assert_eq!(top(&mut db, 42), "note 42", "page {page} offset {offset}");
        assert_eq!(top(&mut db, 2999), "note 2999");
        assert!(!db.has_index());
        let n = damage_notice(&db).expect("a notice");
        assert!(n.contains("memories are intact"), "{n}");
        assert!(n.contains("mnemo index --rebuild"), "{n}");
        assert_eq!(db.notices().len(), 1, "once: {:?}", db.notices());
        // The records verify; the index is reported apart from them.
        assert_eq!(db.verify().unwrap(), 3000);
        assert!(
            matches!(db.verify_index().unwrap(), IndexCheck::Damaged(_)),
            "{:?}",
            db.verify_index()
        );

        // The next commit that writes rebuilds the index.
        db.remember(Memory::new("fresh", MemoryType::Semantic, vector(7_000)))
            .unwrap();
        db.flush().unwrap();
        assert!(db.has_index());
        assert_eq!(
            db.verify_index().unwrap(),
            IndexCheck::Ok {
                vectors: 3001,
                delta_runs: 0
            }
        );
        db.close().unwrap();
        let mut db = Mnemo::open(&p, PW).unwrap();
        assert_eq!(top(&mut db, 42), "note 42");
        assert_eq!(top(&mut db, 7_000), "fresh");
        assert!(db.has_index());
        assert!(damage_notice(&db).is_none());
        db.close().unwrap();
    }
}

#[test]
fn a_read_only_handle_recalls_exactly_and_says_so() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("t.mnemo");
    indexed(&p, 1000, 0);
    let (start, _) = run_of(&p, 0);
    flip(&p, start, 100);
    let mut db = Mnemo::open_read_only(&p, PW).unwrap();
    assert_eq!(top(&mut db, 7), "note 7");
    let n = damage_notice(&db).expect("a notice");
    assert!(n.contains("`mnemo index --rebuild` repairs it"), "{n}");
    assert!(matches!(db.verify_index().unwrap(), IndexCheck::Damaged(_)));
    assert_eq!(db.verify().unwrap(), 1000);
}

#[test]
fn a_damaged_delta_run_is_damage_too_and_its_runs_are_freed() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("t.mnemo");
    indexed(&p, 1000, 3);
    let (head, _) = run_of(&p, 1);
    flip(&p, head, 50);
    let mut db = Mnemo::open(&p, PW).unwrap();
    assert_eq!(top(&mut db, 10_002), "late 2");
    assert!(damage_notice(&db).is_some());
    // The damaged head is tracked (its pointer is authentic), so the
    // rebuild frees it; the runs before it can't be read from it.
    assert_eq!(db.__index_shape().0, 1);
    assert!(matches!(db.verify_index().unwrap(), IndexCheck::Damaged(_)));
    db.remember(Memory::new("fresh", MemoryType::Semantic, vector(7_000)))
        .unwrap();
    db.flush().unwrap();
    assert_eq!(db.__index_shape().0, 0, "the chain was released");
    assert!(matches!(
        db.verify_index().unwrap(),
        IndexCheck::Ok { vectors: 1004, .. }
    ));
    db.close().unwrap();
}

#[test]
fn a_handle_that_only_reads_leaves_the_file_alone() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("t.mnemo");
    indexed(&p, 1000, 0);
    let (start, _) = run_of(&p, 0);
    flip(&p, start, 100);
    let mut db = Mnemo::open(&p, PW).unwrap();
    assert_eq!(top(&mut db, 7), "note 7");
    db.close().unwrap();
    let mut db = Mnemo::open(&p, PW).unwrap();
    assert!(matches!(db.verify_index().unwrap(), IndexCheck::Damaged(_)));
}

#[test]
fn a_flush_due_to_load_the_index_doesnt_fail_on_damage() {
    // Before v0.19.1, a writer whose pending records reached
    // INDEX_PERSIST_EVERY_CHANGES loaded the index inside `flush`, so a
    // damaged run failed the write.
    let dir = tempdir().unwrap();
    let p = dir.path().join("t.mnemo");
    indexed(&p, 1000, 0);
    let (start, _) = run_of(&p, 0);
    flip(&p, start, 100);
    let mut db = Mnemo::open(&p, PW).unwrap();
    for i in 0..INDEX_PERSIST_EVERY_CHANGES {
        db.remember(Memory::new(
            format!("more {i}"),
            MemoryType::Semantic,
            vector(20_000 + i),
        ))
        .unwrap();
    }
    db.flush().unwrap();
    assert!(db.has_index(), "rebuilt in that same commit");
    assert!(damage_notice(&db).is_some());
    let want = 1000 + INDEX_PERSIST_EVERY_CHANGES;
    assert!(matches!(
        db.verify_index().unwrap(),
        IndexCheck::Ok { vectors, .. } if vectors == want
    ));
}

#[test]
fn a_rebuild_on_a_fresh_handle_frees_the_old_chain() {
    // `mnemo index --rebuild` opens a handle and rebuilds without a recall,
    // so the index was never loaded. Its delta runs must still be freed:
    // the file ends with as many free pages as one that loaded it first.
    let dir = tempdir().unwrap();
    let free_pages = |p: &Path, recall_first: bool| -> u64 {
        let mut db = Mnemo::open(p, PW).unwrap();
        if recall_first {
            top(&mut db, 3);
        }
        db.rebuild_index().unwrap();
        db.flush().unwrap();
        assert_eq!(db.__index_shape().0, 0);
        let (runs, pending) = db.__free_list();
        assert_eq!(pending, 0);
        runs.iter().map(|r| r.2).sum()
    };
    let a = dir.path().join("a.mnemo");
    indexed(&a, 1000, 3);
    let b = dir.path().join("b.mnemo");
    std::fs::copy(&a, &b).unwrap();
    assert_eq!(free_pages(&a, false), free_pages(&b, true));
}

#[test]
fn the_cli_recalls_warns_and_verify_reports_the_index() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("t.mnemo");
    indexed(&p, 1000, 0);
    let (start, _) = run_of(&p, 0);
    flip(&p, start, 100);
    let path = p.to_str().unwrap();
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_mnemo"))
            .args(args)
            .env("MNEMO_PASSPHRASE", PW)
            .env("MNEMO_DEV_NO_KEYCHAIN", "1")
            .env_remove("MNEMO_AGENT")
            .env("MNEMO_CACHE_DIR", dir.path().join("cache"))
            .output()
            .unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    };
    let q: Vec<String> = vector(42).iter().map(|x| x.to_string()).collect();
    let q = q.join(",");

    let (ok, stdout, stderr) = run(&["verify", path]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("verified 1000 records"), "{stdout}");
    assert!(stderr.contains("the ANN index is damaged"), "{stderr}");
    assert!(stderr.contains("mnemo index --rebuild"), "{stderr}");

    let (ok, stdout, stderr) = run(&["recall", path, "--query", &q, "--top-k", "1", "--no-track"]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("note 42"), "{stdout}");
    assert!(stderr.contains("index is damaged"), "{stderr}");
    assert_eq!(stderr.matches("index is damaged").count(), 1, "{stderr}");

    // A tracking recall writes (its access counts), so it rebuilds.
    let (ok, stdout, stderr) = run(&["recall", path, "--query", &q, "--top-k", "1"]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("note 42"), "{stdout}");
    let (ok, stdout, stderr) = run(&["verify", path]);
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains("verified the ANN index — 1000 vectors"),
        "{stdout}"
    );
    assert!(!stderr.contains("damaged"), "{stderr}");
    let (ok, _, stderr) = run(&["recall", path, "--query", &q, "--top-k", "1", "--no-track"]);
    assert!(ok, "{stderr}");
    assert!(!stderr.contains("damaged"), "{stderr}");
}

#[test]
fn a_shared_handle_says_so_and_its_next_write_window_rebuilds() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("t.mnemo");
    indexed(&p, 1000, 0);
    let (start, _) = run_of(&p, 0);
    flip(&p, start, 100);
    let mut s = Mnemo::open_shared(&p, PW, mnemo_engine::SharedOptions::default()).unwrap();
    let hit = s.read(|db| Ok(top(db, 7))).unwrap();
    assert_eq!(hit, "note 7");
    assert!(
        s.notices().iter().any(|n| n.contains("index is damaged")),
        "{:?}",
        s.notices()
    );
    s.write(|db| {
        db.remember(Memory::new("fresh", MemoryType::Semantic, vector(7_000)))
            .map(|_| ())
    })
    .unwrap();
    drop(s);
    let mut db = Mnemo::open(&p, PW).unwrap();
    assert!(matches!(
        db.verify_index().unwrap(),
        IndexCheck::Ok { vectors: 1001, .. }
    ));
}

/// RB-015.2: the fuzz target's two entries, over seeded random inputs, so
/// every `cargo test` runs them (CI's fuzz-smoke job runs the real target).
#[test]
fn the_layout2_fuzz_entries_never_panic() {
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut decoded = 0usize;
    for round in 0..20_000 {
        let n = (next() % 24) as usize * 3 + (round % 3);
        let edits: Vec<u8> = (0..n).map(|_| next() as u8).collect();
        if mnemo_engine::__fuzz::mutate_index_layout2(&edits).is_ok() {
            decoded += 1;
        }
    }
    // The empty edit list (and harmless edits) decode; most edits don't.
    assert!(decoded > 0 && decoded < 20_000, "{decoded}");
    for _ in 0..2_000 {
        let n = (next() % 512) as usize;
        let bytes: Vec<u8> = (0..n).map(|_| next() as u8).collect();
        let _ = mnemo_engine::__fuzz::decode_index_layout2(&bytes);
    }
    assert!(mnemo_engine::__fuzz::mutate_index_layout2(&[]).is_ok());
}
