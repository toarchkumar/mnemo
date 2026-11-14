//! 12.3 step 4 (v0.15) through the CLI: `--embed-query` (each request says
//! its side), `recall --no-keywords`, and `mnemo reembed`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use mnemo_engine::{KdfParams, Mnemo, MnemoConfig};
use tempfile::tempdir;

const PASS: &str = "cli-retrieval-passphrase";

fn python_cmd() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

/// A fake embedder of width `DIMS` (argv 1) that logs every request line
/// to the file named by argv 2, then answers with a text-dependent vector.
const LOGGING_EMBED_PY: &str = r#"
import sys, json
DIMS = int(sys.argv[1])
log = open(sys.argv[2], "a")
for line in sys.stdin:
    log.write(line)
    log.flush()
    req = json.loads(line)
    s = req["text"] if isinstance(req, dict) else req
    v = [0.1] * DIMS
    v[sum(bytearray(s.encode("utf-8"))) % DIMS] = float(len(s)) + 1.0
    print(json.dumps(v), flush=True)
"#;

struct Env {
    _dir: tempfile::TempDir,
    brain: PathBuf,
    script: PathBuf,
    log: PathBuf,
}

fn setup() -> Env {
    let dir = tempdir().unwrap();
    let brain = dir.path().join("b.mnemo");
    drop(
        Mnemo::create(
            &brain,
            PASS,
            MnemoConfig {
                dimensions: 4,
                kdf: KdfParams::fast(),
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let script = dir.path().join("embed.py");
    std::fs::write(&script, LOGGING_EMBED_PY).unwrap();
    let log = dir.path().join("requests.log");
    Env {
        brain,
        script,
        log,
        _dir: dir,
    }
}

fn embed_cmd(env: &Env, dims: usize) -> String {
    format!(
        "{} {} {dims} {}",
        python_cmd(),
        env.script.display(),
        env.log.display()
    )
}

fn mnemo(args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_mnemo"));
    c.args(args);
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().to_string();
        if k.starts_with("MNEMO_") || k == "CLAUDECODE" || k == "CLAUDE_CODE_CHILD_SESSION" {
            c.env_remove(k);
        }
    }
    c.env("MNEMO_PASSPHRASE", PASS);
    c.output().unwrap()
}

fn ok(o: &Output) -> String {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(o.status.success(), "{text}");
    text
}

fn lines(log: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn with_embed_query_each_request_says_its_side() {
    let env = setup();
    let b = env.brain.to_str().unwrap();
    let cmd = embed_cmd(&env, 4);
    let common = [
        "--embed-cmd",
        &cmd,
        "--embed-name",
        "fake",
        "--embed-query",
        "fake-q",
    ];
    let mut args = vec!["remember", b, "--text", "the repo lives at mnemo.nosync"];
    args.extend(common);
    ok(&mnemo(&args));
    let mut args = vec!["recall", b, "--text", "where is the repo", "--no-track"];
    args.extend(common);
    let out = ok(&mnemo(&args));
    assert!(out.contains("mnemo.nosync"), "{out}");
    let got = lines(&env.log);
    assert_eq!(
        got,
        vec![
            serde_json::json!({"kind": "document", "text": "the repo lives at mnemo.nosync"}),
            serde_json::json!({"kind": "query", "text": "where is the repo"}),
        ]
    );
    // The convention is part of the identity: without it, refused.
    let out = mnemo(&[
        "recall",
        b,
        "--text",
        "x",
        "--no-track",
        "--embed-cmd",
        &cmd,
        "--embed-name",
        "fake",
    ]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("query_convention=\"fake-q\""), "{err}");
}

#[test]
fn without_embed_query_the_protocol_is_unchanged() {
    let env = setup();
    let b = env.brain.to_str().unwrap();
    let cmd = embed_cmd(&env, 4);
    ok(&mnemo(&[
        "remember",
        b,
        "--text",
        "a plain fact",
        "--embed-cmd",
        &cmd,
        "--embed-name",
        "fake",
    ]));
    ok(&mnemo(&[
        "recall",
        b,
        "--text",
        "plain",
        "--no-track",
        "--embed-cmd",
        &cmd,
        "--embed-name",
        "fake",
    ]));
    assert_eq!(
        lines(&env.log),
        vec![
            serde_json::json!("a plain fact"),
            serde_json::json!("plain")
        ]
    );
}

#[test]
fn no_keywords_turns_the_text_match_off() {
    let env = setup();
    let b = env.brain.to_str().unwrap();
    let cmd = embed_cmd(&env, 4);
    ok(&mnemo(&[
        "remember",
        b,
        "--text",
        "the deploy key is in mnemo.nosync",
        "--embed-cmd",
        &cmd,
        "--embed-name",
        "fake",
    ]));
    let base = [
        "recall",
        b,
        "--text",
        "mnemo.nosync",
        "--no-track",
        "--format",
        "json",
        "--embed-cmd",
        &cmd,
        "--embed-name",
        "fake",
    ];
    // Hybrid by default: the keyword score adds to the similarity.
    let hybrid: serde_json::Value = serde_json::from_slice(&mnemo(&base).stdout).unwrap();
    let mut args = base.to_vec();
    args.push("--no-keywords");
    let plain: serde_json::Value = serde_json::from_slice(&mnemo(&args).stdout).unwrap();
    let score = |v: &serde_json::Value| v["hits"][0]["score"].as_f64().unwrap();
    assert!(score(&hybrid) > score(&plain), "{hybrid} vs {plain}");
    // It conflicts with --keywords, and needs an embedder with --text.
    let mut args = base.to_vec();
    args.extend(["--no-keywords", "--keywords", "x"]);
    assert!(!mnemo(&args).status.success());
    let out = mnemo(&["recall", b, "--text", "x", "--no-keywords", "--no-track"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--no-keywords"));
}

#[test]
fn reembed_moves_the_file_to_the_new_embedder() {
    let env = setup();
    let b = env.brain.to_str().unwrap();
    let cmd = embed_cmd(&env, 4);
    for t in ["first fact", "second fact", "third fact"] {
        ok(&mnemo(&[
            "remember",
            b,
            "--text",
            t,
            "--embed-cmd",
            &cmd,
            "--embed-name",
            "old",
        ]));
    }
    let new_cmd = embed_cmd(&env, 8);
    let out = ok(&mnemo(&[
        "reembed",
        b,
        "--embed-cmd",
        &new_cmd,
        "--embed-name",
        "new",
        "--embed-dims",
        "8",
        "--embed-query",
        "new-q",
    ]));
    assert!(out.contains("re-embedded"), "{out}");
    assert!(
        out.contains("3 memories, old -> new (8 dimensions)"),
        "{out}"
    );
    let mut db = Mnemo::open(&env.brain, PASS).unwrap();
    assert_eq!(db.dimensions(), 8);
    let id = db.embedder_identity().unwrap().unwrap();
    assert_eq!((id.name.as_str(), id.dimensions), ("new", 8));
    assert_eq!(id.query_convention, "new-q");
    drop(db);
    // Reembed sends every text as a document.
    let docs = lines(&env.log)
        .into_iter()
        .filter(|l| l["kind"] == "document")
        .count();
    assert_eq!(docs, 3);
    // The old embedder is refused now; the new one recalls.
    let out = mnemo(&[
        "recall",
        b,
        "--text",
        "fact",
        "--no-track",
        "--embed-cmd",
        &cmd,
        "--embed-name",
        "old",
    ]);
    assert!(!out.status.success());
    let out = ok(&mnemo(&[
        "recall",
        b,
        "--text",
        "second fact",
        "--no-track",
        "--embed-cmd",
        &new_cmd,
        "--embed-name",
        "new",
        "--embed-query",
        "new-q",
    ]));
    assert!(out.contains("second fact"), "{out}");
}

/// One MCP session over stdio: initialize, then each tool call; returns each
/// call's result text by id.
fn mcp(env: &Env, calls: &[(u64, &str, serde_json::Value)]) -> Vec<serde_json::Value> {
    use std::io::Write;
    use std::process::Stdio;
    let cmd = embed_cmd(env, 4);
    let mut lines = vec![
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{}}}"#.to_string(),
        r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#.to_string(),
    ];
    for (id, name, args) in calls {
        lines.push(
            serde_json::json!({
                "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": { "name": name, "arguments": args },
            })
            .to_string(),
        );
    }
    let mut c = Command::new(env!("CARGO_BIN_EXE_mnemo"));
    c.args([
        "serve",
        env.brain.to_str().unwrap(),
        "--mcp",
        "--embed-cmd",
        &cmd,
        "--embed-name",
        "fake",
        "--embed-query",
        "fake-q",
    ]);
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().to_string();
        if k.starts_with("MNEMO_") || k == "CLAUDECODE" || k == "CLAUDE_CODE_CHILD_SESSION" {
            c.env_remove(k);
        }
    }
    let mut child = c
        .env("MNEMO_PASSPHRASE", PASS)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for l in &lines {
            writeln!(stdin, "{l}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut results = Vec::new();
    for l in stdout.lines() {
        let v: serde_json::Value = serde_json::from_str(l).unwrap();
        if v["id"].as_u64().unwrap_or(0) > 1 {
            assert!(v.get("error").is_none(), "{v}");
            let text = v["result"]["content"][0]["text"].as_str().unwrap();
            results.push(serde_json::from_str(text).unwrap());
        }
    }
    results
}

#[test]
fn the_mcp_recall_tool_queries_with_the_query_side_and_is_hybrid_by_default() {
    let env = setup();
    let r = mcp(
        &env,
        &[
            (
                2,
                "remember",
                serde_json::json!({"content": "the deploy key is in mnemo.nosync", "memory_type": "semantic", "text": "the deploy key is in mnemo.nosync"}),
            ),
            (3, "recall", serde_json::json!({"text": "nosync"})),
            (
                4,
                "recall",
                serde_json::json!({"text": "nosync", "keywords": ""}),
            ),
        ],
    );
    let score = |v: &serde_json::Value| v[0]["score"].as_f64().unwrap();
    assert!(score(&r[1]) > score(&r[2]), "{} vs {}", r[1], r[2]);
    let kinds: Vec<String> = lines(&env.log)
        .into_iter()
        .map(|l| l["kind"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(kinds, vec!["document", "query", "query"]);
}
