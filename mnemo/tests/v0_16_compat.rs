//! RB-013.1 (v0.17.0) against the published v0.16.1 CLI, named by
//! `MNEMO_V0_16_1_BIN` (CI's `compat (v0.16.1)` job installs it). Without
//! that variable every test here returns at once.
//!
//! v0.16.1 doesn't know the coordination row kind, so its `compact` writes
//! every message, task, claim and presence row back tagged as a fact. v0.17
//! still finds them (its coordination scan also reads the `episodic` fact
//! rows), says they're tagged as facts, re-tags each on its next write, and
//! a v0.17 compaction re-tags the rest. A private record stays out of
//! recall throughout (the record is checked as well as the row).

use std::path::{Path, PathBuf};
use std::process::Command;

use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, RecallRequest, TaskSpec};
use tempfile::tempdir;

const PASS: &str = "v0-16-compat-passphrase";

fn old_bin() -> Option<PathBuf> {
    std::env::var_os("MNEMO_V0_16_1_BIN").map(PathBuf::from)
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
    let out = c.output().expect("run the v0.16.1 CLI");
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "v0.16.1 {args:?} failed: {text}");
    text
}

#[test]
fn a_v0_16_compaction_hides_nothing_from_v0_17() {
    let Some(old) = old_bin() else {
        eprintln!("MNEMO_V0_16_1_BIN not set: skipped");
        return;
    };
    let dir = tempdir().unwrap();
    let p = dir.path().join("mixed.mnemo");
    {
        let cfg = MnemoConfig {
            dimensions: 4,
            kdf: KdfParams::fast(),
            ..Default::default()
        };
        let mut db = Mnemo::create(&p, PASS, cfg).unwrap();
        db.remember(Memory::scaffold_manifest(4)).unwrap();
        db.remember(Memory::new(
            "the deploy target is staging seven",
            MemoryType::Semantic,
            vec![1.0, 0.0, 0.0, 0.0],
        ))
        .unwrap();
        db.remember(
            Memory::new(
                "a private note about staging seven",
                MemoryType::Episodic,
                vec![1.0, 0.1, 0.0, 0.0],
            )
            .with_visibility("private"),
        )
        .unwrap();
        db.set_author(Some("planner-a".into()));
        db.task_add(&{
            let mut built = TaskSpec::default();
            built.title = "Ship it".into();
            built.role = Some("worker".into());
            built
        })
        .unwrap();
        db.post("worker-a", "staging seven is up", "", &[], None, None, None)
            .unwrap();
        db.set_author(Some("worker-a".into()));
        db.task_next(Some("worker")).unwrap().unwrap();
        db.touch_presence(Some("worker"), Some("cli")).unwrap();
        assert_eq!(db.mistagged_coordination().unwrap(), 0);
        db.close().unwrap();
    }
    let path = p.to_str().unwrap();
    // The old build reads it, then compacts it.
    run_old(&old, &["verify", path]);
    run_old(&old, &["compact", path]);

    let mut db = Mnemo::open(&p, PASS).unwrap();
    // Everything is still found, and said to be tagged as facts.
    let n = db.mistagged_coordination().unwrap();
    assert!(n >= 4, "the task, its claim, the message and presence: {n}");
    assert_eq!(db.tasks(None, None).unwrap().len(), 1);
    assert_eq!(
        db.claims().unwrap().len(),
        1,
        "the task's claim still holds"
    );
    assert_eq!(db.inbox(Some("worker-a"), None, true).unwrap().len(), 1);
    assert_eq!(db.who().unwrap().len(), 1);
    // A second worker can't take the claimed task.
    db.set_author(Some("worker-b".into()));
    assert!(db.task_next(Some("worker")).unwrap().is_none());
    // Recall: no coordination record, and the private one stays hidden.
    for req in [
        RecallRequest::new(vec![1.0, 0.0, 0.0, 0.0]).top_k(10),
        RecallRequest::keywords_only("staging seven").top_k(10),
    ] {
        let hits = db.recall(&req.track_access(false)).unwrap();
        assert!(hits
            .iter()
            .all(|h| !mnemo_engine::is_coordination(&h.memory)));
        assert!(hits
            .iter()
            .all(|h| !h.memory.content.contains("private note")));
        assert!(hits
            .iter()
            .any(|h| h.memory.content.contains("deploy target")));
    }
    // A write re-tags the record it changes.
    db.set_author(Some("worker-a".into()));
    db.task_update("ship-it", None, Some("still going"), None)
        .unwrap();
    db.close().unwrap();
    let mut db = Mnemo::open(&p, PASS).unwrap();
    assert!(db.mistagged_coordination().unwrap() < n);
    db.close().unwrap();
    // A v0.17 compaction re-tags the rest.
    Mnemo::compact_file(path, PASS).unwrap();
    let mut db = Mnemo::open(&p, PASS).unwrap();
    assert_eq!(db.mistagged_coordination().unwrap(), 0);
    assert_eq!(db.tasks(None, None).unwrap().len(), 1);
    assert_eq!(db.inbox(Some("worker-a"), None, true).unwrap().len(), 1);
}
