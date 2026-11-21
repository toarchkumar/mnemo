//! PERF-2 (v0.19.0) against the published v0.18.1 CLI, named by
//! `MNEMO_V0_18_1_BIN` (CI's `compat (v0.18.1)` job installs it). Without
//! that variable every test here returns at once.
//!
//! v0.19 writes the ANN index in layout 2. v0.18.1 doesn't know that layout,
//! so it loads no index and recalls exactly, with no error; a record it
//! writes is picked up by v0.19's next load. And v0.18.1's own `index`
//! command writes layout 1, which v0.19 still reads, and rewrites as layout
//! 2 with its next base run.
//!
//! v0.21.0 (RB-019.1): v0.18.1 doesn't know the tombstone kind either, so
//! its `compact` writes each tombstone's row back as a `working` fact row.
//! v0.21 still finds them, counts them as tombstones, and keeps them out of
//! recall, and its own compaction re-tags them.

use std::path::{Path, PathBuf};
use std::process::Command;

use mnemo_engine::{
    KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, RecallRequest, ThreadTurn, MNEMO_META_KIND,
    MNEMO_META_TOMBSTONE_OF, MNEMO_META_TOMBSTONE_TURN,
};
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
    // RB-015.2 item 2: at this size too, the old build recalls (exactly)
    // and writes on the layout-2 file before it builds its own index.
    let out = run_old(
        &old,
        &["recall", path, "--query", &csv(&vector(42)), "--top-k", "3"],
    );
    assert!(out.contains("note 42"), "v0.18.1 recall: {out}");
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
    assert_eq!(layout_of(&p), 2, "the old build left the index alone");
    run_old(&old, &["index", path]);
    assert_eq!(layout_of(&p), 1, "v0.18.1 writes layout 1");
    let mut db = Mnemo::open(&p, PASS).unwrap();
    assert!(db.has_index());
    assert_eq!(top(&mut db, &vector(7_777)), "note 7777");
    assert_eq!(top(&mut db, &mine), "written by 0.18.1");
    // The next base this build writes is layout 2, with the same results.
    db.__rewrite_index_base_for_testing().unwrap();
    db.close().unwrap();
    assert_eq!(layout_of(&p), 2);
    let mut db = Mnemo::open(&p, PASS).unwrap();
    assert_eq!(top(&mut db, &vector(7_777)), "note 7777");
    assert_eq!(top(&mut db, &vector(12)), "note 12");
}

fn tombstones(db: &mut Mnemo) -> Vec<Memory> {
    db.memories()
        .unwrap()
        .into_iter()
        .filter(|m| {
            m.metadata
                .get(MNEMO_META_KIND)
                .and_then(serde_json::Value::as_str)
                == Some("tombstone")
        })
        .collect()
}

#[test]
fn an_older_builds_compact_hides_no_tombstone() {
    let Some(old) = old_bin() else {
        eprintln!("MNEMO_V0_18_1_BIN not set: skipped");
        return;
    };
    let dir = tempdir().unwrap();
    let p = dir.path().join("marked.mnemo");
    let (plain, keyed) = {
        let cfg = MnemoConfig {
            dimensions: 4,
            kdf: KdfParams::fast(),
            ..Default::default()
        };
        let mut db = Mnemo::create(&p, PASS, cfg).unwrap();
        db.remember(Memory::scaffold_manifest(4)).unwrap();
        for i in 0..5 {
            db.remember(Memory::new(
                format!("note {i}"),
                MemoryType::Working,
                vector(i),
            ))
            .unwrap();
        }
        let plain = db
            .remember(Memory::new(
                "a plain fact",
                MemoryType::Semantic,
                vector(10),
            ))
            .unwrap();
        let (keyed, _) = db
            .remember_keyed(
                Memory::new("a keyed fact", MemoryType::Episodic, vector(11)),
                "fact.k",
            )
            .unwrap();
        let turns = [ThreadTurn {
            turn_id: "u-1".into(),
            role: "user".into(),
            content: "a turn".into(),
            timestamp: None,
        }];
        db.ingest_thread_with_vectors("t", &turns, &[vector(12)])
            .unwrap();
        db.delete(&plain).unwrap();
        db.delete(&keyed).unwrap();
        assert_eq!(db.thread_forget("t").unwrap(), 1);
        db.close().unwrap();
        (plain, keyed)
    };
    let counts = {
        let mut db = Mnemo::open(&p, PASS).unwrap();
        let c = db.kind_counts().unwrap();
        assert_eq!(c.tombstones, 3);
        assert_eq!(db.mistagged_coordination().unwrap(), 0);
        c
    };
    let path = p.to_str().unwrap();
    run_old(&old, &["verify", path]);
    run_old(&old, &["compact", path]);

    let mut db = Mnemo::open(&p, PASS).unwrap();
    // Every tombstone is found, said to be tagged as a fact, and counted
    // as what it is.
    assert_eq!(db.mistagged_coordination().unwrap(), 3);
    assert_eq!(db.kind_counts().unwrap(), counts);
    let ts = tombstones(&mut db);
    let of: Vec<String> = ts
        .iter()
        .map(|t| {
            t.metadata[MNEMO_META_TOMBSTONE_OF]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert!(of.contains(&plain.to_string()) && of.contains(&keyed.to_string()));
    assert!(ts
        .iter()
        .any(|t| t.metadata.get(MNEMO_META_TOMBSTONE_TURN).is_some()));
    // Out of recall still, and the working facts beside them are found.
    let hits = db
        .recall(&RecallRequest::new(vector(1)).top_k(20).track_access(false))
        .unwrap();
    assert!(hits
        .iter()
        .all(|h| !mnemo_engine::is_coordination(&h.memory)));
    assert!(hits.iter().any(|h| h.memory.content == "note 1"));
    db.close().unwrap();
    // A v0.21 compaction re-tags them.
    Mnemo::compact_file(path, PASS).unwrap();
    let mut db = Mnemo::open(&p, PASS).unwrap();
    assert_eq!(db.mistagged_coordination().unwrap(), 0);
    assert_eq!(db.kind_counts().unwrap(), counts);
    assert_eq!(tombstones(&mut db).len(), 3);
}
