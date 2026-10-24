//! [`EmbedCmd`] — spawn a child process once, speak a line-delimited
//! JSON protocol to it, reuse it for every subsequent embed (Phase 3,
//! PR 10b + 10b.2).
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
//! - The child **must `flush()` stdout after each line** or the next
//!   [`EmbedCmd::embed`] call will time out (default 60s, configurable
//!   via [`EmbedCmd::with_timeout`] and the `--embed-timeout` CLI
//!   flag).
//! - **Stdout is reserved for replies only.** Any line written to
//!   stdout outside a reply (startup banners, log messages, warnings)
//!   is treated as a protocol violation — EmbedCmd restarts the child
//!   and surfaces a clear error. Log to **stderr** instead; stderr is
//!   inherited to the parent and is safe to use.
//! - Dimensionality is enforced per-vector; wrong-width or malformed
//!   replies cause the child to be killed and respawned on the next
//!   call, so no mismatched vector is ever returned.
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
//! # Any logs: print("…", file=sys.stderr, flush=True)
//! ```
//! Then start the engine with:
//! `mnemo serve --mcp agent.mnemo --embed-cmd "python3 embedder.py" --embed-name "sentence-transformers/all-MiniLM-L6-v2"`
//!
//! ### Correctness guarantees (PR 10b.2)
//! - **Real timeouts.** A background thread owns the child's stdout
//!   and drains lines into a channel; [`EmbedCmd::embed`] reads via
//!   `recv_timeout`, so a silent-but-alive child trips the timeout
//!   deterministically. The reader thread also keeps stdout drained
//!   while we write stdin, which prevents the large-batch deadlock a
//!   write-all-then-read-all design would have.
//! - **Never a mismatched vector.** Any protocol error (write failure,
//!   timeout, EOF, malformed JSON, wrong width, zero vector, or
//!   unsolicited stdout output before a request) kills the child, drops
//!   the reader thread, and marks the adapter for restart. The next
//!   [`EmbedCmd::embed`] call spawns a fresh child + reader thread
//!   with the same argv/env/timeout before writing anything, and logs
//!   one line to stderr saying it restarted and why. The alternative
//!   — reading leftover output as a new reply — would silently
//!   corrupt recall quality, which is strictly worse than any error.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::embed::{Embedder, EmbedderIdentity};
use crate::error::{MnemoError, Result};

/// Default per-request timeout. Set generously because a cold-start
/// `sentence-transformers` call can genuinely take a few seconds the
/// first time; configurable via [`EmbedCmd::with_timeout`] (Rust API)
/// or the `--embed-timeout` CLI flag.
pub const DEFAULT_EMBED_TIMEOUT: Duration = Duration::from_secs(60);

/// What the reader thread sends over the channel. One `Line` per
/// stdout line the child wrote, then exactly one terminal `Eof` or
/// `Err(_)` and the thread exits.
enum ReaderMsg {
    Line(String),
    Eof,
    Err(String),
}

/// Persistent-child-process adapter for a user-supplied embedder
/// command. See the module docs for the protocol.
pub struct EmbedCmd {
    identity: EmbedderIdentity,
    /// Running-child state lives inside a `Mutex` so [`Embedder`] can
    /// be `Sync` (the engine holds the embedder as `Arc<dyn Embedder>`
    /// and the trait's `embed(&self, …)` is called via `&self`). The
    /// lock is only held for the duration of one batch, which the
    /// `&mut Mnemo` serialization already enforces at the call site.
    state: Mutex<Option<ChildState>>,
    timeout: Duration,
    /// Pretty command string for error messages — the raw argv as
    /// parsed, joined back for display. Not used for execution.
    cmd_display: String,
    /// Argv the next spawn will use. Captured from `cmd` at
    /// construction so a respawn after error reuses identical args.
    argv: Vec<String>,
}

/// All the handles belonging to one running child + its reader thread.
struct ChildState {
    child: Child,
    stdin: ChildStdin,
    /// Receives each stdout line (or the terminal `Eof`/`Err`) from
    /// the reader thread. Dropping the sender inside the thread lets
    /// `recv_timeout` detect EOF after the terminal message.
    stdout_rx: Receiver<ReaderMsg>,
    /// Reader thread handle, kept for `Drop`. Joining is best-effort —
    /// the thread exits once the child's stdout closes, which happens
    /// when we kill the child in [`EmbedCmd::teardown`].
    reader: Option<JoinHandle<()>>,
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
        if argv.is_empty() {
            return Err(MnemoError::Invalid(
                "embed-cmd is empty — pass a program path".into(),
            ));
        }
        let state = spawn_child(&argv)?;
        Ok(Self {
            identity,
            state: Mutex::new(Some(state)),
            timeout: DEFAULT_EMBED_TIMEOUT,
            cmd_display: cmd.to_string(),
            argv,
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
        if let Ok(mut slot) = self.state.lock() {
            if let Some(st) = slot.take() {
                teardown(st);
            }
        }
    }
}

/// Kill the child, best-effort reap, drop the stdin / receiver, and
/// join the reader thread. Used both on `Drop` and whenever a protocol
/// error marks the adapter for restart.
fn teardown(mut st: ChildState) {
    let _ = st.stdin.flush();
    // Dropping stdin by moving it out ends the child's own read loop
    // on EOF for well-behaved embedders.
    drop(st.stdin);
    // Give the child a brief moment to exit on its own.
    let _ = st.child.try_wait();
    // Then kill and reap. `wait()` is required to avoid a zombie.
    let _ = st.child.kill();
    let _ = st.child.wait();
    // Dropping the receiver unblocks the reader thread's send on the
    // next line (channel closed → send errors → thread exits). Joining
    // is best-effort: if the thread was blocked in `read_line` and the
    // child-killed stdout close didn't wake it fast enough, we don't
    // want to hold Drop indefinitely.
    drop(st.stdout_rx);
    if let Some(j) = st.reader.take() {
        // Discard the join result — any panic in the reader thread is
        // already behind us; we can't propagate it from Drop anyway.
        let _ = j.join();
    }
}

/// Spawn one child + reader thread. Returns a fresh [`ChildState`].
fn spawn_child(argv: &[String]) -> Result<ChildState> {
    let (program, args) = argv.split_first().expect("argv non-empty by caller");
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
    let (tx, rx) = mpsc::channel::<ReaderMsg>();
    let reader = std::thread::Builder::new()
        .name("mnemo-embed-cmd-reader".into())
        .spawn(move || {
            let mut rdr = stdout;
            let mut buf = String::new();
            loop {
                buf.clear();
                match rdr.read_line(&mut buf) {
                    Ok(0) => {
                        let _ = tx.send(ReaderMsg::Eof);
                        break;
                    }
                    Ok(_) => {
                        if tx.send(ReaderMsg::Line(buf.clone())).is_err() {
                            // Receiver dropped — nothing to do but exit.
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(ReaderMsg::Err(e.to_string()));
                        break;
                    }
                }
            }
        })
        .map_err(|e| {
            MnemoError::Invalid(format!("could not start embed-cmd reader thread: {e}"))
        })?;
    Ok(ChildState {
        child,
        stdin,
        stdout_rx: rx,
        reader: Some(reader),
    })
}

impl EmbedCmd {
    /// Ensure a running child is available, respawning if necessary.
    /// On respawn, logs one stderr line explaining why.
    fn ensure_child<'a>(
        &self,
        slot: &'a mut Option<ChildState>,
        reason_if_respawn: &str,
    ) -> Result<&'a mut ChildState> {
        if slot.is_none() {
            eprintln!(
                "mnemo embed-cmd: restarting {} ({})",
                self.cmd_display, reason_if_respawn
            );
            *slot = Some(spawn_child(&self.argv)?);
        }
        Ok(slot.as_mut().expect("just filled"))
    }

    /// Fail the current request with `msg`, kill the child, mark for
    /// restart. The next `embed()` call spawns a fresh child.
    fn fail_and_restart(&self, slot: &mut Option<ChildState>, msg: String) -> MnemoError {
        if let Some(st) = slot.take() {
            teardown(st);
        }
        MnemoError::Invalid(msg)
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
        let mut slot = self.state.lock().map_err(|_| {
            MnemoError::Invalid("embed-cmd state poisoned by an earlier panic".into())
        })?;

        // Respawn if the previous call tore the child down.
        self.ensure_child(&mut slot, "previous request errored")?;

        // Before writing, probe the channel. Any message waiting here
        // is unsolicited (the previous call either read all its replies
        // or restarted); treat it as a protocol violation. One probe
        // is enough — the first stale message already means the pipe
        // state is wrong, and we're going to kill + respawn anyway.
        {
            let st = slot.as_mut().expect("ensure_child above");
            match st.stdout_rx.try_recv() {
                Ok(ReaderMsg::Line(line)) => {
                    return Err(self.fail_and_restart(
                        &mut slot,
                        format!(
                            "embed-cmd {} wrote to stdout outside a reply (line: {:?}). \
                             Stdout is reserved for replies only; log to stderr instead. \
                             The child has been killed; the next call will respawn.",
                            self.cmd_display,
                            line.trim_end()
                        ),
                    ));
                }
                Ok(ReaderMsg::Eof) => {
                    return Err(self.fail_and_restart(
                        &mut slot,
                        format!(
                            "embed-cmd {} closed stdout before this request. \
                             The next call will respawn.",
                            self.cmd_display
                        ),
                    ));
                }
                Ok(ReaderMsg::Err(e)) => {
                    return Err(self.fail_and_restart(
                        &mut slot,
                        format!(
                            "embed-cmd {} stdout read error before this request: {e}. \
                             The next call will respawn.",
                            self.cmd_display
                        ),
                    ));
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    // Reader thread gone; respawn next call.
                    return Err(self.fail_and_restart(
                        &mut slot,
                        format!(
                            "embed-cmd {} reader thread died. The next call will respawn.",
                            self.cmd_display
                        ),
                    ));
                }
            }
        }

        // Also probe for a dead child (OS-level reap).
        {
            let st = slot.as_mut().expect("ensure_child above");
            if let Some(status) = st.child.try_wait().map_err(|e| {
                MnemoError::Invalid(format!(
                    "could not probe embed-cmd {}: {e}",
                    self.cmd_display
                ))
            })? {
                return Err(self.fail_and_restart(
                    &mut slot,
                    format!(
                        "embed-cmd {} exited before this request: {status}. \
                         The next call will respawn.",
                        self.cmd_display
                    ),
                ));
            }
        }

        // Write all inputs. The reader thread drains stdout in parallel
        // so we don't deadlock on a large batch where the child's
        // stdout fills before our stdin writes finish.
        {
            let st = slot.as_mut().expect("ensure_child above");
            for t in texts {
                let line = serde_json::to_string(t).map_err(|e| {
                    MnemoError::Invalid(format!("could not encode embed-cmd input: {e}"))
                })?;
                if let Err(e) = writeln!(st.stdin, "{line}") {
                    return Err(self.fail_and_restart(
                        &mut slot,
                        format!(
                            "could not write to embed-cmd {}: {e}. The next call will respawn.",
                            self.cmd_display
                        ),
                    ));
                }
            }
            if let Err(e) = st.stdin.flush() {
                return Err(self.fail_and_restart(
                    &mut slot,
                    format!(
                        "could not flush embed-cmd {}: {e}. The next call will respawn.",
                        self.cmd_display
                    ),
                ));
            }
        }

        // Read exactly `texts.len()` reply lines, honoring the timeout.
        // Any failure (timeout, EOF mid-batch, malformed JSON, wrong
        // width, zero vector) kills the child so stale leftover output
        // can't be mis-attributed to a later call.
        let deadline = Instant::now() + self.timeout;
        let mut out = Vec::with_capacity(texts.len());
        for i in 0..texts.len() {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or(Duration::ZERO);
            let msg = {
                let st = slot.as_mut().expect("ensure_child above");
                st.stdout_rx.recv_timeout(remaining)
            };
            let line = match msg {
                Ok(ReaderMsg::Line(l)) => l,
                Ok(ReaderMsg::Eof) => {
                    return Err(self.fail_and_restart(
                        &mut slot,
                        format!(
                            "embed-cmd {} closed stdout while waiting for reply {}/{}. \
                             The next call will respawn.",
                            self.cmd_display,
                            i + 1,
                            texts.len()
                        ),
                    ));
                }
                Ok(ReaderMsg::Err(e)) => {
                    return Err(self.fail_and_restart(
                        &mut slot,
                        format!(
                            "embed-cmd {} stdout read error on reply {}/{}: {e}. \
                             The next call will respawn.",
                            self.cmd_display,
                            i + 1,
                            texts.len()
                        ),
                    ));
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(self.fail_and_restart(
                        &mut slot,
                        format!(
                            "embed-cmd {} timed out after {:?} waiting for reply {}/{}. \
                             The next call will respawn.",
                            self.cmd_display,
                            self.timeout,
                            i + 1,
                            texts.len()
                        ),
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(self.fail_and_restart(
                        &mut slot,
                        format!(
                            "embed-cmd {} reader thread exited mid-request. \
                             The next call will respawn.",
                            self.cmd_display
                        ),
                    ));
                }
            };
            let v: Vec<f32> = match serde_json::from_str(line.trim()) {
                Ok(v) => v,
                Err(e) => {
                    return Err(self.fail_and_restart(
                        &mut slot,
                        format!(
                            "embed-cmd {} returned malformed JSON on reply {}/{}: {e} (line: {:?}). \
                             The next call will respawn.",
                            self.cmd_display,
                            i + 1,
                            texts.len(),
                            line.trim_end()
                        ),
                    ));
                }
            };
            if v.len() != self.identity.dimensions {
                return Err(self.fail_and_restart(
                    &mut slot,
                    format!(
                        "embed-cmd {} returned {} dims on reply {}/{} but identity says {}. \
                         The next call will respawn.",
                        self.cmd_display,
                        v.len(),
                        i + 1,
                        texts.len(),
                        self.identity.dimensions
                    ),
                ));
            }
            out.push(v);
        }
        Ok(out)
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
