//! RB-013.2 (v0.17.0): private records are shown only when asked. One test
//! walks every read surface (the CLI's commands, the MCP tools, the
//! resources and the prompts) over a brain holding private records of
//! every shape, and fails if any default output shows one. A new surface
//! that shows them by default should fail it too: add it here.
//!
//! Also: a keyed write that gives no level keeps the live version's (and
//! its review date), and says when a write changes it.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use mnemo_engine::{KdfParams, Mnemo, MnemoConfig};
use serde_json::{json, Value};

const PASS: &str = "visibility-surfaces-passphrase";
const SECRET: &str = "QZPRIV";

const EMBED_PY: &str = r#"
import sys, json
for line in sys.stdin:
    s = json.loads(line)
    w = [1.0, float(len(s) % 7) + 1.0, float(s.count("QZ")) + 0.5, 0.25]
    print(json.dumps(w), flush=True)
"#;

fn python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

struct Brain {
    dir: tempfile::TempDir,
}

impl Brain {
    fn new() -> Brain {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("embed.py"), EMBED_PY).unwrap();
        let cfg = MnemoConfig {
            dimensions: 4,
            kdf: KdfParams::fast(),
            ..Default::default()
        };
        Mnemo::create(dir.path().join("b.mnemo"), PASS, cfg)
            .unwrap()
            .close()
            .unwrap();
        Brain { dir }
    }
    fn path(&self) -> PathBuf {
        self.dir.path().join("b.mnemo")
    }
    fn embed(&self) -> String {
        format!(
            "{} \"{}\"",
            python(),
            self.dir.path().join("embed.py").display()
        )
    }
    fn clean(&self, c: &mut Command) {
        for (k, _) in std::env::vars_os() {
            let k = k.to_string_lossy().to_string();
            if k.starts_with("MNEMO_") || k.starts_with("CLAUDE") || k.starts_with("GIT_") {
                c.env_remove(k);
            }
        }
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        c.env("MNEMO_PASSPHRASE", PASS)
            .env("MNEMO_CACHE_DIR", self.dir.path().join("cache"))
            .env("MNEMO_MODEL_URL", closed)
            .env("HOME", self.dir.path());
    }
    /// One MCP session: each response, by position (0 is `initialize`).
    fn mcp(&self, requests: &[(&str, Value)]) -> Vec<Value> {
        let mut c = Command::new(env!("CARGO_BIN_EXE_mnemo"));
        c.args(["serve", "--mcp", "--shared"]).arg(self.path());
        let embed = self.embed();
        c.args(["--embed-cmd", &embed, "--embed-name", "test-4"]);
        self.clean(&mut c);
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        {
            let mut stdin = child.stdin.take().unwrap();
            let init = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
                "params": {"protocolVersion": "2025-06-18"}});
            writeln!(stdin, "{init}").unwrap();
            for (i, (method, params)) in requests.iter().enumerate() {
                let r = json!({"jsonrpc": "2.0", "id": i + 1, "method": method, "params": params});
                writeln!(stdin, "{r}").unwrap();
            }
        }
        let o = child.wait_with_output().unwrap();
        let mut by_id: Vec<(u64, Value)> = String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter_map(|v| v["id"].as_u64().map(|id| (id, v)))
            .collect();
        by_id.sort_by_key(|(id, _)| *id);
        assert_eq!(
            by_id.len(),
            requests.len() + 1,
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        by_id.into_iter().map(|(_, v)| v).collect()
    }
    /// One CLI command on the brain (its path is the first argument after
    /// the subcommand's words, given by `args`).
    fn cli(&self, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_mnemo"));
        c.args(args);
        self.clean(&mut c);
        c.stdin(Stdio::null()).output().unwrap()
    }
}

fn tool(name: &str, args: Value) -> (&'static str, Value) {
    ("tools/call", json!({"name": name, "arguments": args}))
}

/// Everything a response says, as text (a tool's result, a resource's
/// contents, a prompt's messages).
fn said(r: &Value) -> String {
    assert!(r.get("error").is_none(), "{r}");
    r["result"].to_string()
}

#[test]
fn every_read_surface_leaves_private_records_out() {
    let b = Brain::new();
    let p = b.path();
    let p = p.to_str().unwrap();
    // Private records of every shape, beside visible ones.
    let rs = b.mcp(&[
        tool("remember", json!({"content": "The deploy target is staging seven.", "memory_type": "semantic"})),
        tool("remember", json!({"content": format!("{SECRET} unkeyed note about staging"), "memory_type": "semantic", "visibility": "private"})),
        tool("remember", json!({"content": format!("{SECRET} keyed draft"), "memory_type": "semantic", "visibility": "private", "key": "plan.draft"})),
        tool("remember", json!({"content": format!("{SECRET} onboarding note"), "memory_type": "semantic", "visibility": "private", "metadata": {"area": "onboarding"}})),
        tool("remember", json!({"content": format!("{SECRET} overdue note"), "memory_type": "semantic", "visibility": "private", "review_by": 1_600_000_000})),
        tool("essence", json!({"op": "add", "project": true, "section": "decisions", "text": "Use the release branch."})),
        tool("essence", json!({"op": "add", "project": true, "section": "open", "text": format!("{SECRET} open item")})),
        tool("inbox", json!({"op": "post", "to": "all", "subject": format!("{SECRET} broadcast")})),
    ]);
    for r in &rs[1..] {
        said(r);
    }
    // The essence item and the message are made private by `update`.
    let ids: Vec<String> = [&rs[7], &rs[8]]
        .iter()
        .map(|r| {
            let t = r["result"]["content"][0]["text"].as_str().unwrap();
            let v: Value = serde_json::from_str(t).unwrap_or(json!({}));
            v["id"].as_str().map(str::to_string).unwrap_or_else(|| {
                t.split_whitespace()
                    .find(|w| w.len() == 26)
                    .unwrap_or("")
                    .to_string()
            })
        })
        .collect();
    for id in &ids {
        assert_eq!(id.len(), 26, "an id: {ids:?}");
        let o = b.cli(&["update", p, id, "--visibility", "private"]);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }

    // The MCP tools, resources and prompts, each with its defaults.
    let surfaces: Vec<(&str, (&str, Value))> = vec![
        (
            "recall by text",
            tool(
                "recall",
                json!({"text": format!("{SECRET} staging"), "top_k": 20}),
            ),
        ),
        (
            "recall by keywords",
            tool("recall", json!({"keywords": SECRET, "top_k": 20})),
        ),
        ("about", tool("about", json!({}))),
        ("about stale", tool("about", json!({"stale": true}))),
        ("list", tool("list", json!({}))),
        ("history", tool("history", json!({"key": "plan.draft"}))),
        ("stats", tool("stats", json!({}))),
        (
            "essence render",
            tool("essence", json!({"op": "render", "project": true})),
        ),
        (
            "inbox list",
            tool("inbox", json!({"op": "list", "all": true})),
        ),
        (
            "mnemo://about",
            ("resources/read", json!({"uri": "mnemo://about"})),
        ),
        (
            "mnemo://recent",
            ("resources/read", json!({"uri": "mnemo://recent"})),
        ),
        (
            "mnemo://key/plan.draft",
            ("resources/read", json!({"uri": "mnemo://key/plan.draft"})),
        ),
        (
            "mnemo://guide",
            ("resources/read", json!({"uri": "mnemo://guide"})),
        ),
        (
            "session-start",
            ("prompts/get", json!({"name": "session-start"})),
        ),
        (
            "session-start with a task",
            (
                "prompts/get",
                json!({"name": "session-start", "arguments": {"task": "the staging note"}}),
            ),
        ),
        (
            "session-end",
            ("prompts/get", json!({"name": "session-end"})),
        ),
    ];
    let reqs: Vec<(&str, Value)> = surfaces.iter().map(|(_, r)| r.clone()).collect();
    let rs = b.mcp(&reqs);
    for ((name, _), r) in surfaces.iter().zip(&rs[1..]) {
        let t = r.to_string();
        assert!(
            !t.contains(SECRET),
            "MCP {name} shows a private record: {t}"
        );
    }

    // The CLI, with its defaults.
    let cli: Vec<Vec<&str>> = vec![
        vec!["recall", p, "--keywords", SECRET, "--top-k", "20"],
        vec!["about", p],
        vec!["about", p, "--format", "json"],
        vec!["list", p],
        vec!["list", p, "--format", "jsonl"],
        vec!["history", p, "--key", "plan.draft"],
        vec!["stale", p],
        vec!["export", p],
        vec!["export", p, "--format", "jsonl"],
        vec!["export", p, "--git"],
        vec!["export", p, "--git", "--visibility", "all"],
        vec!["inbox", p, "--all", "--as", "worker-a"],
        vec!["essence", "render", p, "--project"],
        vec!["search", p, "--query", "1.0,1.0,0.5,0.25", "--top-k", "20"],
    ];
    for args in &cli {
        let o = b.cli(args);
        let out = String::from_utf8_lossy(&o.stdout);
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(
            !out.contains(SECRET),
            "the CLI's {args:?} shows a private record: {out}"
        );
    }

    // Asked for, they're there: the test can see them.
    let o = b.cli(&["list", p, "--visibility", "all"]);
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains(&format!("[private] {SECRET} keyed draft")),
        "{out}"
    );
    let rs = b.mcp(&[tool("list", json!({"visibility": ["all"]}))]);
    assert!(said(&rs[1]).contains(SECRET));
}

/// Part B: a keyed write that gives no level keeps the live version's, and
/// its review date; a write that changes the level says so.
#[test]
fn a_keyed_write_keeps_the_level_and_says_when_it_changes() {
    let b = Brain::new();
    let p = b.path();
    let p = p.to_str().unwrap();
    let embed = b.embed();
    let remember = |text: &str, extra: &[&str]| -> String {
        let mut a = vec![
            "remember",
            p,
            "--text",
            text,
            "--key",
            "plan.draft",
            "--embed-cmd",
            &embed,
            "--embed-name",
            "test-4",
        ];
        a.extend(extra);
        let o = b.cli(&a);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    remember(
        "plan v1",
        &["--visibility", "private", "--review-by", "2030-01-01"],
    );
    let out = remember("plan v2", &[]);
    assert!(out.ends_with("\tsuperseded"), "no level change: {out}");
    let o = b.cli(&["list", p, "--visibility", "all", "--format", "jsonl"]);
    let live: Vec<Value> = String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .filter(|v: &Value| v["content"] == "plan v2")
        .collect();
    assert_eq!(
        live[0]["metadata"]["_mnemo.visibility"], "private",
        "{live:?}"
    );
    assert!(
        live[0]["metadata"]["_mnemo.review_by"].is_number(),
        "{live:?}"
    );
    let o = b.cli(&["list", p]);
    assert!(!String::from_utf8_lossy(&o.stdout).contains("plan v2"));
    // Saying another level changes it, and says so.
    let out = remember("plan v3", &["--visibility", "team"]);
    assert!(out.ends_with("\tsuperseded (private → team)"), "{out}");
    // Over MCP too.
    let rs = b.mcp(&[tool("remember", json!({"content": "plan v4", "memory_type": "semantic", "key": "plan.draft", "visibility": "private"}))]);
    let v: Value =
        serde_json::from_str(rs[1]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(v["outcome"], "superseded (team → private)", "{v}");
    assert_eq!(v["visibility_changed"]["from"], "team");
}

/// RB-013.4: the MCP `agent_id` of a new write is checked.
#[test]
fn an_invalid_agent_id_is_refused_over_mcp() {
    let b = Brain::new();
    let rs = b.mcp(&[
        tool(
            "remember",
            json!({"content": "x", "memory_type": "semantic", "agent_id": "two words"}),
        ),
        tool(
            "remember",
            json!({"content": "x", "memory_type": "semantic", "agent_id": "worker-a"}),
        ),
    ]);
    let bad = &rs[1];
    assert!(
        bad["error"].is_object() || bad["result"]["isError"] == json!(true),
        "{bad}"
    );
    assert!(bad.to_string().contains("agent_id"), "{bad}");
    said(&rs[2]);
}
