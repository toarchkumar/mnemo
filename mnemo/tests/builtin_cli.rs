//! 12.3 step 5 (v0.15): the built-in embedder through the CLI
//! (`embed-candle`): `mnemo embedder`, and `remember`, `recall --text`,
//! `reembed` and `serve --mcp` with no `--embed-cmd`.
//!
//! Every run gets its own cache (`MNEMO_CACHE_DIR`) and a download base
//! (`MNEMO_MODEL_URL`): a closed port for the offline tests, or a local
//! server of the model's files from `MNEMO_TEST_MODEL_DIR` for the rest
//! (skipped without it). Nothing reaches the network or the real cache.
#![cfg(feature = "embed-candle")]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use mnemo_engine::builtin::{self, FILES};
use mnemo_engine::{KdfParams, Mnemo, MnemoConfig};
use tempfile::tempdir;

const PASS: &str = "builtin-cli-test-passphrase";

fn model_dir() -> Option<PathBuf> {
    match std::env::var_os("MNEMO_TEST_MODEL_DIR") {
        Some(d) if !d.is_empty() => Some(PathBuf::from(d)),
        _ => {
            eprintln!("skipped: set MNEMO_TEST_MODEL_DIR to a directory with the model's files");
            None
        }
    }
}

/// A download base nothing listens on.
fn offline() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    format!("http://{addr}")
}

/// Serve `dir`'s files by name (the last segment of the path) until the
/// test ends; returns the base URL and a count of the requests served.
fn serve_model(dir: PathBuf) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let served = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = served.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut first = String::new();
            r.read_line(&mut first).unwrap();
            let mut line = String::new();
            while r.read_line(&mut line).unwrap() > 0 && line != "\r\n" {
                line.clear();
            }
            let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
            let name = path.rsplit('/').next().unwrap_or("");
            match std::fs::read(dir.join(name)) {
                Ok(body) if !name.is_empty() => {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let _ = write!(
                        s,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = s.write_all(&body);
                }
                _ => {
                    let _ = write!(
                        s,
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                }
            }
        }
    });
    (format!("http://{addr}"), served)
}

struct Env {
    dir: tempfile::TempDir,
    url: String,
}

impl Env {
    fn new(url: String) -> Env {
        Env {
            dir: tempdir().unwrap(),
            url,
        }
    }
    fn cache(&self) -> PathBuf {
        self.dir.path().join("cache")
    }
    fn installed_dir(&self) -> PathBuf {
        self.cache()
            .join("models")
            .join("snowflake-arctic-embed-xs")
            .join(builtin::REVISION)
    }
    fn brain(&self, dims: usize) -> PathBuf {
        let p = self.dir.path().join(format!("b{dims}.mnemo"));
        if !p.exists() {
            drop(
                Mnemo::create(
                    &p,
                    PASS,
                    MnemoConfig {
                        dimensions: dims,
                        kdf: KdfParams::fast(),
                        ..Default::default()
                    },
                )
                .unwrap(),
            );
        }
        p
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_mnemo"));
        c.args(args);
        for (k, _) in std::env::vars_os() {
            let k = k.to_string_lossy().to_string();
            if k.starts_with("MNEMO_") || k == "CLAUDECODE" || k == "CLAUDE_CODE_CHILD_SESSION" {
                c.env_remove(k);
            }
        }
        c.env("MNEMO_PASSPHRASE", PASS)
            .env("MNEMO_CACHE_DIR", self.cache())
            .env("MNEMO_MODEL_URL", &self.url);
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn ok(o: &Output) -> String {
    let t = text(o);
    assert!(o.status.success(), "{t}");
    t
}

fn fails(o: &Output) -> String {
    let t = text(o);
    assert!(!o.status.success(), "{t}");
    t
}

#[test]
fn offline_nothing_is_installed_and_writes_fail_clearly() {
    let env = Env::new(offline());
    let status = ok(&env.run(&["embedder", "status"]));
    assert!(status.contains("installed:  no (91.0 MB"), "{status}");
    assert!(status.contains(&builtin::identity().name), "{status}");

    let e = fails(&env.run(&["embedder", "install"]));
    assert!(
        e.contains("91.0 MB") && e.contains("download of config.json failed"),
        "{e}"
    );
    // Nothing half-written is left in the cache.
    let left: Vec<_> = std::fs::read_dir(env.installed_dir())
        .map(|d| d.flatten().collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");

    let b = env.brain(384);
    let p = b.to_str().unwrap();
    let e = fails(&env.run(&["remember", p, "--text", "a fact"]));
    assert!(e.contains("download of config.json failed"), "{e}");
    let db = Mnemo::open_read_only(&b, PASS).unwrap();
    assert_eq!(db.len(), 0);
}

#[test]
fn offline_recall_by_text_is_keyword_only() {
    let env = Env::new(offline());
    let b = env.brain(4);
    {
        let mut db = Mnemo::open(&b, PASS).unwrap();
        db.remember(mnemo_engine::Memory::new(
            String::from("the release workflow needs one approval"),
            mnemo_engine::MemoryType::Semantic,
            vec![1.0, 0.0, 0.0, 0.0],
        ))
        .unwrap();
        db.close().unwrap();
    }
    let p = b.to_str().unwrap();
    let o = env.run(&["recall", p, "--text", "release approval"]);
    let t = ok(&o);
    assert!(t.contains("recalling by keywords only"), "{t}");
    assert!(t.contains("this file records no embedder yet"), "{t}");
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("approval"),
        "{t}"
    );
    // No download was tried for a file the built-in didn't write.
    assert!(!t.contains("downloading"), "{t}");
    let e = fails(&env.run(&["recall", p, "--text", "release", "--no-keywords"]));
    assert!(
        e.contains("--no-keywords with --text needs an embedder"),
        "{e}"
    );
}

#[test]
fn offline_the_mcp_server_matches_text_by_keyword() {
    let env = Env::new(offline());
    let b = env.brain(4);
    {
        let mut db = Mnemo::open(&b, PASS).unwrap();
        db.remember(mnemo_engine::Memory::new(
            String::from("the release workflow needs one approval"),
            mnemo_engine::MemoryType::Semantic,
            vec![1.0, 0.0, 0.0, 0.0],
        ))
        .unwrap();
        db.close().unwrap();
    }
    let out = mcp(
        &env,
        &b,
        &[],
        &[(2, "recall", serde_json::json!({"text": "release approval"}))],
    );
    // A 4-wide file: the width notice (RB-010.1), so no download is tried.
    assert!(
        out.stderr.contains("matched by keyword only") && !out.stderr.contains("downloading"),
        "{}",
        out.stderr
    );
    let hits = out.results[0].as_array().unwrap();
    assert!(hits[0]["content"].as_str().unwrap().contains("approval"));
}

struct McpOut {
    results: Vec<serde_json::Value>,
    stderr: String,
    /// Whether `initialize` (id 1) was answered.
    initialized: bool,
    /// Whether the server exited 0.
    ok: bool,
}

fn mcp(
    env: &Env,
    brain: &Path,
    extra: &[&str],
    calls: &[(u64, &str, serde_json::Value)],
) -> McpOut {
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
    let mut args = vec!["serve", brain.to_str().unwrap(), "--mcp"];
    args.extend_from_slice(extra);
    let mut child = env
        .command(&args)
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
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let mut results = Vec::new();
    let mut initialized = false;
    for l in String::from_utf8_lossy(&out.stdout).lines() {
        let v: serde_json::Value = serde_json::from_str(l).unwrap();
        if v["id"].as_u64() == Some(1) && v.get("result").is_some() {
            initialized = true;
        }
        if v["id"].as_u64().unwrap_or(0) > 1 {
            assert!(v.get("error").is_none(), "{v}\n{stderr}");
            let text = v["result"]["content"][0]["text"].as_str().unwrap();
            results.push(serde_json::from_str(text).unwrap_or(serde_json::json!(text)));
        }
    }
    McpOut {
        results,
        stderr,
        initialized,
        ok: out.status.success(),
    }
}

#[test]
fn install_fetches_checks_and_then_serves_every_command() {
    let Some(model) = model_dir() else { return };
    let (url, served) = serve_model(model.clone());
    let env = Env::new(url);

    // Install: the size first, then the files, checked.
    let t = ok(&env.run(&["embedder", "install"]));
    assert!(t.contains("(91.0 MB)") && t.contains("installed"), "{t}");
    assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 3);
    for f in FILES {
        let got = std::fs::metadata(env.installed_dir().join(f.name)).unwrap();
        assert_eq!(got.len(), f.size);
    }
    let t = ok(&env.run(&["embedder", "install"]));
    assert!(t.contains("already installed"), "{t}");
    assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 3);
    assert!(ok(&env.run(&["embedder", "status"])).contains("installed:  yes"));

    // `init`'s default width is the built-in's; remember and recall need no
    // flags.
    let b = env.dir.path().join("fresh.mnemo");
    let p = b.to_str().unwrap();
    ok(&env.run(&["init", p, "--passphrase", PASS]));
    for fact in [
        "snapshot_list returns every snapshot, newest first",
        "The release workflow needs one approval click from the user",
        "Readers keep the intent lock while a writer commits",
    ] {
        ok(&env.run(&["remember", p, "--text", fact]));
    }
    let top = |q: &str| {
        let o = env.run(&["recall", p, "--text", q, "--top-k", "1"]);
        ok(&o);
        String::from_utf8_lossy(&o.stdout).to_string()
    };
    assert!(top("what does `snapshot_list` return").contains("snapshot_list returns"));
    assert!(top("who approves a release").contains("approval click"));

    // A changed file in the cache is refused, and `install` says so.
    let tok = env.installed_dir().join("tokenizer.json");
    let mut bytes = std::fs::read(&tok).unwrap();
    bytes[10] ^= 1;
    std::fs::write(&tok, &bytes).unwrap();
    let e = fails(&env.run(&["remember", p, "--text", "x"]));
    assert!(e.contains("doesn't match its pinned SHA-256"), "{e}");
    let e = fails(&env.run(&["embedder", "install"]));
    assert!(e.contains("doesn't match its pinned SHA-256"), "{e}");
    // Deleting it lets `install` fetch it again.
    std::fs::remove_file(&tok).unwrap();
    ok(&env.run(&["embedder", "install"]));
    assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 4);

    // The MCP server uses it with no flags: `remember` and `recall` by text.
    let out = mcp(
        &env,
        &b,
        &[],
        &[
            (
                2,
                "remember",
                serde_json::json!({
                    "content": "MnemoError::Replaced means the file was swapped",
                    "text": "MnemoError::Replaced means the file was swapped",
                    "memory_type": "semantic",
                }),
            ),
            (
                3,
                "recall",
                serde_json::json!({"text": "when is `MnemoError::Replaced` raised", "top_k": 1}),
            ),
        ],
    );
    assert!(
        out.stderr.contains("embedder ready (the built-in"),
        "{}",
        out.stderr
    );
    let hits = out.results[1].as_array().unwrap();
    assert!(hits[0]["content"]
        .as_str()
        .unwrap()
        .contains("MnemoError::Replaced"));
}

#[test]
fn model_dir_and_reembed() {
    let Some(model) = model_dir() else { return };
    let env = Env::new(offline());
    let md = model.to_str().unwrap();

    // `--model-dir` needs no cache and never downloads.
    let t = ok(&env.run(&["embedder", "status", "--model-dir", md]));
    assert!(
        t.contains("the pinned files (the built-in embedder)"),
        "{t}"
    );
    let e = fails(&env.run(&["embedder", "install", "--model-dir", md]));
    assert!(e.contains("nothing to install"), "{e}");

    // A 4-wide file with keyword-only facts moves to the built-in.
    let b = env.brain(4);
    {
        let mut db = Mnemo::open(&b, PASS).unwrap();
        db.remember(mnemo_engine::Memory::new(
            String::from("readers come first in v0.14"),
            mnemo_engine::MemoryType::Semantic,
            vec![1.0, 0.0, 0.0, 0.0],
        ))
        .unwrap();
        db.close().unwrap();
    }
    let p = b.to_str().unwrap();
    let e = fails(&env.run(&["remember", p, "--text", "x", "--model-dir", md]));
    assert!(
        e.contains("4-dimensional and the built-in embedder is 384"),
        "{e}"
    );
    let t = ok(&env.run(&["reembed", p, "--model-dir", md]));
    assert!(
        t.contains(&builtin::identity().name) && t.contains("384 dimensions"),
        "{t}"
    );
    let mut db = Mnemo::open_read_only(&b, PASS).unwrap();
    assert_eq!(db.dimensions(), 384);
    assert_eq!(db.embedder_identity().unwrap(), Some(builtin::identity()));
    drop(db);
    let o = env.run(&["recall", p, "--text", "who comes first", "--model-dir", md]);
    let t = ok(&o);
    assert!(!t.contains("keywords only"), "{t}");
    assert!(String::from_utf8_lossy(&o.stdout).contains("readers come first"));

    // Offline with no cache, a file the built-in wrote recalls by keyword,
    // after one try at the download.
    let o = env.run(&["recall", p, "--text", "readers first"]);
    let t = ok(&o);
    assert!(
        t.contains("downloading") && t.contains("keywords only"),
        "{t}"
    );
    assert!(String::from_utf8_lossy(&o.stdout).contains("readers come first"));
}

// --- RB-010.1: `serve --mcp` with the built-in never fails to start, and
// writes nothing at start to a file that has vectors from elsewhere.

/// The model's files put in the run's cache, as `install` leaves them.
fn put_model_in_cache(env: &Env, model: &Path) {
    std::fs::create_dir_all(env.installed_dir()).unwrap();
    for f in FILES {
        std::fs::copy(model.join(f.name), env.installed_dir().join(f.name)).unwrap();
    }
}

/// A brain of `dims` with the scaffold manifest and, if `with_vectors`, a
/// memory whose vector came from elsewhere. It records no embedder.
fn brain_without_embedder(env: &Env, name: &str, dims: usize, with_vectors: bool) -> PathBuf {
    let p = env.dir.path().join(name);
    let mut db = Mnemo::create(
        &p,
        PASS,
        MnemoConfig {
            dimensions: dims,
            kdf: KdfParams::fast(),
            ..Default::default()
        },
    )
    .unwrap();
    db.remember(mnemo_engine::Memory::scaffold_manifest(dims))
        .unwrap();
    if with_vectors {
        let mut v = vec![0.0; dims];
        v[0] = 1.0;
        db.remember(mnemo_engine::Memory::new(
            String::from("the release workflow needs one approval"),
            mnemo_engine::MemoryType::Semantic,
            v,
        ))
        .unwrap();
    }
    db.close().unwrap();
    p
}

fn sha(p: &Path) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(std::fs::read(p).unwrap()))
}

const MODES: [&[&str]; 2] = [&[], &["--shared"]];

#[test]
fn the_server_serves_a_file_of_another_width_without_the_builtin() {
    // Offline always, and with the model in the cache when there is one: the
    // model is neither fetched nor loaded for a 768-wide file.
    for with_model in [false, true] {
        let model = if with_model {
            match model_dir() {
                Some(m) => Some(m),
                None => continue,
            }
        } else {
            None
        };
        for mode in MODES {
            let env = Env::new(offline());
            if let Some(m) = &model {
                put_model_in_cache(&env, m);
            }
            let b = brain_without_embedder(&env, "w768.mnemo", 768, true);
            let out = mcp(
                &env,
                &b,
                mode,
                &[(2, "recall", serde_json::json!({"text": "release approval"}))],
            );
            let ctx = format!("model {with_model}, mode {mode:?}: {}", out.stderr);
            assert!(out.ok && out.initialized, "{ctx}");
            assert!(
                out.stderr
                    .contains("768-dimensional and the built-in embedder is 384"),
                "{ctx}"
            );
            assert!(out.stderr.contains("--embed-dims 384"), "{ctx}");
            assert!(!out.stderr.contains("downloading"), "{ctx}");
            let hits = out.results[0].as_array().unwrap();
            assert!(
                hits[0]["content"].as_str().unwrap().contains("approval"),
                "{ctx}"
            );
            let db = Mnemo::open_read_only(&b, PASS).unwrap();
            drop(db);
        }
    }
}

#[test]
fn the_server_writes_nothing_at_start_to_a_file_with_vectors_from_elsewhere() {
    for with_model in [false, true] {
        let model = if with_model {
            match model_dir() {
                Some(m) => Some(m),
                None => continue,
            }
        } else {
            None
        };
        for mode in MODES {
            let env = Env::new(offline());
            if let Some(m) = &model {
                put_model_in_cache(&env, m);
            }
            let b = brain_without_embedder(&env, "w384.mnemo", 384, true);
            let before = sha(&b);
            let out = mcp(&env, &b, mode, &[]);
            let ctx = format!("model {with_model}, mode {mode:?}: {}", out.stderr);
            assert!(out.ok && out.initialized, "{ctx}");
            assert_eq!(sha(&b), before, "the file changed at start; {ctx}");
            if with_model {
                assert!(out.stderr.contains("records no embedder"), "{ctx}");
            }
            assert!(!out.stderr.contains("embedder ready"), "{ctx}");
            // A text recall is keyword-only, and a text write is refused.
            let out = mcp(
                &env,
                &b,
                mode,
                &[(2, "recall", serde_json::json!({"text": "release approval"}))],
            );
            let hits = out.results[0].as_array().unwrap();
            assert!(
                hits[0]["content"].as_str().unwrap().contains("approval"),
                "{ctx}"
            );
            let mut db = Mnemo::open_read_only(&b, PASS).unwrap();
            assert_eq!(db.embedder_identity().unwrap(), None, "{ctx}");
        }
    }
}

#[test]
fn the_server_adopts_the_builtin_on_a_file_with_only_its_scaffold() {
    let Some(model) = model_dir() else { return };
    for mode in MODES {
        let env = Env::new(offline());
        put_model_in_cache(&env, &model);
        let b = brain_without_embedder(&env, "fresh.mnemo", 384, false);
        let out = mcp(&env, &b, mode, &[]);
        let ctx = format!("mode {mode:?}: {}", out.stderr);
        assert!(out.ok && out.initialized, "{ctx}");
        assert!(out.stderr.contains("embedder ready (the built-in"), "{ctx}");
        let mut db = Mnemo::open_read_only(&b, PASS).unwrap();
        assert_eq!(
            db.embedder_identity().unwrap(),
            Some(builtin::identity()),
            "{ctx}"
        );
    }
}
