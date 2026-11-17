//! Review round 1 (RB-014) against the published v0.17.2 CLI, named by
//! `MNEMO_V0_17_2_BIN` (CI's `compat (v0.17.2)` job installs it). Without
//! that variable every test here returns at once.
//!
//! v0.17 doesn't know the `finding` kind, so it reads a finding as an
//! ordinary fact. It does respect `private`: v0.18 writes a `SECURITY`
//! finding `private` until it's verified, so the old build's `export --git`,
//! `list` and recall leave an unverified one out, and show a verified one.

use std::path::{Path, PathBuf};
use std::process::Command;

use mnemo_engine::{FindingSpec, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig};
use tempfile::tempdir;

const PASS: &str = "v0-17-compat-passphrase";

fn old_bin() -> Option<PathBuf> {
    std::env::var_os("MNEMO_V0_17_2_BIN").map(PathBuf::from)
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
    let out = c.output().expect("run the v0.17.2 CLI");
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "v0.17.2 {args:?} failed: {text}");
    text
}

#[test]
fn an_older_build_leaves_out_an_unverified_security_finding() {
    let Some(old) = old_bin() else {
        eprintln!("MNEMO_V0_17_2_BIN not set: skipped");
        return;
    };
    // CI installs 0.17.2; any 0.17 build reads `private` the same way.
    let version = run_old(&old, &["--version"]);
    assert!(version.contains("mnemo 0.17."), "{version}");
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
        db.set_author(Some("reviewer-b".into()));
        let add = |db: &mut Mnemo, id: &str, category: &str, title: &str| {
            db.finding_add(&FindingSpec {
                id: id.into(),
                category: category.into(),
                priority: "MEDIUM".into(),
                title: title.into(),
                body: format!("{title}: the body"),
                ..Default::default()
            })
            .unwrap();
        };
        add(&mut db, "RB-900.1", "SECURITY", "zebrahidden open one");
        add(&mut db, "RB-900.2", "SECURITY", "zebrahidden accepted one");
        add(&mut db, "RB-900.3", "SECURITY", "zebrashown verified one");
        add(
            &mut db,
            "RB-900.4",
            "CORRECTNESS",
            "zebrashown correctness one",
        );
        db.set_author(Some("planner-a".into()));
        db.finding_triage("RB-900.2", "accepted", "zebrahidden triage text")
            .unwrap();
        db.finding_triage("RB-900.3", "accepted", "fix it").unwrap();
        db.set_author(Some("reviewer-b".into()));
        db.finding_verify("RB-900.3", "v0.18.0").unwrap();
        db.close().unwrap();
    }
    let path = p.to_str().unwrap();
    let outputs = [
        ("export --git", run_old(&old, &["export", path, "--git"])),
        (
            "export --git jsonl",
            run_old(&old, &["export", path, "--git", "--format", "jsonl"]),
        ),
        ("export", run_old(&old, &["export", path])),
        ("list", run_old(&old, &["list", path])),
        (
            "recall by keyword",
            run_old(
                &old,
                &[
                    "recall",
                    path,
                    "--keywords",
                    "zebrahidden zebrashown",
                    "--top-k",
                    "20",
                ],
            ),
        ),
        (
            "recall by the sentinel's direction",
            run_old(
                &old,
                &[
                    "recall",
                    path,
                    "--query",
                    "0.5,0.5,0.5,0.5",
                    "--top-k",
                    "20",
                ],
            ),
        ),
    ];
    for (what, out) in &outputs {
        assert!(!out.contains("zebrahidden"), "v0.17.2 {what}: {out}");
    }
    // The old build does see the rest, so the check above means something.
    for (what, out) in &outputs[..4] {
        assert!(
            out.contains("zebrashown verified one") && out.contains("zebrashown correctness one"),
            "v0.17.2 {what}: {out}"
        );
    }
    // And v0.18 still has all four.
    let mut db = Mnemo::open_read_only(&p, PASS).unwrap();
    assert_eq!(
        db.findings(&Default::default()).unwrap().len(),
        4,
        "v0.18's finding list"
    );
}
