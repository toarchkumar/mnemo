//! Smoke tests for the `mnemo` CLI exploration commands (`get`, `list`,
//! `recall`).
//!
//! These tests build a tiny database with the library, then invoke the
//! freshly built `mnemo` binary via [`std::process::Command`], asserting on
//! exit code and standard-output substrings. They are deliberately light:
//! the library API is exercised by `tests/integration.rs`; this file only
//! verifies that the CLI wiring around `db.get` / `db.memories` / `db.recall`
//! parses arguments and renders output as expected.

use std::process::Command;

use mnemo::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig};
use tempfile::tempdir;

/// Path to the `mnemo` binary built by Cargo for these tests.
fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mnemo")
}

/// Cheap KDF so the tests are fast.
fn fast_cfg(dimensions: usize) -> MnemoConfig {
    MnemoConfig { dimensions, kdf: KdfParams::fast(), ..Default::default() }
}

/// Create a tiny test database with three memories and return (path,
/// passphrase, id-of-the-semantic-memory).
fn seed_db() -> (tempfile::TempDir, std::path::PathBuf, &'static str, String) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("smoke.mnemo");
    let pp = "smoke-pw";

    let mut db = Mnemo::create(&path, pp, fast_cfg(4)).unwrap();
    let semantic_id = db
        .remember(
            Memory::new("the user prefers tea", MemoryType::Semantic, vec![1.0, 0.0, 0.0, 0.0])
                .with_agent("assistant")
                .with_importance(0.9),
        )
        .unwrap();
    db.remember(
        Memory::new("ran the deploy script", MemoryType::Episodic, vec![0.0, 1.0, 0.0, 0.0])
            .with_agent("assistant")
            .with_importance(0.4),
    )
    .unwrap();
    db.remember(
        Memory::new("escalate via pager", MemoryType::Procedural, vec![0.0, 0.0, 1.0, 0.0])
            .with_agent("ops")
            .with_importance(0.6),
    )
    .unwrap();
    db.flush().unwrap();
    db.close().unwrap();

    (dir, path, pp, semantic_id.to_string())
}

/// Run the CLI with `MNEMO_PASSPHRASE` and return (status, stdout, stderr).
fn run_cli(args: &[&str], pp: &str) -> (std::process::ExitStatus, String, String) {
    let out = Command::new(bin())
        .args(args)
        .env("MNEMO_PASSPHRASE", pp)
        .output()
        .expect("failed to invoke mnemo binary");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (out.status, stdout, stderr)
}

#[test]
fn list_table_shows_three_memories() {
    let (_dir, path, pp, _) = seed_db();
    let (status, stdout, stderr) =
        run_cli(&["list", path.to_str().unwrap(), "--format", "table"], pp);
    assert!(status.success(), "list failed: {stderr}");
    assert!(stdout.contains("showing 3 of 3 memories"), "header missing: {stdout}");
    assert!(stdout.contains("the user prefers tea"), "content missing: {stdout}");
    assert!(stdout.contains("[semantic]"), "type tag missing: {stdout}");
}

#[test]
fn list_filters_by_type_and_agent() {
    let (_dir, path, pp, _) = seed_db();
    let (status, stdout, _) = run_cli(
        &[
            "list",
            path.to_str().unwrap(),
            "--type",
            "procedural",
            "--agent",
            "ops",
        ],
        pp,
    );
    assert!(status.success());
    assert!(stdout.contains("escalate via pager"));
    assert!(!stdout.contains("prefers tea"));
    assert!(!stdout.contains("deploy script"));
}

#[test]
fn list_json_emits_total_and_count() {
    let (_dir, path, pp, _) = seed_db();
    let (status, stdout, _) =
        run_cli(&["list", path.to_str().unwrap(), "--format", "json", "--limit", "2"], pp);
    assert!(status.success());
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json output");
    assert_eq!(v["total"], 3);
    assert_eq!(v["count"], 2);
    let memories = v["memories"].as_array().expect("memories array");
    assert_eq!(memories.len(), 2);
    // Vector is stripped by default.
    assert!(memories[0].get("vector").is_none(), "vector should be omitted");
}

#[test]
fn list_jsonl_with_vector_includes_embedding() {
    let (_dir, path, pp, _) = seed_db();
    let (status, stdout, _) = run_cli(
        &["list", path.to_str().unwrap(), "--format", "jsonl", "--vector"],
        pp,
    );
    assert!(status.success());
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 3);
    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line).expect("jsonl line");
        assert!(v["vector"].as_array().is_some(), "missing vector: {line}");
    }
}

#[test]
fn get_by_id_table_and_json() {
    let (_dir, path, pp, semantic_id) = seed_db();

    let (status, stdout, _) =
        run_cli(&["get", path.to_str().unwrap(), &semantic_id, "--verbose"], pp);
    assert!(status.success());
    assert!(stdout.contains(&semantic_id), "id missing in table output: {stdout}");
    assert!(stdout.contains("the user prefers tea"));
    assert!(stdout.contains("imp=0.90"));
    assert!(stdout.contains("scope"));

    let (status, stdout, _) = run_cli(
        &["get", path.to_str().unwrap(), &semantic_id, "--format", "json"],
        pp,
    );
    assert!(status.success());
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json from get");
    assert_eq!(v["content"], "the user prefers tea");
    assert_eq!(v["memory_type"], "Semantic");
    assert!(v.get("vector").is_none(), "vector should be omitted by default");
}

#[test]
fn get_missing_id_fails_cleanly() {
    let (_dir, path, pp, _) = seed_db();
    // A valid ULID that isn't in the DB.
    let (status, _stdout, stderr) = run_cli(
        &["get", path.to_str().unwrap(), "01ARZ3NDEKTSV4RRFFQ69G5FAV"],
        pp,
    );
    assert!(!status.success(), "missing id should exit non-zero");
    assert!(stderr.contains("not found"), "expected NotFound message, got: {stderr}");
}

#[test]
fn recall_returns_query_nearest_first() {
    let (_dir, path, pp, _) = seed_db();
    let (status, stdout, _) = run_cli(
        &[
            "recall",
            path.to_str().unwrap(),
            "--query",
            "0.9,0.1,0.0,0.0",
            "--top-k",
            "2",
        ],
        pp,
    );
    assert!(status.success());
    let first_line = stdout.lines().next().unwrap_or("");
    assert!(first_line.contains("the user prefers tea"), "expected tea first: {stdout}");
    assert!(first_line.starts_with("score="));
}

#[test]
fn recall_json_carries_score_and_similarity() {
    let (_dir, path, pp, _) = seed_db();
    let (status, stdout, _) = run_cli(
        &[
            "recall",
            path.to_str().unwrap(),
            "--query",
            "0.9,0.1,0.0,0.0",
            "--top-k",
            "1",
            "--format",
            "json",
        ],
        pp,
    );
    assert!(status.success());
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json from recall");
    assert_eq!(v["count"], 1);
    let hits = v["hits"].as_array().expect("hits array");
    assert!(hits[0]["score"].is_number());
    assert!(hits[0]["similarity"].is_number());
    assert_eq!(hits[0]["memory"]["content"], "the user prefers tea");
}

#[test]
fn recall_filters_by_type_excludes_other_kinds() {
    let (_dir, path, pp, _) = seed_db();
    let (status, stdout, _) = run_cli(
        &[
            "recall",
            path.to_str().unwrap(),
            "--query",
            "1.0,0.0,0.0,0.0",
            "--type",
            "procedural",
        ],
        pp,
    );
    assert!(status.success());
    // Only the procedural memory should appear.
    assert!(stdout.contains("escalate via pager"));
    assert!(!stdout.contains("prefers tea"));
    assert!(!stdout.contains("deploy script"));
}
