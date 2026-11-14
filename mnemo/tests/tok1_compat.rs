//! TOK-1 (v0.15, 12.3 step 4) against a build before v0.15: the published
//! v0.14.0 CLI, named by `MNEMO_V0_14_0_BIN` (CI's `compat (v0.14.0)` job
//! installs it). Without that variable every test here returns at once.
//!
//! - A tokenizer 2 file opens, recalls and writes under v0.14.0 (the
//!   manifest byte stays 1, so older builds keep opening it).
//! - What v0.14.0 writes to a tokenizer 2 file makes the index mixed, which
//!   v0.15 reports and a v0.15 compaction heals.
//! - A v0.14.0 compaction leaves a tokenizer 1 index, which v0.15 reports.
//!
//! The committed tokenizer 1 fixture is written here too
//! (`write_the_tok1_fixture`, ignored).

use std::path::{Path, PathBuf};
use std::process::Command;

use mnemo_engine::{
    KdfParams, KeywordTokenizer, Memory, MemoryType, Mnemo, MnemoConfig, RecallRequest,
};
use tempfile::tempdir;

const PASS: &str = "tok1-compat-passphrase";

fn old_bin() -> Option<PathBuf> {
    std::env::var_os("MNEMO_V0_14_0_BIN").map(PathBuf::from)
}

/// Run the old CLI with only the throwaway passphrase in its environment.
fn run_old(bin: &Path, pass: &str, args: &[&str]) -> String {
    let mut c = Command::new(bin);
    c.args(args);
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().to_string();
        if k.starts_with("MNEMO_") || k == "CLAUDECODE" || k == "CLAUDE_CODE_CHILD_SESSION" {
            c.env_remove(k);
        }
    }
    c.env("MNEMO_PASSPHRASE", pass);
    let out = c.output().expect("run the v0.14.0 CLI");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "v0.14.0 {args:?} failed: {stdout}{stderr}"
    );
    stdout + &stderr
}

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn mem(text: &str, i: usize) -> Memory {
    Memory::new(
        text.to_string(),
        MemoryType::Semantic,
        vec![(i as f32) + 1.0, 1.0, 0.0, 0.0],
    )
}

/// A JSONL file for `mnemo import`, one line per text.
fn jsonl(dir: &Path, name: &str, texts: &[&str]) -> PathBuf {
    let p = dir.join(name);
    let body: String = texts
        .iter()
        .enumerate()
        .map(|(i, t)| {
            serde_json::json!({
                "content": t,
                "vector": [1.0, (i as f32) + 1.0, 0.0, 0.0],
                "memory_type": "semantic",
            })
            .to_string()
                + "\n"
        })
        .collect();
    std::fs::write(&p, body).unwrap();
    p
}

fn keyword_hits(db: &mut Mnemo, k: &str) -> Vec<String> {
    db.recall(&RecallRequest::keywords_only(k).track_access(false))
        .unwrap()
        .into_iter()
        .map(|h| h.memory.content)
        .collect()
}

fn tokenizer(path: &Path) -> KeywordTokenizer {
    let mut db = Mnemo::open_read_only(path, PASS).unwrap();
    db.stats().unwrap().keyword_tokenizer.unwrap()
}

/// A tokenizer 2 file (v0.15 wrote every document) opens, recalls by
/// keyword, verifies and takes a write under v0.14.0.
#[test]
fn a_tokenizer_2_file_opens_recalls_and_writes_under_v0_14_0() {
    let Some(bin) = old_bin() else { return };
    let dir = tempdir().unwrap();
    let p = dir.path().join("t2.mnemo");
    {
        let mut db = Mnemo::create(&p, PASS, cfg()).unwrap();
        db.remember(mem("the repo lives at mnemo.nosync", 0))
            .unwrap();
        db.remember(mem("tea, not coffee", 1)).unwrap();
        db.flush().unwrap();
    }
    assert_eq!(tokenizer(&p), KeywordTokenizer::V2);
    let ps = p.to_str().unwrap();
    // Whole words match under v0.14.0, as they always did.
    let out = run_old(
        &bin,
        PASS,
        &["recall", ps, "--keywords", "mnemo.nosync", "--no-track"],
    );
    assert!(out.contains("mnemo.nosync"), "{out}");
    run_old(&bin, PASS, &["verify", ps]);
    let j = jsonl(dir.path(), "add.jsonl", &["written by v0.14.0"]);
    run_old(&bin, PASS, &["import", ps, j.to_str().unwrap()]);
    let out = run_old(&bin, PASS, &["info", ps]);
    assert!(out.contains("memories:    3"), "{out}");
    // And v0.15 reads all of it back.
    let mut db = Mnemo::open_read_only(&p, PASS).unwrap();
    assert_eq!(db.len(), 3);
    assert_eq!(
        keyword_hits(&mut db, "nosync"),
        vec!["the repo lives at mnemo.nosync"]
    );
}

/// v0.14.0's write to a tokenizer 2 file adds an unmarked segment: v0.15
/// sees a mixed index, whose parts match only its own documents and whose
/// whole words match every document, says so, and a compaction heals it.
#[test]
fn an_older_builds_write_makes_the_index_mixed_and_a_compaction_heals_it() {
    let Some(bin) = old_bin() else { return };
    let dir = tempdir().unwrap();
    let p = dir.path().join("mixed.mnemo");
    {
        let mut db = Mnemo::create(&p, PASS, cfg()).unwrap();
        db.remember(mem("the repo lives at mnemo.nosync", 0))
            .unwrap();
        db.flush().unwrap();
    }
    let ps = p.to_str().unwrap();
    let j = jsonl(
        dir.path(),
        "old.jsonl",
        &["the old copy sat in iCloud's folder"],
    );
    run_old(&bin, PASS, &["import", ps, j.to_str().unwrap()]);
    assert_eq!(tokenizer(&p), KeywordTokenizer::Mixed);
    {
        let mut db = Mnemo::open_read_only(&p, PASS).unwrap();
        // v0.15's document matches by its parts; v0.14.0's doesn't.
        assert_eq!(
            keyword_hits(&mut db, "nosync"),
            vec!["the repo lives at mnemo.nosync"]
        );
        assert!(keyword_hits(&mut db, "icloud").is_empty());
        // Whole words match both.
        assert_eq!(
            keyword_hits(&mut db, "icloud's"),
            vec!["the old copy sat in iCloud's folder"]
        );
        assert_eq!(
            db.verify_report().unwrap().keyword_tokenizer,
            Some(KeywordTokenizer::Mixed)
        );
    }
    // The CLI says so in `info` and `verify`.
    let out = Command::new(env!("CARGO_BIN_EXE_mnemo"))
        .args(["info", ps])
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_CHILD_SESSION")
        .env("MNEMO_PASSPHRASE", PASS)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("keywords:    tokenizer mixed"), "{text}");
    Mnemo::compact_file(ps, PASS).unwrap();
    assert_eq!(tokenizer(&p), KeywordTokenizer::V2);
    let mut db = Mnemo::open_read_only(&p, PASS).unwrap();
    assert_eq!(
        keyword_hits(&mut db, "icloud"),
        vec!["the old copy sat in iCloud's folder"]
    );
}

/// v0.14.0's compaction rebuilds the index with tokenizer 1 and no marks:
/// v0.15 sees tokenizer 1, keeps it so, and its own compaction moves the
/// file to tokenizer 2.
#[test]
fn an_older_builds_compaction_leaves_tokenizer_1() {
    let Some(bin) = old_bin() else { return };
    let dir = tempdir().unwrap();
    let p = dir.path().join("oldc.mnemo");
    {
        let mut db = Mnemo::create(&p, PASS, cfg()).unwrap();
        db.remember(mem("the repo lives at mnemo.nosync", 0))
            .unwrap();
        db.flush().unwrap();
    }
    let ps = p.to_str().unwrap();
    run_old(&bin, PASS, &["compact", ps]);
    assert_eq!(tokenizer(&p), KeywordTokenizer::V1);
    {
        let mut db = Mnemo::open(&p, PASS).unwrap();
        assert!(keyword_hits(&mut db, "nosync").is_empty());
        db.remember(mem(
            "written by v0.15 into a tokenizer 1 file: other.place",
            1,
        ))
        .unwrap();
        db.flush().unwrap();
        // A v0.15 write keeps a tokenizer 1 index pure.
        assert_eq!(
            db.stats().unwrap().keyword_tokenizer,
            Some(KeywordTokenizer::V1)
        );
        assert!(keyword_hits(&mut db, "place").is_empty());
    }
    Mnemo::compact_file(ps, PASS).unwrap();
    assert_eq!(tokenizer(&p), KeywordTokenizer::V2);
    let mut db = Mnemo::open_read_only(&p, PASS).unwrap();
    assert_eq!(
        keyword_hits(&mut db, "nosync"),
        vec!["the repo lives at mnemo.nosync"]
    );
}

/// Writes `tests/fixtures/tok1/v0_14_0_tokenizer_1.mnemo`: an empty file
/// made here (fast, throwaway KDF parameters), into which v0.14.0 imports
/// every document, so its whole keyword index is tokenizer 1. Run with
/// `MNEMO_V0_14_0_BIN=… cargo test --features failpoints --test tok1_compat
/// -- --ignored write_the_tok1_fixture`.
#[test]
#[ignore]
fn write_the_tok1_fixture() {
    let bin = old_bin().expect("set MNEMO_V0_14_0_BIN to the v0.14.0 CLI");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tok1");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("v0_14_0_tokenizer_1.mnemo");
    let _ = std::fs::remove_file(&p);
    drop(Mnemo::create(&p, "tok1-fixture-passphrase", cfg()).unwrap());
    let tmp = tempdir().unwrap();
    let j = jsonl(
        tmp.path(),
        "fixture.jsonl",
        &[
            "The repo lives at ~/Documents/GitHub/mnemo.nosync",
            "The old folder was one of iCloud's placeholders",
            "Compaction renames with std::fs::rename",
            "Tea, not coffee",
        ],
    );
    run_old(
        &bin,
        "tok1-fixture-passphrase",
        &["import", p.to_str().unwrap(), j.to_str().unwrap()],
    );
    let mut db = Mnemo::open_read_only(&p, "tok1-fixture-passphrase").unwrap();
    assert_eq!(db.len(), 4);
    assert_eq!(
        db.stats().unwrap().keyword_tokenizer,
        Some(KeywordTokenizer::V1)
    );
}
