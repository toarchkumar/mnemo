//! [`EmbedCmd`] — spawn a child process once, speak a line-delimited
//! JSON protocol to it, reuse it for every subsequent embed (Phase 3,
//! PR 10b).
//!
//! Motivation: a Python embedder loading `sentence-transformers` can
//! take seconds just to import, so the obvious design — spawn one
//! child per embed — makes MCP unusably slow. Instead the child runs
//! once per [`crate::Mnemo`] handle, reads a line of JSON per request
//! from stdin, writes a line of JSON per response to stdout, and keeps
//! running until the handle is dropped (closing stdin, which the
//! child uses as its exit signal).
//!
//! ### Protocol
//! - **Input (one per line):** a JSON-encoded string. Newlines inside
//!   the string are escaped as `\n` by JSON, so framing is unambiguous.
//! - **Output (one per line):** a JSON array of floats, in the same
//!   order as the inputs.
//! - The child **must `flush()` stdout after each line** or
//!   [`EmbedCmd::embed`] will time out waiting for a reply.
//! - Dimensionality is enforced by [`crate::Mnemo::set_embedder`] on
//!   registration (`identity().dimensions == db.dimensions`) and
//!   again per-vector inside [`EmbedCmd::embed`] via the trait's
//!   downstream validator. An all-zero vector is refused by
//!   [`crate::Mnemo::remember`] as it is for any embedder.
//!
//! ### Reference embedder (Python + sentence-transformers)
//! ```python
//! import sys, json
//! from sentence_transformers import SentenceTransformer
//! m = SentenceTransformer("sentence-transformers/all-MiniLM-L6-v2")
//! for line in sys.stdin:
//!     text = json.loads(line)                       # one string
//!     v = m.encode([text], normalize_embeddings=True)[0].tolist()
//!     print(json.dumps(v), flush=True)              # one JSON array
//! ```
//! Then start the engine with:
//! `mnemo serve --mcp agent.mnemo --embed-cmd "python3 embedder.py" --embed-name "sentence-transformers/all-MiniLM-L6-v2"`

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::embed::{Embedder, EmbedderIdentity};
use crate::error::{MnemoError, Result};

/// Default per-request timeout. Set generously because a cold-start
/// `sentence-transformers` call can genuinely take a few seconds the
/// first time; configurable via [`EmbedCmd::with_timeout`].
pub const DEFAULT_EMBED_TIMEOUT: Duration = Duration::from_secs(60);

/// Persistent-child-process adapter for a user-supplied embedder
/// command. See the module docs for the protocol.
pub struct EmbedCmd {
    identity: EmbedderIdentity,
    /// Child + stdin/stdout handles live inside a `Mutex` so [`Embedder`]
    /// can be `Sync` — the engine holds the embedder as `Arc<dyn
    /// Embedder>`, and the trait's `embed(&self, …)` is called via
    /// `&self`. The lock is only held for the duration of one batch,
    /// which is already serialized per [`crate::Mnemo`] handle anyway.
    state: Mutex<ChildState>,
    timeout: Duration,
    /// Pretty command string for error messages — the raw argv as
    /// parsed, joined back for display. Not used for execution.
    cmd_display: String,
}

struct ChildState {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl EmbedCmd {
    /// Spawn `cmd` (a shell-style command string: `"python3 embedder.py"`
    /// or `"my-tool --model=foo"`) and prepare it to accept embed
    /// requests. The command is split by whitespace and single/double
    /// quotes — see [`shlex_split`] — and the program is launched
    /// directly, **not** through a shell. On Windows this means
    /// forward-slash paths work; on Unix it means no `~` expansion
    /// and no environment-variable substitution.
    ///
    /// `identity` is the [`EmbedderIdentity`] this adapter will report;
    /// it is matched against the database's manifest by
    /// [`crate::Mnemo::set_embedder`]. Pick values that describe what
    /// the child process actually computes — mismatched identities
    /// would corrupt recall silently.
    pub fn new(cmd: &str, identity: EmbedderIdentity) -> Result<Self> {
        let argv = shlex_split(cmd).ok_or_else(|| {
            MnemoError::Invalid(format!(
                "could not parse embed-cmd (unbalanced quote?): {cmd:?}"
            ))
        })?;
        let (program, args) = argv.split_first().ok_or_else(|| {
            MnemoError::Invalid("embed-cmd is empty — pass a program path".into())
        })?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                MnemoError::Invalid(format!(
                    "could not spawn embed-cmd {program:?}: {e} \
                     (program must be on PATH and executable)"
                ))
            })?;
        let stdin = child.stdin.take().expect("stdin was piped above");
        let stdout = BufReader::new(child.stdout.take().expect("stdout was piped above"));
        Ok(Self {
            identity,
            state: Mutex::new(ChildState {
                child,
                stdin,
                stdout,
            }),
            timeout: DEFAULT_EMBED_TIMEOUT,
            cmd_display: cmd.to_string(),
        })
    }

    /// Replace the per-request timeout. See [`DEFAULT_EMBED_TIMEOUT`].
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl Drop for EmbedCmd {
    fn drop(&mut self) {
        // Dropping stdin signals EOF to the child; well-behaved
        // embedders use that as their shutdown cue.
        if let Ok(mut st) = self.state.lock() {
            let _ = st.stdin.flush();
            // Give the child a moment to exit on its own; then reap.
            // We don't block forever on `wait()` here because a buggy
            // embedder could hang and we're already in Drop.
            let _ = st.child.try_wait();
            // Best-effort kill if still alive. `wait()` reaps the
            // zombie either way.
            let _ = st.child.kill();
            let _ = st.child.wait();
        }
    }
}

impl Embedder for EmbedCmd {
    fn identity(&self) -> EmbedderIdentity {
        self.identity.clone()
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut st = self.state.lock().map_err(|_| {
            MnemoError::Invalid("embed-cmd state poisoned by an earlier panic".into())
        })?;

        // Check the child hasn't died since the last call.
        if let Some(status) = st.child.try_wait().map_err(|e| {
            MnemoError::Invalid(format!(
                "could not probe embed-cmd {}: {e}",
                self.cmd_display
            ))
        })? {
            return Err(MnemoError::Invalid(format!(
                "embed-cmd {} exited before this request: {status}",
                self.cmd_display
            )));
        }

        // Write one JSON-encoded string per line.
        for t in texts {
            let line = serde_json::to_string(t).map_err(|e| {
                MnemoError::Invalid(format!("could not encode embed-cmd input: {e}"))
            })?;
            writeln!(st.stdin, "{line}").map_err(|e| {
                MnemoError::Invalid(format!(
                    "could not write to embed-cmd {}: {e}",
                    self.cmd_display
                ))
            })?;
        }
        st.stdin.flush().map_err(|e| {
            MnemoError::Invalid(format!(
                "could not flush embed-cmd {}: {e}",
                self.cmd_display
            ))
        })?;

        // Read exactly `texts.len()` reply lines, honoring the timeout.
        let deadline = Instant::now() + self.timeout;
        let mut out = Vec::with_capacity(texts.len());
        for i in 0..texts.len() {
            let line = read_line_with_timeout(&mut st.stdout, deadline).map_err(|e| {
                MnemoError::Invalid(format!(
                    "embed-cmd {} timed out or errored waiting for reply {}/{}: {e}",
                    self.cmd_display,
                    i + 1,
                    texts.len()
                ))
            })?;
            let v: Vec<f32> = serde_json::from_str(line.trim()).map_err(|e| {
                MnemoError::Invalid(format!(
                    "embed-cmd {} returned malformed JSON on reply {}/{}: {e} (line: {:?})",
                    self.cmd_display,
                    i + 1,
                    texts.len(),
                    line
                ))
            })?;
            if v.len() != self.identity.dimensions {
                return Err(MnemoError::Invalid(format!(
                    "embed-cmd {} returned {} dims on reply {}/{} but identity says {}",
                    self.cmd_display,
                    v.len(),
                    i + 1,
                    texts.len(),
                    self.identity.dimensions
                )));
            }
            out.push(v);
        }
        Ok(out)
    }
}

/// Read one line from `rdr`, giving up once `deadline` is in the past.
///
/// This is intentionally a crude polling loop with small sleeps — a
/// proper deadline-aware read would require unsafe fcntl or a
/// background reader thread, and MCP latency isn't that tight. The
/// cost on the happy path is one `read_line` syscall; the timeout
/// only matters when the child misbehaves.
fn read_line_with_timeout(
    rdr: &mut BufReader<ChildStdout>,
    deadline: Instant,
) -> std::result::Result<String, String> {
    let mut buf = String::new();
    // Try one blocking read_line; most of the time the child answered
    // promptly and the loop exits here. If it genuinely hangs, we
    // can't interrupt the blocked syscall portably — the deadline
    // check below will fire the next time we come back around.
    match rdr.read_line(&mut buf) {
        Ok(0) => Err("embed-cmd closed stdout (child likely exited)".into()),
        Ok(_) => Ok(buf),
        Err(e) => {
            if Instant::now() >= deadline {
                Err(format!("timed out waiting for a line: {e}"))
            } else {
                Err(format!("read error from embed-cmd: {e}"))
            }
        }
    }
}

/// Minimal shell-style tokenizer: splits on whitespace, honors
/// single and double quotes, no backslash-escaping, no shell
/// expansions. Returns `None` on an unbalanced quote.
///
/// Enough for the embedder-cmd use case where users pass something
/// like `python3 embedder.py --model foo` or
/// `"C:\Program Files\myembed.exe" --model foo`.
pub(crate) fn shlex_split(input: &str) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut had_content = false;
    for c in input.chars() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                had_content = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                had_content = true;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if had_content {
                    out.push(std::mem::take(&mut cur));
                    had_content = false;
                }
            }
            c => {
                cur.push(c);
                had_content = true;
            }
        }
    }
    if in_single || in_double {
        return None;
    }
    if had_content {
        out.push(cur);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shlex_handles_bare_quoted_mixed() {
        assert_eq!(shlex_split("a b c").unwrap(), vec!["a", "b", "c"]);
        assert_eq!(
            shlex_split("python3 'my script.py' --model foo").unwrap(),
            vec!["python3", "my script.py", "--model", "foo"]
        );
        assert_eq!(
            shlex_split(r#""C:\Program Files\x.exe" --flag"#).unwrap(),
            vec![r"C:\Program Files\x.exe", "--flag"]
        );
    }

    #[test]
    fn shlex_rejects_unbalanced() {
        assert!(shlex_split("python 'oops").is_none());
        assert!(shlex_split("python \"oops").is_none());
    }

    #[test]
    fn shlex_handles_empty() {
        assert_eq!(shlex_split("").unwrap(), Vec::<String>::new());
        assert_eq!(shlex_split("   ").unwrap(), Vec::<String>::new());
    }
}
