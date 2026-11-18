//! PERF-2 (v0.19.0) against the published v0.18.1 CLI, named by
//! `MNEMO_V0_18_1_BIN` (CI's `compat (v0.18.1)` job installs it). Without
//! that variable every test here returns at once.
//!
//! v0.19 writes the ANN index in layout 2. v0.18.1 doesn't know that layout,
//! so it loads no index and recalls exactly, with no error; a record it
//! writes is picked up by v0.19's next load. And v0.18.1's own `index`
//! command writes layout 1, which v0.19 still reads, and rewrites as layout
//! 2 with its next base run.

use std::path::{Path, PathBuf};
use std::process::Command;

use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, RecallRequest};
use tempfile::tempdir;

const PASS: &str = "v0-18-compat-passphrase";

fn old_bin() -> Option<PathBuf> {
    std::env::var_os("MNEMO_V0_18_1_BIN").map(PathBuf::from)
}

/// Run the old CLI with only the throwaway passphrase in its environment.
fn run_old(bin: &Path, args: &[&str]) -> String {
    let mut c = Command::new(bin);
    c.args(args);
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().to_string();
        if k.starts_with("MNEMO_") || k.starts_with("CLAUDE") || k.starts_with("GIT_") {
            c.env_remove(k);
        }
    }
    c.env("MNEMO_PASSPHRASE", PASS);
    let out = c.output().expect("run the v0.18.1 CLI");
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "v0.18.1 {args:?} failed: {text}");
    text
}

/// A distinct, non-zero four-dimension vector for memory `i`.
fn vector(i: usize) -> Vec<f32> {
    let x = i as f32;
    vec![
        1.0 + (x * 0.37).sin(),
        (x * 0.11).cos(),
        (x * 0.07).sin(),
        0.5 + (x * 0.013).cos(),
    ]
}

fn csv(v: &[f32]) -> String {
    v.iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// A file of `n` memories `note <i>`, with an index built by this build.
fn indexed_file(path: &Path, n: usize) {
    std::env::set_var("MNEMO_DEV_ANN_MIN_FACTS", "0");
    let cfg = MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    let mut db = Mnemo::create(path, PASS, cfg).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    for i in 0..n {
        db.remember(Memory::new(
            format!("note {i}"),
            MemoryType::Semantic,
            vector(i),
        ))
        .unwrap();
    }
    assert!(db.build_index().unwrap().partitions > 0);
    db.flush().unwrap();
    db.close().unwrap();
}

/// The layout byte of the file's index run (0 when it has none).
fn layout_of(path: &Path) -> u8 {
    let mut db = Mnemo::open(path, PASS).unwrap();
    db.__index_load_split()
        .unwrap()
        .into_iter()
        .find(|(k, _)| *k == "layout")
        .map_or(0, |(_, v)| v as u8)
}

fn top(db: &mut Mnemo, v: &[f32]) -> String {
    db.recall(&RecallRequest::new(v.to_vec()).top_k(1).track_access(false))
        .unwrap()[0]
        .memory
        .content
        .clone()
}

#[test]
fn an_older_build_falls_back_and_its_writes_are_picked_up() {
    let Some(old) = old_bin() else {
        eprintln!("MNEMO_V0_18_1_BIN not set: skipped");
        return;
    };
    // CI installs 0.18.1; any 0.18 build reads layout 1 only.
    let version = run_old(&old, &["--version"]);
    assert!(version.contains("mnemo 0.18."), "{version}");
    let dir = tempdir().unwrap();
    let p = dir.path().join("l2.mnemo");
    indexed_file(&p, 300);
    assert_eq!(layout_of(&p), 2, "this build writes layout 2");
    let path = p.to_str().unwrap();

    // The old build recalls exactly, with no error, and the right answer.
    let out = run_old(
        &old,
        &["recall", path, "--query", &csv(&vector(42)), "--top-k", "3"],
    );
    assert!(out.contains("note 42"), "v0.18.1 recall: {out}");
    run_old(&old, &["info", path]);
    // It writes a record.
    let jsonl = dir.path().join("old.jsonl");
    let mine = vec![9.0, -3.0, 4.0, 0.25];
    std::fs::write(
        &jsonl,
        format!(
            "{{\"content\": \"written by 0.18.1\", \"vector\": [{}]}}\n",
            csv(&mine)
        ),
    )
    .unwrap();
    run_old(&old, &["import", path, jsonl.to_str().unwrap()]);

    // This build still has its index (the old build left it alone), finds
    // the old build's record through it, and the right note too.
    assert_eq!(layout_of(&p), 2);
    let mut db = Mnemo::open(&p, PASS).unwrap();
    assert!(db.has_index());
    assert_eq!(top(&mut db, &mine), "written by 0.18.1");
    assert_eq!(top(&mut db, &vector(42)), "note 42");
}

#[test]
fn an_older_builds_index_is_read_and_rewritten() {
    let Some(old) = old_bin() else {
        eprintln!("MNEMO_V0_18_1_BIN not set: skipped");
        return;
    };
    // A release build of 0.18.1 builds an index only at 20,000 facts.
    let dir = tempdir().unwrap();
    let p = dir.path().join("l1.mnemo");
    indexed_file(&p, 20_100);
    let path = p.to_str().unwrap();
    run_old(&old, &["index", path]);
    assert_eq!(layout_of(&p), 1, "v0.18.1 writes layout 1");
    let mut db = Mnemo::open(&p, PASS).unwrap();
    assert!(db.has_index());
    assert_eq!(top(&mut db, &vector(7_777)), "note 7777");
    // The next base this build writes is layout 2, with the same results.
    db.__rewrite_index_base_for_testing().unwrap();
    db.close().unwrap();
    assert_eq!(layout_of(&p), 2);
    let mut db = Mnemo::open(&p, PASS).unwrap();
    assert_eq!(top(&mut db, &vector(7_777)), "note 7777");
    assert_eq!(top(&mut db, &vector(12)), "note 12");
}
