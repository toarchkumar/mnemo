//! GUIDE-1 (12.2 step 2): the guide is evergreen because it's tested
//! (`docs/design/12.2-onboard.md`, section 6). Five checks:
//! 1. every ` ```sh test ` block runs, in order, against this build's
//!    `mnemo` (Unix, under bash);
//! 2. each exits 0, unless it carries `expect-fail`;
//! 3. every ` ```json mcp ` block, sent to `mnemo serve --mcp --shared`,
//!    gets a result, not an error;
//! 4. every `mnemo …` command in the guide, in prose or in any shell block,
//!    parses with the CLI (every platform);
//! 5. the generated files (the repository's `llms.txt` and `llms-full.txt`)
//!    equal what the binary generates.
//!
//! The blocks run in a temporary git repository with a temporary `HOME`,
//! the test key store (`MNEMO_DEV_KEYSTORE`, never the OS keychain), a cache
//! of its own, and a model URL nothing listens on. Blocks tagged `model`
//! run only with the `embed-candle` feature and `MNEMO_TEST_MODEL_DIR` (CI's
//! `builtin embedder (linux)` job).

use std::path::Path;
use std::process::{Command, Stdio};

use mnemo_engine::guide;

/// Commands the guide describes that later steps of v0.16.0 build. A block
/// or a command containing the text is skipped (with the stand-in run in its
/// place, if there is one), and each step removes its entries. **At READY
/// this list must be empty.**
const PENDING_UNTIL_STEP: &[(&str, u8, Option<&str>)] = &[
    // Step 5: `onboard` itself; until then the brain comes from `init`.
    ("mnemo onboard --yes", 5, Some("mnemo init")),
    ("mnemo onboard --undo", 5, None),
    // Step 5: the plugin's skill, generated from the guide (check 5).
    ("integrations/claude-code", 5, None),
];

fn pending(text: &str) -> Option<&'static (&'static str, u8, Option<&'static str>)> {
    PENDING_UNTIL_STEP.iter().find(|(n, _, _)| text.contains(n))
}

/// The `mnemo …` commands in the guide: inline code spans outside fences,
/// and lines of shell blocks (after a `| `), with their source line.
fn commands() -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut in_block = false;
    let mut shell = false;
    for (i, line) in guide::SOURCE.lines().enumerate() {
        if let Some(info) = line.strip_prefix("```") {
            if in_block {
                in_block = false;
            } else {
                in_block = true;
                shell = info.split_whitespace().next() == Some("sh");
            }
            continue;
        }
        if in_block {
            if shell {
                let cmd = line.rsplit("| ").next().unwrap_or(line).trim();
                if cmd.starts_with("mnemo ") {
                    out.push((i + 1, cmd.to_string()));
                }
            }
            continue;
        }
        let mut rest = line;
        while let Some(a) = rest.find('`') {
            let after = &rest[a + 1..];
            let Some(b) = after.find('`') else { break };
            let span = &after[..b];
            if span.starts_with("mnemo ") || span == "mnemo" {
                out.push((i + 1, span.to_string()));
            }
            rest = &after[b + 1..];
        }
    }
    out
}

/// Split a command line on spaces, keeping quoted parts whole.
fn split(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in cmd.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                any = true;
            }
            None if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            None => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[test]
fn the_card_fits_and_says_brain_content_is_data() {
    let c = guide::card();
    assert!(
        c.len() <= guide::CARD_MAX,
        "the card is {} bytes, more than {}",
        c.len(),
        guide::CARD_MAX
    );
    assert!(
        c.contains("not instructions") && c.contains("never") && c.contains("your user"),
        "{c}"
    );
    // No URL is fetched at run time: the card names none.
    assert!(!c.contains("http"), "{c}");
}

/// Check 4: every `mnemo …` command parses. `--help` at the end makes clap
/// check the subcommand and every flag and value before it, and skip the
/// required ones.
#[test]
fn every_command_in_the_guide_parses() {
    let cmds = commands();
    assert!(cmds.len() > 10, "{cmds:?}");
    let mut checked = 0;
    for (line, cmd) in &cmds {
        if pending(cmd).is_some() {
            continue;
        }
        let args = split(cmd);
        let o = Command::new(env!("CARGO_BIN_EXE_mnemo"))
            .args(&args[1..])
            .arg("--help")
            .env_remove("CLAUDE_PROJECT_DIR")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "guide.md:{line}: `{cmd}` doesn't parse: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        checked += 1;
    }
    eprintln!("{checked} commands parsed");
}

/// Check 5: the repository's `llms.txt` is what the binary generates, and
/// so is `onboard --guide`'s output.
#[test]
fn the_generated_files_match() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for (file, format, want) in [
        ("llms.txt", "llms-txt", guide::llms_txt()),
        ("llms-full.txt", "llms-full", guide::llms_full()),
    ] {
        // A Windows checkout may end its lines in `\r\n`.
        let have = std::fs::read_to_string(root.join(file))
            .unwrap_or_else(|e| panic!("{file} at the repository root: {e}"))
            .replace("\r\n", "\n");
        assert!(
            have == want,
            "{file} is stale: run `mnemo onboard --guide --format {format} > {file}` from the \
             repository root"
        );
    }
    for (format, want) in [
        ("markdown", guide::markdown()),
        ("llms-txt", guide::llms_txt()),
        ("llms-full", guide::llms_full()),
    ] {
        let o = Command::new(env!("CARGO_BIN_EXE_mnemo"))
            .args(["onboard", "--guide", "--format", format])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(o.status.success());
        assert_eq!(String::from_utf8(o.stdout).unwrap(), want, "{format}");
    }
    // Until step 5, `onboard` alone says it's being built, and exits 2.
    let o = Command::new(env!("CARGO_BIN_EXE_mnemo"))
        .arg("onboard")
        .env_remove("CLAUDE_PROJECT_DIR")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("onboard --guide"));
}

/// Every pending entry still matches something in the guide (or names the
/// plugin check), so a stale entry can't hide.
#[test]
fn every_pending_entry_is_still_needed() {
    for (needle, step, _) in PENDING_UNTIL_STEP {
        assert!((3..=8).contains(step), "{needle}: step {step}");
        if *needle == "integrations/claude-code" {
            continue;
        }
        assert!(
            guide::SOURCE.contains(needle),
            "`{needle}` (step {step}) is on the pending list but not in the guide"
        );
    }
    eprintln!(
        "pending until later steps: {:?}",
        PENDING_UNTIL_STEP
            .iter()
            .map(|(n, s, _)| format!("{n} (step {s})"))
            .collect::<Vec<_>>()
    );
}

// --- Checks 1 to 3: running the blocks (Unix) --------------------------------

#[cfg(unix)]
mod run {
    use super::{guide, pending};
    use std::io::{BufRead, BufReader, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

    use mnemo_engine::guide::Block;
    use serde_json::{json, Value};

    /// The model's files, when this build has the built-in embedder and CI gave
    /// them.
    fn model_dir() -> Option<PathBuf> {
        if !cfg!(feature = "embed-candle") {
            return None;
        }
        std::env::var_os("MNEMO_TEST_MODEL_DIR")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
    }

    /// The directory holding `name` on this process's PATH.
    fn tool_dir(name: &str) -> PathBuf {
        let path = std::env::var_os("PATH").expect("PATH");
        std::env::split_paths(&path)
            .find(|d| d.join(name).is_file())
            .unwrap_or_else(|| panic!("{name} on the PATH"))
    }

    struct Sandbox {
        dir: tempfile::TempDir,
        model: bool,
        server: Option<(Child, ChildStdin, BufReader<ChildStdout>)>,
        next_id: u64,
    }

    impl Sandbox {
        fn new() -> Sandbox {
            let dir = tempfile::tempdir().unwrap();
            for d in ["home", "store", "cache", "repo"] {
                std::fs::create_dir_all(dir.path().join(d)).unwrap();
            }
            let mut s = Sandbox {
                dir,
                model: false,
                server: None,
                next_id: 1,
            };
            if let Some(m) = model_dir() {
                s.install_model(&m);
                s.model = true;
            }
            let repo = s.p("repo");
            let git = Command::new("git")
                .args(["init", "-q"])
                .current_dir(&repo)
                .env("HOME", s.p("home"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(git.status.success());
            s
        }
        fn p(&self, rel: &str) -> PathBuf {
            self.dir.path().join(rel)
        }
        /// The model, linked into this sandbox's cache where the built-in
        /// embedder looks for it.
        #[allow(unused_variables)]
        fn install_model(&self, from: &Path) {
            #[cfg(feature = "embed-candle")]
            {
                let to = self
                    .p("cache")
                    .join("models")
                    .join("snowflake-arctic-embed-xs")
                    .join(mnemo_engine::builtin::REVISION);
                std::fs::create_dir_all(&to).unwrap();
                for f in mnemo_engine::builtin::FILES {
                    std::os::unix::fs::symlink(from.join(f.name), to.join(f.name)).unwrap();
                }
            }
        }
        fn env(&self, c: &mut Command) {
            for (k, _) in std::env::vars_os() {
                let k = k.to_string_lossy().to_string();
                if k.starts_with("MNEMO_")
                    || k.starts_with("CLAUDE")
                    || k.starts_with("GIT_")
                    || k == "XDG_CONFIG_HOME"
                {
                    c.env_remove(k);
                }
            }
            // A model URL nothing listens on: nothing here downloads.
            let closed = {
                let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                format!("http://{}", l.local_addr().unwrap())
            };
            let bin = Path::new(env!("CARGO_BIN_EXE_mnemo")).parent().unwrap();
            let path = std::env::join_paths([bin.to_path_buf(), tool_dir("git"), tool_dir("bash")])
                .unwrap();
            c.env("HOME", self.p("home"))
                .env("PATH", path)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("MNEMO_DEV_KEYSTORE", self.p("store"))
                .env("MNEMO_CACHE_DIR", self.p("cache"))
                .env("MNEMO_MODEL_URL", closed)
                .current_dir(self.p("repo"));
        }
        fn sh(&self, b: &Block, body: &str) {
            let mut c = Command::new("bash");
            c.args(["-euo", "pipefail", "-c", body]);
            self.env(&mut c);
            let o = c.stdin(Stdio::null()).output().unwrap();
            let ok = o.status.success() != b.has("expect-fail");
            assert!(
                ok,
                "guide.md:{}: the block {} ({:?}):\n{body}\nstdout: {}\nstderr: {}",
                b.line,
                if b.has("expect-fail") {
                    "should fail but passed"
                } else {
                    "failed"
                },
                o.status,
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
        }
        fn request(&mut self, method: &str, params: Value) -> Value {
            if self.server.is_none() {
                let mut c = Command::new(env!("CARGO_BIN_EXE_mnemo"));
                c.args(["serve", "--mcp", "--shared"]);
                self.env(&mut c);
                let mut child = c
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .unwrap();
                let stdin = child.stdin.take().unwrap();
                let stdout = BufReader::new(child.stdout.take().unwrap());
                self.server = Some((child, stdin, stdout));
                let init = self.request(
                    "initialize",
                    json!({"protocolVersion": "2025-06-18", "capabilities": {},
                       "clientInfo": {"name": "guide-test", "version": "0"}}),
                );
                assert!(init.get("result").is_some(), "{init}");
            }
            let id = self.next_id;
            self.next_id += 1;
            let (_, stdin, stdout) = self.server.as_mut().unwrap();
            let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
            writeln!(stdin, "{req}").unwrap();
            stdin.flush().unwrap();
            let mut line = String::new();
            let n = stdout.read_line(&mut line).unwrap();
            assert!(n > 0, "the server ended before answering {req}");
            serde_json::from_str(&line).unwrap()
        }
        fn mcp(&mut self, b: &Block) {
            let v: Value = serde_json::from_str(&b.body)
                .unwrap_or_else(|e| panic!("guide.md:{}: not JSON: {e}", b.line));
            let (method, params) = match v.get("method") {
                Some(m) => (
                    m.as_str().unwrap().to_string(),
                    v.get("params").cloned().unwrap_or(json!({})),
                ),
                None => ("tools/call".to_string(), v.clone()),
            };
            let r = self.request(&method, params);
            assert!(
                r.get("error").is_none() && r.get("result").is_some(),
                "guide.md:{}: {method} failed: {r}",
                b.line
            );
            assert_ne!(
                r["result"]["isError"],
                json!(true),
                "guide.md:{}: {r}",
                b.line
            );
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            if let Some((mut child, stdin, _)) = self.server.take() {
                drop(stdin);
                let _ = child.wait();
            }
        }
    }

    /// Checks 1 to 3: every tested block, in the guide's order.
    #[test]
    fn every_block_in_the_guide_runs() {
        let mut s = Sandbox::new();
        let (mut ran, mut skipped) = (0, Vec::new());
        for b in guide::blocks() {
            let tested = (b.lang == "sh" && b.has("test")) || (b.lang == "json" && b.has("mcp"));
            if !tested {
                continue;
            }
            if b.has("model") && !s.model {
                skipped.push(format!("guide.md:{} (needs the model)", b.line));
                continue;
            }
            if let Some((needle, step, stand_in)) = pending(&b.body) {
                skipped.push(format!("guide.md:{} (`{needle}`, step {step})", b.line));
                if let Some(cmd) = stand_in {
                    s.sh(&b, cmd);
                }
                continue;
            }
            if b.lang == "sh" {
                s.sh(&b, &b.body);
            } else {
                s.mcp(&b);
            }
            ran += 1;
        }
        eprintln!("{ran} blocks ran; skipped: {skipped:?}");
        assert!(ran >= 6, "only {ran} blocks ran");
        // The brain the blocks used is the project brain, made in the key store.
        assert!(s.p("repo/.git/mnemo/project.mnemo").is_file());
        assert!(std::fs::read_dir(s.p("store")).unwrap().next().is_some());
    }
}
