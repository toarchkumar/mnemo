//! 12.2 step 3: the MCP server's `instructions`, resources and prompts
//! (`docs/design/12.2-onboard.md`, section 7), and `remember` embedding its
//! `content` when it's given no `text` and no vector. Each test runs
//! `mnemo serve --mcp` over stdio, in plain and in shared mode, on a 4-wide
//! brain with a small embed command.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use mnemo_engine::{guide, KdfParams, Mnemo, MnemoConfig};
use serde_json::{json, Value};

const PASS: &str = "mcp-context-test-passphrase";

/// A 4-wide embed command: answers each JSON string with a vector.
const EMBED_PY: &str = r#"
import sys, json
for line in sys.stdin:
    s = json.loads(line)
    w = [1.0, float(len(s) % 7) + 1.0, float(s.count("release")) + 0.5, 0.25]
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
    fn embed_cmd(&self) -> String {
        format!(
            "{} \"{}\"",
            python(),
            self.dir.path().join("embed.py").display()
        )
    }
    /// One server session: send `requests` (method, params) in order and
    /// return each response, by position. With `embed`, the server has the
    /// test embed command.
    fn session(&self, shared: bool, embed: bool, requests: &[(&str, Value)]) -> Vec<Value> {
        self.session_env(shared, embed, &[], requests).0
    }
    /// [`Brain::session`] with extra environment variables, returning the
    /// server's stderr too.
    fn session_env(
        &self,
        shared: bool,
        embed: bool,
        env: &[(&str, &str)],
        requests: &[(&str, Value)],
    ) -> (Vec<Value>, String) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_mnemo"));
        c.args(["serve", "--mcp"]);
        if shared {
            c.arg("--shared");
        }
        c.arg(self.path());
        let cmd = self.embed_cmd();
        if embed {
            c.args(["--embed-cmd", &cmd, "--embed-name", "test-4"]);
        }
        for (k, _) in std::env::vars_os() {
            let k = k.to_string_lossy().to_string();
            if k.starts_with("MNEMO_") || k.starts_with("CLAUDE") {
                c.env_remove(k);
            }
        }
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        c.envs(env.iter().copied());
        c.env("MNEMO_PASSPHRASE", PASS)
            .env("MNEMO_CACHE_DIR", self.dir.path().join("cache"))
            .env("MNEMO_MODEL_URL", closed)
            .stdin(Stdio::piped())
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
        let out = String::from_utf8_lossy(&o.stdout);
        let mut by_id: Vec<(u64, Value)> = out
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter_map(|v| v["id"].as_u64().map(|id| (id, v)))
            .collect();
        by_id.sort_by_key(|(id, _)| *id);
        assert_eq!(
            by_id.len(),
            requests.len() + 1,
            "{out}\n{}",
            String::from_utf8_lossy(&o.stderr)
        );
        (
            by_id.into_iter().map(|(_, v)| v).collect(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    }
}

fn tool(name: &str, args: Value) -> (&'static str, Value) {
    ("tools/call", json!({"name": name, "arguments": args}))
}

fn read(uri: &str) -> (&'static str, Value) {
    ("resources/read", json!({ "uri": uri }))
}

fn ok(r: &Value) -> &Value {
    assert!(r.get("error").is_none(), "{r}");
    assert_ne!(r["result"]["isError"], json!(true), "{r}");
    &r["result"]
}

/// A resource's text.
fn text(r: &Value) -> String {
    ok(r)["contents"][0]["text"].as_str().unwrap().to_string()
}

/// A prompt's text.
fn prompt_text(r: &Value) -> String {
    ok(r)["messages"][0]["content"]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The brain the tests read: three memories (one key, written twice), a
/// long one, a project essence item and a thread's open item, written
/// through the server with no `text` (so `content` is embedded).
fn seed(b: &Brain, shared: bool) {
    let rs = b.session(
        shared,
        true,
        &[
            tool("remember", json!({"content": "Releases are cut from the release branch.",
                "memory_type": "procedural", "key": "process.release"})),
            tool("remember", json!({"content": "The staging database resets on Mondays.",
                "memory_type": "semantic"})),
            tool("remember", json!({"content": format!("A long note: {}", "word ".repeat(400)),
                "memory_type": "semantic"})),
            tool("remember", json!({"content": "Releases are cut from the release branch, then main is fast-forwarded at the tag.",
                "memory_type": "procedural", "key": "process.release"})),
            tool("essence", json!({"op": "add", "project": true, "section": "decisions",
                "text": "Use the release branch for every release."})),
            tool("essence", json!({"op": "add", "thread": "t1", "section": "open",
                "text": "The flaky upload test needs a fix."})),
        ],
    );
    for r in &rs[1..] {
        ok(r);
    }
}

fn both_modes(f: impl Fn(bool)) {
    f(false);
    f(true);
}

#[test]
fn initialize_offers_resources_prompts_and_the_card() {
    let b = Brain::new();
    let rs = b.session(false, false, &[]);
    let r = ok(&rs[0]);
    assert!(r["capabilities"]["resources"].is_object(), "{r}");
    assert!(r["capabilities"]["prompts"].is_object(), "{r}");
    assert!(r["capabilities"]["tools"].is_object(), "{r}");
    assert_eq!(r["instructions"].as_str().unwrap(), guide::card());
}

#[test]
fn the_lists_name_every_resource_template_and_prompt() {
    let b = Brain::new();
    let rs = b.session(
        false,
        false,
        &[
            ("resources/list", json!({})),
            ("resources/templates/list", json!({})),
            ("prompts/list", json!({})),
        ],
    );
    let uris: Vec<&str> = ok(&rs[1])["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uri"].as_str().unwrap())
        .collect();
    assert_eq!(uris, ["mnemo://about", "mnemo://recent", "mnemo://guide"]);
    for r in ok(&rs[1])["resources"].as_array().unwrap() {
        assert_eq!(r["mimeType"], "text/markdown");
    }
    assert_eq!(
        ok(&rs[2])["resourceTemplates"][0]["uriTemplate"],
        "mnemo://key/{key}"
    );
    let prompts: Vec<&str> = ok(&rs[3])["prompts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(prompts, ["session-start", "session-end"]);
}

#[test]
fn every_resource_has_its_shape_budget_and_framing() {
    both_modes(|shared| {
        let b = Brain::new();
        seed(&b, shared);
        let rs = b.session(
            shared,
            true,
            &[
                read("mnemo://about"),
                read("mnemo://recent"),
                read("mnemo://guide"),
                read("mnemo://key/process.release"),
                read("mnemo://key/process%2Erelease"),
            ],
        );
        let about = text(&rs[1]);
        assert!(about.len() <= 8_000, "{}", about.len());
        assert!(about.starts_with(&guide::card()), "{about}");
        let framing = about.find(guide::FRAMING).expect("the framing line");
        assert!(framing < about.find("## About this brain").unwrap());
        assert!(about.contains("Use the release branch for every release."));

        let recent = text(&rs[2]);
        assert!(recent.len() <= 6_000, "{}", recent.len());
        assert!(recent.starts_with(guide::FRAMING), "{recent}");
        // Newest first; the superseded version and essence items aren't
        // memories here; a long one is cut to a line.
        let newest = recent.find("then main is fast-forwarded").unwrap();
        let older = recent.find("staging database").unwrap();
        assert!(newest < older, "{recent}");
        assert!(!recent.contains("Releases are cut from the release branch.\n"));
        assert!(!recent.contains("Use the release branch for every release."));
        assert!(recent.lines().all(|l| l.len() <= 420), "{recent}");
        assert!(recent.contains("key `process.release`"), "{recent}");

        assert_eq!(text(&rs[3]), guide::markdown());

        let key = text(&rs[4]);
        assert!(key.len() <= 4_000);
        assert!(key.starts_with(guide::FRAMING), "{key}");
        let live = key.find("then main is fast-forwarded").unwrap();
        let earlier = key.find("### Earlier versions").unwrap();
        assert!(live < earlier, "{key}");
        assert!(key[earlier..].contains("Releases are cut from the release branch."));
        // A percent-escaped key is the same key.
        assert_eq!(text(&rs[5]), key);
    });
}

#[test]
fn a_missing_key_or_an_unknown_uri_is_resource_not_found() {
    both_modes(|shared| {
        let b = Brain::new();
        let rs = b.session(
            shared,
            false,
            &[
                read("mnemo://key/no.such.key"),
                read("mnemo://nothing"),
                read("mnemo://key/"),
                ("resources/read", json!({})),
            ],
        );
        for r in &rs[1..4] {
            assert_eq!(r["error"]["code"], -32002, "{r}");
            assert!(r["error"]["message"]
                .as_str()
                .unwrap()
                .contains("resource not found"));
        }
        assert_eq!(rs[4]["error"]["code"], -32602, "{}", rs[4]);
    });
}

/// RB-011.1: a key with a `%` before a character wider than a byte is
/// just not found (it ended the server), and a panic inside a request is
/// answered with an internal error: the server goes on after a read, and
/// stops using the file after a tool call.
#[test]
fn a_bad_key_or_a_panic_doesnt_end_the_server() {
    both_modes(|shared| {
        let b = Brain::new();
        seed(&b, shared);
        let rs = b.session(
            shared,
            false,
            &[
                read("mnemo://key/50%→60%"),
                read("mnemo://key/a%é"),
                read("mnemo://key/process%2Erelease"),
            ],
        );
        for r in &rs[1..3] {
            assert_eq!(r["error"]["code"], -32002, "{r}");
        }
        assert!(text(&rs[3]).contains("main is fast-forwarded"), "{}", rs[3]);

        let (rs, err) = b.session_env(
            shared,
            false,
            &[("MNEMO_DEV_MCP_PANIC", "resources/read")],
            &[read("mnemo://about"), ("tools/list", json!({}))],
        );
        assert_eq!(rs[1]["error"]["code"], -32603, "{}", rs[1]);
        assert!(rs[2]["result"]["tools"].is_array(), "{}", rs[2]);
        assert!(
            err.contains("internal error in resources/read; the server goes on"),
            "{err}"
        );

        let (rs, err) = b.session_env(
            shared,
            false,
            &[("MNEMO_DEV_MCP_PANIC", "tools/call")],
            &[tool("stats", json!({})), read("mnemo://about")],
        );
        assert_eq!(rs[1]["error"]["code"], -32603, "{}", rs[1]);
        assert_eq!(rs[2]["error"]["code"], -32603, "{}", rs[2]);
        assert!(
            rs[2]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("restart it"),
            "{}",
            rs[2]
        );
        assert!(
            err.contains("nothing flushed after the internal error"),
            "{err}"
        );
        // The file is as it was.
        let rs = b.session(shared, false, &[read("mnemo://key/process.release")]);
        assert!(text(&rs[1]).contains("main is fast-forwarded"));
    });
}

#[test]
fn the_prompts_with_and_without_their_arguments() {
    both_modes(|shared| {
        let b = Brain::new();
        seed(&b, shared);
        let rs = b.session(
            shared,
            true,
            &[
                (
                    "prompts/get",
                    json!({"name": "session-start", "arguments": {"task": "cut the release"}}),
                ),
                ("prompts/get", json!({"name": "session-start"})),
                ("prompts/get", json!({"name": "session-end"})),
                (
                    "prompts/get",
                    json!({"name": "session-end", "arguments": {"thread": "t1"}}),
                ),
                ("prompts/get", json!({"name": "no-such-prompt"})),
            ],
        );
        let start = prompt_text(&rs[1]);
        assert!(start.len() <= 6_000, "{}", start.len());
        assert!(start.starts_with(&guide::card()));
        assert!(start.contains("The task: cut the release"));
        let framing = start.find(guide::FRAMING).unwrap();
        assert!(framing < start.find("## Project essence").unwrap());
        assert!(start.contains("Use the release branch for every release."));
        assert!(start.contains("## Memories relevant to the task"));
        assert!(start.contains("release branch, then main"), "{start}");
        assert!(start.ends_with("with `remember` and a `key` as you go."));
        assert_eq!(ok(&rs[1])["messages"][0]["role"], "user");

        let start = prompt_text(&rs[2]);
        assert!(start.contains("## Recent memories") && !start.contains("The task:"));

        let end = prompt_text(&rs[3]);
        assert!(end.starts_with("Before ending, save what lasts"));
        assert!(
            !end.contains(guide::FRAMING),
            "nothing from the brain: {end}"
        );
        let end = prompt_text(&rs[4]);
        assert!(end.contains(guide::FRAMING) && end.contains("flaky upload test"));
        assert!(end.len() <= 6_000);

        assert_eq!(rs[5]["error"]["code"], -32602, "{}", rs[5]);
    });
}

#[test]
fn reads_and_prompts_leave_the_access_stats_alone() {
    both_modes(|shared| {
        let b = Brain::new();
        seed(&b, shared);
        let list = || tool("list", json!({}));
        let rs = b.session(
            shared,
            true,
            &[
                list(),
                read("mnemo://about"),
                read("mnemo://recent"),
                read("mnemo://key/process.release"),
                (
                    "prompts/get",
                    json!({"name": "session-start", "arguments": {"task": "release"}}),
                ),
                ("prompts/get", json!({"name": "session-start"})),
                list(),
            ],
        );
        assert_eq!(ok(&rs[1]), ok(&rs[7]));
        // And across a restart, so nothing rode a flush either.
        let again = b.session(shared, true, &[list()]);
        assert_eq!(ok(&rs[1]), ok(&again[1]));
        // The control: a `recall` tool call does count (in plain mode the
        // stats show in the same session; a shared server writes them with
        // its next write).
        if !shared {
            let rs = b.session(
                false,
                true,
                &[
                    tool("recall", json!({"text": "release", "top_k": 3})),
                    list(),
                ],
            );
            assert_ne!(
                ok(&again[1]),
                ok(&rs[2]),
                "recall should count as an access"
            );
        }
    });
}

#[test]
fn remember_embeds_its_content_when_given_no_text() {
    both_modes(|shared| {
        let b = Brain::new();
        let rs = b.session(
            shared,
            true,
            &[
                tool(
                    "remember",
                    json!({"content": "release notes live in CHANGELOG.md",
                    "memory_type": "semantic"}),
                ),
                tool(
                    "recall",
                    json!({"text": "release notes live in CHANGELOG.md",
                    "keywords": "", "top_k": 1}),
                ),
            ],
        );
        ok(&rs[1]);
        let hits = ok(&rs[2])["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(hits.contains("CHANGELOG.md"), "{hits}");
        // Without an embedder, `remember` with no `text` and no vector says
        // what it needs, and stores nothing.
        let b = Brain::new();
        let rs = b.session(
            shared,
            false,
            &[
                tool(
                    "remember",
                    json!({"content": "x", "memory_type": "semantic"}),
                ),
                tool("stats", json!({})),
            ],
        );
        let msg = rs[1]["error"]["message"].as_str().unwrap_or_default();
        assert!(msg.contains("embed"), "{}", rs[1]);
        let stats: Value =
            serde_json::from_str(ok(&rs[2])["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(stats["memories"], 0, "{stats}");
    });
}

/// The brain file is untouched by a session that only reads resources.
#[test]
fn a_resource_only_session_writes_nothing() {
    let b = Brain::new();
    seed(&b, false);
    let before = std::fs::read(b.path()).unwrap();
    b.session(
        false,
        true,
        &[read("mnemo://about"), read("mnemo://recent")],
    );
    assert_eq!(std::fs::read(b.path()).unwrap(), before);
}
