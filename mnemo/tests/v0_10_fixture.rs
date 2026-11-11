//! FMT-9f: a v8 file built by the published v0.10.0 CLI opens under this
//! build, compacts to v9, and recalls the same way afterwards; v0.10.0
//! refuses the v9 copy. Since the v8 sunset (v0.13) the v8 file opens
//! read-only: a read-write open is `FormatTooOld` until it's compacted.
//!
//! Needs a v0.10.0 `mnemo` binary in `MNEMO_V0_10_BIN` (CI installs it with
//! `cargo install mnemo-engine --version =0.10.0 --locked`). Without it the
//! test prints a notice and passes, so a plain `cargo test` doesn't need
//! the old release. The fixture's memories are
//! `tests/fixtures/v0_10_0_memories.jsonl` (240 memories, 8 dims, eight
//! well-separated topic clusters).

use std::path::Path;
use std::process::Command;

use mnemo_engine::{Memory, MemoryType, Metric, Mnemo, MnemoError, RecallRequest};
use tempfile::tempdir;

const PASSPHRASE: &str = "fixture-pw";

fn old_bin() -> Option<String> {
    std::env::var("MNEMO_V0_10_BIN")
        .ok()
        .filter(|p| Path::new(p).exists())
}

fn run_old(bin: &str, args: &[&str]) -> std::process::Output {
    Command::new(bin)
        .args(args)
        .env("MNEMO_PASSPHRASE", PASSPHRASE)
        .output()
        .expect("spawn the v0.10.0 mnemo")
}

/// One query per topic cluster: the cluster's axis.
fn queries() -> Vec<Vec<f32>> {
    (0..8)
        .map(|t| {
            let mut v = vec![0.0f32; 8];
            v[t] = 1.0;
            v
        })
        .collect()
}

/// The topic word of each hit's content ("release note 3: ..." -> "release").
fn topics(db: &mut Mnemo, q: &[f32]) -> Vec<String> {
    db.recall(&RecallRequest::new(q.to_vec()).top_k(10).track_access(false))
        .unwrap()
        .into_iter()
        .map(|h| h.memory.content.split(' ').next().unwrap().to_string())
        .collect()
}

#[test]
fn a_v0_10_0_file_opens_read_only_and_compacts_to_v9() {
    let Some(bin) = old_bin() else {
        eprintln!("skipped: set MNEMO_V0_10_BIN to a v0.10.0 `mnemo` binary to run this test");
        return;
    };
    let ver = run_old(&bin, &["--version"]);
    assert!(
        String::from_utf8_lossy(&ver.stdout).contains("0.10.0"),
        "MNEMO_V0_10_BIN isn't v0.10.0: {}",
        String::from_utf8_lossy(&ver.stdout)
    );
    let dir = tempdir().unwrap();
    let path = dir.path().join("old.mnemo");
    let p = path.to_str().unwrap();
    let jsonl = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/v0_10_0_memories.jsonl"
    );
    for args in [
        vec!["init", p, "--dimensions", "8", "--no-manifest"],
        vec!["import", p, jsonl],
        vec!["index", p],
    ] {
        let out = run_old(&bin, &args);
        assert!(
            out.status.success(),
            "v0.10.0 {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // Under this build: still v8, and read-only (the v8 sunset).
    match Mnemo::open(&path, PASSPHRASE) {
        Err(MnemoError::FormatTooOld { version: 8 }) => {}
        other => panic!("expected FormatTooOld, got {:?}", other.err()),
    }
    let mut db = Mnemo::open_read_only(&path, PASSPHRASE).unwrap();
    assert_eq!(db.stats().unwrap().format_version, 8);
    let before = db.len();
    assert_eq!(before, 240);
    let mut want = Vec::new();
    for q in &queries() {
        let t = topics(&mut db, q);
        assert!(t.iter().all(|x| x == &t[0]), "one cluster per query: {t:?}");
        want.push(t);
    }
    drop(db);

    // Compaction upgrades it to v9; recall comes back the same.
    let report = Mnemo::compact_file(p, PASSPHRASE).unwrap();
    assert_eq!(report.after, before);
    let mut db = Mnemo::open(&path, PASSPHRASE).unwrap();
    assert_eq!(db.stats().unwrap().format_version, 9);
    assert_eq!(db.verify().unwrap(), before);
    // ... and it's writable again.
    let mut v = vec![0.0f32; 8];
    v[0] = 1.0;
    v[1] = 0.02;
    let added = db
        .remember(Memory::new(
            "release note written by v0.13",
            MemoryType::Semantic,
            v,
        ))
        .unwrap();
    assert_eq!(db.get(&added).unwrap().content, "release note written by v0.13");
    for (q, w) in queries().iter().zip(&want) {
        assert_eq!(&topics(&mut db, q), w, "recall on the v9 copy");
    }
    let exact = db.search(&queries()[3], 5, Metric::Cosine).unwrap();
    assert!(exact.iter().all(|(m, _)| m.content.starts_with("thread")));
    drop(db);

    // v0.10.0 refuses the v9 copy, naming the version.
    let out = run_old(&bin, &["info", p]);
    assert!(!out.status.success(), "v0.10.0 must not open a v9 file");
    let msg = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        msg.contains("unsupported .mnemo format version 9"),
        "v0.10.0's message: {msg}"
    );
}
