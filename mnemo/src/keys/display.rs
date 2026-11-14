//! 12.3: the display rule (the keys note, section 7). A recovery code is
//! shown only to a person at a terminal: stdout and stdin are terminals, no
//! known agent-harness marker is set, and the controlling terminal opens.
//! It's shown through the controlling terminal, after a key press, on the
//! alternate screen, so nothing reaches stdout or stays in scrollback.

use std::io::{IsTerminal, Write};

/// Environment variables that agent harnesses set in the commands they
/// run. Each is checked against its vendor's documentation, cited here. A
/// marker only makes the rule stricter; only whether it's set (and not
/// empty) is read. **A best effort**: it can only name harnesses someone
/// has checked, so the key press is the guard that doesn't depend on it.
///
/// - `CLAUDECODE`: "Set to `1` in subprocesses Claude Code spawns (Bash
///   and PowerShell tools, tmux sessions, hook commands, status line
///   commands, stdio MCP server subprocesses). IDE extensions also set this
///   in their integrated terminals."
///   <https://code.claude.com/docs/en/env-vars>
/// - `CLAUDE_CODE_CHILD_SESSION`: "Set to `1` in subprocesses Claude Code
///   spawns via the Bash, PowerShell, and Monitor tools, hook commands, and
///   status line commands." Same page.
///
/// A candidate not yet on the list: `AI_AGENT`, a proposed cross-tool
/// convention; no vendor's documentation found yet.
pub const AGENT_MARKERS: &[&str] = &["CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION"];

/// What the user is told when a code isn't shown.
pub const REFUSAL: &str = "the recovery code wasn't shown, because this doesn't look like a \
terminal a person is at. Run `mnemo recovery-code <file>` in a terminal of your own, not \
through an agent's shell or an editor terminal that one runs in (for example, Claude Code's `!`)";

/// Why the rule doesn't hold here, or `None` when it does.
pub fn refusal_reason() -> Option<String> {
    if !std::io::stdout().is_terminal() {
        return Some("stdout isn't a terminal".into());
    }
    if !std::io::stdin().is_terminal() {
        return Some("stdin isn't a terminal".into());
    }
    marker_reason(|k| std::env::var_os(k))
        .or_else(|| (!controlling_terminal_opens()).then(|| "no controlling terminal".into()))
}

/// The marker part of the rule, over an environment lookup (for tests).
pub fn marker_reason(get: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<String> {
    AGENT_MARKERS
        .iter()
        .find(|m| get(m).is_some_and(|v| !v.is_empty()))
        .map(|m| format!("{m} is set (an agent harness)"))
}

#[cfg(unix)]
fn controlling_terminal_opens() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .is_ok()
}

#[cfg(windows)]
fn controlling_terminal_opens() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")
        .is_ok()
}

#[cfg(not(any(unix, windows)))]
fn controlling_terminal_opens() -> bool {
    false
}

/// Show `code` under the rule: ask for Enter first, then show it on the
/// alternate screen, wait for Enter again, and leave it. Call only when
/// [`refusal_reason`] is `None`. The answers are read without echo.
pub fn show(code: &str) -> std::io::Result<()> {
    rpassword::prompt_password(
        "Press Enter to show the recovery code. Make sure this is your own terminal. ",
    )?;
    let mut tty = open_tty()?;
    #[cfg(unix)]
    write!(tty, "\x1b[?1049h\x1b[H\x1b[2J")?;
    write!(
        tty,
        "Your recovery code:\r\n\r\n    {code}\r\n\r\n\
         Write it down and keep it somewhere safe. It's the only way to open this brain \r\n\
         on another machine, or here if the keychain entry is lost.\r\n\r\n"
    )?;
    #[cfg(not(unix))]
    write!(
        tty,
        "Clear the screen (cls) once you've written it down.\r\n\r\n"
    )?;
    tty.flush()?;
    drop(tty);
    let r = rpassword::prompt_password("Press Enter when you've written it down. ");
    let mut tty = open_tty()?;
    #[cfg(unix)]
    write!(tty, "\x1b[?1049l")?;
    tty.flush()?;
    r.map(|_| ())
}

/// Ask on the controlling terminal for `prompt` and read the answer
/// without echo (`onboard`'s confirmation of the last group).
pub fn ask_hidden(prompt: &str) -> std::io::Result<String> {
    rpassword::prompt_password(prompt)
}

#[cfg(unix)]
fn open_tty() -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new().write(true).open("/dev/tty")
}

#[cfg(windows)]
fn open_tty() -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new().write(true).open("CONOUT$")
}

#[cfg(not(any(unix, windows)))]
fn open_tty() -> std::io::Result<std::fs::File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no controlling terminal",
    ))
}
