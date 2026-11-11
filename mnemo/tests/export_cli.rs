//! `mnemo export` (v0.13, PLAN.md 12.5): the readable plaintext export.
//!
//! Both formats as output, the filters, `--git`'s determinism (a plain read
//! of the file doesn't change it, and one new memory adds only its own
//! lines), the plaintext warning, `-o`, and a v8 file through the read-only
//! path.

use std::path::{Path, PathBuf};
use std::process::Command;

use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, RecallRequest};
use serde_json::Value;
use tempfile::TempDir;

const PW: &str = "export-pw";

fn run(args: &[&str], pp: &str) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_mnemo"))
        .args(args)
        .env("MNEMO_PASSPHRASE", pp)
        .output()
        .expect("run mnemo");
    (
        out.status.success(),
        String::from_utf8(out.stdout).expect("utf-8 stdout"),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn export(path: &Path, extra: &[&str]) -> String {
    let mut args = vec!["export", path.to_str().unwrap()];
    args.extend_from_slice(extra);
    let (ok, out, err) = run(&args, PW);
    assert!(ok, "export failed: {err}");
    out
}

fn v(i: usize) -> Vec<f32> {
    let mut x = vec![0.0; 4];
    x[i % 4] = 1.0;
    x[(i + 1) % 4] += 0.5;
    x
}

/// Four memories over two agents and three types, one of them keyed with a
/// superseded first version.
fn seed() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("e.mnemo");
    let cfg = MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    let mut db = Mnemo::create(&path, PW, cfg).unwrap();
    db.remember(
        Memory::new("the user prefers tea", MemoryType::Semantic, v(0))
            .with_agent("assistant")
            .with_importance(0.7),
    )
    .unwrap();
    db.remember(
        Memory::new("ran the deploy script", MemoryType::Episodic, v(1)).with_agent("assistant"),
    )
    .unwrap();
    db.remember(
        Memory::new(
            "escalate via pager\n```\nnot a fence\n```",
            MemoryType::Procedural,
            v(2),
        )
        .with_agent("ops"),
    )
    .unwrap();
    db.remember_keyed(
        Memory::new("region: eu-west-1", MemoryType::Semantic, v(3)).with_agent("ops"),
        "deploy:region",
    )
    .unwrap();
    db.remember_keyed(
        Memory::new("region: eu-central-1", MemoryType::Semantic, v(3)).with_agent("ops"),
        "deploy:region",
    )
    .unwrap();
    db.close().unwrap();
    (dir, path)
}

fn jsonl(s: &str) -> Vec<Value> {
    s.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn jsonl_has_one_object_per_memory_and_no_vectors() {
    let (_d, path) = seed();
    let rows = jsonl(&export(&path, &["--format", "jsonl"]));
    // Three plain memories and two versions of the key (`Mnemo::create`
    // adds no scaffold manifest; `mnemo init` does).
    assert_eq!(rows.len(), 5);
    for r in &rows {
        assert!(r.get("vector").is_none());
        assert!(r["id"].is_string() && r["content"].is_string());
        assert!(
            r.get("access_count").is_some(),
            "without --git: access stats"
        );
    }
    let tea = rows
        .iter()
        .find(|r| r["content"] == "the user prefers tea")
        .unwrap();
    assert_eq!(tea["type"], "semantic");
    assert_eq!(tea["agent"], "assistant");
    assert_eq!(tea["importance"].to_string(), "0.7");
    let keyed: Vec<&Value> = rows
        .iter()
        .filter(|r| r["key"] == "deploy:region")
        .collect();
    assert_eq!(keyed.len(), 2, "the superseded version is exported too");
    assert!(keyed
        .iter()
        .any(|r| r["metadata"].get("_mnemo.superseded_by").is_some()));
    // Without --git: ordered by id (creation order).
    let ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
}

#[test]
fn markdown_has_a_section_per_memory_with_its_content_intact() {
    let (_d, path) = seed();
    let md = export(&path, &[]);
    assert!(md.starts_with("# mnemo export\n"));
    assert!(md.contains("5 memories. Vectors are left out."));
    assert_eq!(md.matches("\n## ").count(), 5);
    assert!(md.contains("- agent: `ops`"));
    assert!(md.contains("- key: `deploy:region`"));
    // Content holding a fence gets a longer one, so it survives as written.
    assert!(md.contains("````text\nescalate via pager\n```\nnot a fence\n```\n````\n"));
}

#[test]
fn the_filters_restrict_by_type_and_agent() {
    let (_d, path) = seed();
    let rows = jsonl(&export(
        &path,
        &["--format", "jsonl", "--type", "procedural"],
    ));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["type"], "procedural");
    let rows = jsonl(&export(&path, &["--format", "jsonl", "--agent", "ops"]));
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r["agent"] == "ops"));
    let rows = jsonl(&export(
        &path,
        &[
            "--format",
            "jsonl",
            "--agent",
            "assistant",
            "--type",
            "episodic,semantic",
        ],
    ));
    assert_eq!(rows.len(), 2);
    let (ok, _, err) = run(&["export", path.to_str().unwrap(), "--type", "nope"], PW);
    assert!(!ok && err.contains("nope"), "{err}");
}

#[test]
fn git_output_is_byte_identical_across_reads_and_recalls() {
    let (_d, path) = seed();
    for format in ["markdown", "jsonl"] {
        let a = export(&path, &["--git", "--format", format]);
        let b = export(&path, &["--git", "--format", format]);
        assert_eq!(a, b, "{format}: two exports of an unchanged file");
    }
    let git_md = export(&path, &["--git"]);
    let git_jsonl = export(&path, &["--git", "--format", "jsonl"]);
    let plain = export(&path, &["--format", "jsonl"]);
    assert!(!git_md.contains("accessed") && !git_jsonl.contains("access_count"));

    // A tracked recall changes access stats in the file.
    let mut db = Mnemo::open(&path, PW).unwrap();
    let hits = db.recall(&RecallRequest::new(v(0)).top_k(3)).unwrap();
    assert!(!hits.is_empty());
    db.close().unwrap();

    assert_ne!(
        export(&path, &["--format", "jsonl"]),
        plain,
        "the plain export shows access stats"
    );
    assert_eq!(export(&path, &["--git"]), git_md);
    assert_eq!(export(&path, &["--git", "--format", "jsonl"]), git_jsonl);

    // Keyed memories first, in key order, then the rest by id.
    let rows = jsonl(&git_jsonl);
    assert_eq!(rows[0]["key"], "deploy:region");
    assert_eq!(rows[1]["key"], "deploy:region");
    assert!(rows[0]["id"].as_str() < rows[1]["id"].as_str());
    let rest: Vec<&str> = rows[2..]
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    let mut sorted = rest.clone();
    sorted.sort();
    assert_eq!(rest, sorted);
}

/// The lines of `new` that aren't in `old`, given that `new` is `old` with
/// one contiguous block inserted (panics otherwise).
fn inserted_block(old: &str, new: &str) -> Vec<String> {
    let o: Vec<&str> = old.lines().collect();
    let n: Vec<&str> = new.lines().collect();
    let prefix = o.iter().zip(&n).take_while(|(a, b)| a == b).count();
    let suffix = o[prefix..]
        .iter()
        .rev()
        .zip(n[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    assert_eq!(prefix + suffix, o.len(), "lines of the old export changed");
    n[prefix..n.len() - suffix]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn one_new_memory_changes_only_its_own_lines() {
    let (_d, path) = seed();
    let md = export(&path, &["--git"]);
    let jl = export(&path, &["--git", "--format", "jsonl"]);

    let mut db = Mnemo::open(&path, PW).unwrap();
    let id = db
        .remember(Memory::new("a new note", MemoryType::Semantic, v(1)).with_agent("ops"))
        .unwrap();
    db.close().unwrap();

    let block = inserted_block(&md, &export(&path, &["--git"]));
    assert!(block.iter().any(|l| l == &format!("## {id}")), "{block:?}");
    assert!(block.iter().any(|l| l == "a new note"));
    assert!(block.len() < 16, "{block:?}");

    let block = inserted_block(&jl, &export(&path, &["--git", "--format", "jsonl"]));
    assert_eq!(block.len(), 1);
    assert!(block[0].contains(&id.to_string()));
}

#[test]
fn it_warns_that_the_output_is_plaintext_and_writes_a_file_only_with_o() {
    let (dir, path) = seed();
    let before = std::fs::read(&path).unwrap();
    let (ok, out, err) = run(&["export", path.to_str().unwrap()], PW);
    assert!(ok && !out.is_empty());
    assert!(err.contains("plaintext"), "{err}");

    let dest = dir.path().join("out.md");
    let (ok, out, err) = run(
        &[
            "export",
            path.to_str().unwrap(),
            "-o",
            dest.to_str().unwrap(),
        ],
        PW,
    );
    assert!(ok, "{err}");
    assert!(out.is_empty(), "nothing on stdout with -o");
    assert!(
        err.contains("plaintext") && err.contains("exported 5 memories"),
        "{err}"
    );
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), export(&path, &[]));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // Never over the file itself.
    let (ok, _, err) = run(
        &[
            "export",
            path.to_str().unwrap(),
            "-o",
            path.to_str().unwrap(),
        ],
        PW,
    );
    assert!(!ok && err.contains("refusing"), "{err}");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "exporting changed the file"
    );
}

#[test]
fn a_v8_file_exports_through_the_read_only_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v8.mnemo");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v8/v8_passphrase.mnemo"),
        &path,
    )
    .unwrap();
    let pp = "v8-fixture-passphrase";
    let before = std::fs::read(&path).unwrap();
    let go = |extra: &[&str]| {
        let mut args = vec!["export", path.to_str().unwrap()];
        args.extend_from_slice(extra);
        let (ok, out, err) = run(&args, pp);
        assert!(ok, "{err}");
        out
    };
    let md = go(&[]);
    assert!(md.contains("(format v8, 8 dimensions)"));
    assert!(md.contains("17 memories."));
    // The thread's 4 turns are memories; --git leaves them out.
    let all = jsonl(&go(&["--format", "jsonl"]));
    let git = jsonl(&go(&["--format", "jsonl", "--git"]));
    assert_eq!((all.len(), git.len()), (17, 13));
    assert!(git
        .iter()
        .all(|r| r["metadata"].get("_mnemo.kind") != Some(&Value::from("turn"))));
    assert!(git
        .iter()
        .any(|r| r["metadata"].get("_mnemo.kind") == Some(&Value::from("essence"))));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "exporting changed the v8 file"
    );
}
