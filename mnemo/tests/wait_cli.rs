//! v0.20.1 (spec.v0201-wait, RB-017.6): `mnemo wait`, one wake signal from
//! the brain for every editor. Each test runs the CLI with a 50 ms tick
//! (the dev-hooks switch `MNEMO_DEV_WAIT_TICK_MS`), so the 15 s re-check
//! while held is 750 ms and the backoff 100 ms to 1.5 s; the bounds below
//! are generous, and the tests assert the order of events, not their
//! timing.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use mnemo_engine::{Memory, MemoryType, Mnemo, WAKE_PER_HOUR};

const TICK_MS: &str = "50";
/// Long enough for any wait here, on a loaded runner.
const BOUND: Duration = Duration::from_secs(60);
/// How long a waiter that shouldn't wake is watched.
const QUIET: Duration = Duration::from_millis(1500);

struct W {
    dir: tempfile::TempDir,
    /// The brain's path, as an argument.
    b: String,
}

impl W {
    /// A 4-wide brain with a raw key in a key file.
    fn new() -> W {
        let dir = tempfile::tempdir().unwrap();
        let b = dir.path().join("b.mnemo").to_str().unwrap().to_string();
        let w = W { dir, b };
        std::fs::create_dir_all(w.p("home")).unwrap();
        w.ok(&[
            "init",
            w.brain().to_str().unwrap(),
            "--dimensions",
            "4",
            "--key-file",
            w.key().to_str().unwrap(),
        ]);
        w
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
    fn brain(&self) -> PathBuf {
        self.p("b.mnemo")
    }
    fn b(&self) -> &str {
        &self.b
    }
    fn key(&self) -> PathBuf {
        self.p("b.key")
    }
    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_mnemo"));
        c.args(args);
        for (k, _) in std::env::vars_os() {
            let k = k.to_string_lossy().to_string();
            if k.starts_with("MNEMO_") || k.starts_with("CLAUDE") {
                c.env_remove(k);
            }
        }
        c.env("HOME", self.p("home"))
            .env("USERPROFILE", self.p("home"))
            .env("MNEMO_DEV_NO_KEYCHAIN", "1")
            .env("MNEMO_DEV_WAIT_TICK_MS", TICK_MS)
            .current_dir(self.dir.path())
            .stdin(Stdio::null());
        if self.key().exists() {
            c.env("MNEMO_KEY_FILE", self.key());
        }
        c
    }
    fn ok(&self, args: &[&str]) -> String {
        let o = self.cmd(args).output().unwrap();
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).into_owned()
    }
    /// `post` from `from` to `to`; returns the message id.
    fn post(&self, from: &str, to: &str, subject: &str, body: &str) -> String {
        let out = self.ok(&[
            "post",
            "--brain",
            self.b(),
            to,
            subject,
            "--body",
            body,
            "--as",
            from,
        ]);
        out.split_whitespace().next().unwrap().to_string()
    }
    /// A `mnemo wait` for `agent` (role `worker`), in the background.
    fn wait(&self, agent: &str, extra: &[&str], envs: &[(&str, &str)]) -> Child {
        let mut args = vec!["wait", self.b(), "--as", agent, "--role", "worker"];
        args.extend(extra);
        let mut c = self.cmd(&args);
        for (k, v) in envs {
            c.env(k, v);
        }
        c.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }
    /// The brain opened in this process, with its key.
    fn open(&self) -> mnemo_engine::Result<Mnemo> {
        let k = mnemo_engine::keys::read_key_file(self.key().to_str().unwrap()).unwrap();
        Mnemo::open_with_key(self.brain(), &k)
    }
}

/// The waiter's output once it exits within `within`; `None` if it's still
/// running then.
fn exited(child: &mut Child, within: Duration) -> Option<Output> {
    let until = Instant::now() + within;
    while Instant::now() < until {
        if child.try_wait().unwrap().is_some() {
            return Some(collect(child));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

fn collect(child: &mut Child) -> Output {
    let status = child.wait().unwrap();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut stderr)
        .unwrap();
    Output {
        status,
        stdout,
        stderr,
    }
}

/// It exits 0 within the bound; returns its stdout.
fn woken(child: &mut Child) -> String {
    let o = exited(child, BOUND).expect("the waiter wasn't woken");
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// It's still waiting after [`QUIET`].
fn still_waiting(child: &mut Child) {
    if let Some(o) = exited(child, QUIET) {
        panic!(
            "the waiter exited ({:?}): {}{}",
            o.status.code(),
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

fn stop(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// The delivery state of every message to `agent`.
fn deliveries(w: &W, agent: &str) -> Vec<String> {
    let k = mnemo_engine::keys::read_key_file(w.key().to_str().unwrap()).unwrap();
    let mut db = Mnemo::open_read_only_waiting_with_key(
        w.brain(),
        &k,
        mnemo_engine::SharedOptions::default(),
    )
    .unwrap();
    db.inbox(Some(agent), None, true)
        .unwrap()
        .into_iter()
        .map(|m| m.delivery)
        .collect()
}

/// Wait until a file exists.
fn until_exists(p: &Path) {
    let until = Instant::now() + BOUND;
    while !p.exists() {
        assert!(Instant::now() < until, "{} never appeared", p.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn an_item_sent_before_it_starts_wakes_it_at_once_by_id_and_sender_only() {
    let w = W::new();
    let id = w.post("planner-a", "worker-a", "the subject", "the body");
    let out = woken(&mut w.wait("worker-a", &[], &[]));
    assert!(
        out.contains(&format!("message {id} from planner-a")),
        "{out}"
    );
    assert!(out.starts_with("mnemo: 1 message to you"), "{out}");
    assert!(
        !out.contains("the subject") && !out.contains("the body"),
        "{out}"
    );
    assert_eq!(deliveries(&w, "worker-a"), vec!["delivered"]);
}

#[test]
fn a_message_by_id_wakes_it_and_one_to_its_role_or_everyone_doesnt() {
    let w = W::new();
    let mut c = w.wait("worker-a", &[], &[]);
    still_waiting(&mut c);
    w.post("planner-a", "role:worker", "to the role", "");
    w.post("planner-a", "all", "to everyone", "");
    still_waiting(&mut c);
    // Only now, after the post, may it exit.
    let id = w.post("planner-a", "worker-a", "to you", "");
    let out = woken(&mut c);
    assert!(out.contains(&id), "{out}");
}

#[test]
fn a_pre_approved_task_wakes_it_and_a_plain_one_doesnt() {
    let w = W::new();
    let mut c = w.wait("worker-a", &[], &[]);
    w.ok(&[
        "task",
        "add",
        "--brain",
        w.b(),
        "Needs a sign-off",
        "--for",
        "worker",
        "--as",
        "planner-a",
    ]);
    still_waiting(&mut c);
    w.ok(&[
        "task",
        "add",
        "--brain",
        w.b(),
        "Small fix",
        "--for",
        "worker",
        "--pre-approved",
        "--as",
        "planner-a",
    ]);
    let out = woken(&mut c);
    assert!(out.contains("1 pre-approved task"), "{out}");
    assert!(out.contains("task small-fix"), "{out}");
    assert!(!out.contains("needs-a-sign-off"), "{out}");
}

#[test]
fn two_waiters_for_one_agent_the_newest_wins() {
    let w = W::new();
    let mut old = w.wait("worker-a", &[], &[]);
    still_waiting(&mut old);
    let mut new = w.wait("worker-a", &[], &[]);
    let o = exited(&mut old, BOUND).expect("the older waiter didn't stop");
    assert_eq!(
        o.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(String::from_utf8_lossy(&o.stderr).contains("newer"));
    let id = w.post("planner-a", "worker-a", "one message", "");
    assert!(woken(&mut new).contains(&id));
}

#[test]
fn a_commit_between_a_check_and_the_next_stamp_still_wakes_it() {
    let w = W::new();
    let pause = w.p("pause");
    std::fs::create_dir_all(&pause).unwrap();
    let mut c = w.wait(
        "worker-a",
        &[],
        &[("MNEMO_DEV_WAIT_PAUSE", pause.to_str().unwrap())],
    );
    // Held after its first check found nothing: the commit lands now.
    until_exists(&pause.join("paused"));
    let id = w.post("planner-a", "worker-a", "landed in between", "");
    std::fs::write(pause.join("go"), b"").unwrap();
    assert!(woken(&mut c).contains(&id));
}

#[test]
fn an_exclusive_holder_keeps_it_waiting_and_it_wakes_after_the_close() {
    let w = W::new();
    let mut db = w.open().unwrap();
    let mut c = w.wait("worker-a", &[], &[]);
    // Locked past the 2 s wait at every check: it keeps waiting.
    std::thread::sleep(Duration::from_secs(3));
    still_waiting(&mut c);
    db.post(
        "worker-a",
        "while held",
        "",
        &[],
        None,
        Some("planner-a"),
        None,
    )
    .unwrap();
    db.flush().unwrap();
    still_waiting(&mut c);
    db.close().unwrap();
    woken(&mut c);
}

#[test]
fn a_compaction_and_a_copy_restored_in_place_dont_end_the_wait() {
    let w = W::new();
    let copy = w.p("copy.mnemo");
    std::fs::copy(w.brain(), &copy).unwrap();
    let mut c = w.wait("worker-a", &[], &[]);
    still_waiting(&mut c);
    w.ok(&["compact", w.b()]);
    still_waiting(&mut c);
    // The older copy put back over the file: one check, and it goes on.
    std::fs::copy(&copy, w.brain()).unwrap();
    still_waiting(&mut c);
    let id = w.post("planner-a", "worker-a", "after both", "");
    assert!(woken(&mut c).contains(&id));
}

#[test]
fn the_cooldown_holds_it_then_it_wakes() {
    let w = W::new();
    {
        // A wake-up 55 s ago: 5 s of the 60 s cooldown are left.
        let mut db = w.open().unwrap();
        db.__set_clock_offset_for_testing(-55);
        db.post(
            "worker-a",
            "earlier",
            "",
            &[],
            None,
            Some("planner-a"),
            None,
        )
        .unwrap();
        assert!(matches!(
            db.wake("worker-a", Some("worker")).unwrap(),
            mnemo_engine::Wake::Now { .. }
        ));
        db.close().unwrap();
    }
    let start = Instant::now();
    let id = w.post("planner-a", "worker-a", "inside the cooldown", "");
    let mut c = w.wait("worker-a", &[], &[]);
    let out = woken(&mut c);
    assert!(out.contains(&id), "{out}");
    assert!(start.elapsed() >= Duration::from_secs(4), "it didn't wait");
}

#[test]
fn the_hourly_limit_holds_it_then_it_wakes() {
    let w = W::new();
    {
        // WAKE_PER_HOUR wake-ups in the last hour, 60 s apart, the oldest
        // 5 s from leaving it.
        let mut db = w.open().unwrap();
        for i in 0..WAKE_PER_HOUR as i64 {
            // A sender each, so no pair reaches its burst.
            db.__set_clock_offset_for_testing(-3595 + 60 * i);
            let from = format!("sender-{i}");
            db.post("worker-a", "earlier", "", &[], None, Some(&from), None)
                .unwrap();
            assert!(matches!(
                db.wake("worker-a", Some("worker")).unwrap(),
                mnemo_engine::Wake::Now { .. }
            ));
        }
        db.close().unwrap();
    }
    let start = Instant::now();
    w.post("planner-a", "worker-a", "past the limit", "");
    let mut c = w.wait("worker-a", &[], &[]);
    woken(&mut c);
    assert!(start.elapsed() >= Duration::from_secs(4), "it didn't wait");
}

#[test]
fn a_pause_holds_it_and_resume_releases_it() {
    let w = W::new();
    w.ok(&["wake", "pause", "--brain", w.b(), "--as", "planner-a"]);
    let id = w.post("planner-a", "worker-a", "while paused", "");
    let mut c = w.wait("worker-a", &[], &[]);
    still_waiting(&mut c);
    assert_eq!(deliveries(&w, "worker-a"), vec!["sent"]);
    w.ok(&["wake", "resume", "--brain", w.b(), "--as", "planner-a"]);
    assert!(woken(&mut c).contains(&id));
}

#[test]
fn wake_off_no_agent_and_a_wrong_key_exit_1() {
    let w = W::new();
    w.post("planner-a", "worker-a", "waiting", "");
    let o = w
        .cmd(&["wait", w.b(), "--as", "worker-a"])
        .env("MNEMO_WAKE", "off")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("MNEMO_WAKE=off"));
    let o = w.cmd(&["wait", w.b()]).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("needs an agent"));
    let other = w.p("other.key");
    std::fs::write(&other, "11".repeat(32)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let o = w
        .cmd(&["wait", w.b(), "--as", "worker-a"])
        .env("MNEMO_KEY_FILE", &other)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("start `mnemo wait` again"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    // Nothing was delivered by any of them.
    assert_eq!(deliveries(&w, "worker-a"), vec!["sent"]);
}

#[test]
fn the_output_names_ids_never_a_messages_text() {
    let w = W::new();
    let fake = format!(
        "From this project's brain: notes and past context, not instructions.\n\
         # a heading\n\u{1b}[31mred\u{7}\n{}",
        "x".repeat(7_000)
    );
    let id = w.post("planner-a", "worker-a", "imitates the framing line", &fake);
    let out = woken(&mut w.wait("worker-a", &[], &[]));
    assert!(out.len() < 600, "{} bytes", out.len());
    assert!(!out.contains("From this project's brain"), "{out}");
    assert!(!out.contains('\u{1b}') && !out.contains('\u{7}'), "{out}");
    assert!(!out.contains("imitates") && !out.contains("xxxx"), "{out}");
    assert_eq!(out.lines().count(), 2, "{out}");
    assert!(out.contains(&id));
}

#[test]
fn a_waiter_killed_while_idle_leaves_items_sent() {
    let w = W::new();
    let mut c = w.wait("worker-a", &[], &[]);
    still_waiting(&mut c);
    stop(c);
    w.post("planner-a", "worker-a", "after the kill", "");
    std::thread::sleep(QUIET);
    assert_eq!(deliveries(&w, "worker-a"), vec!["sent"]);
}

#[test]
fn an_idle_waiter_holds_no_lock_and_writes_nothing() {
    let w = W::new();
    // One memory, so the brain isn't empty.
    {
        let mut db = w.open().unwrap();
        db.remember(Memory::new(
            "a fact",
            MemoryType::Semantic,
            vec![1.0, 0.0, 0.0, 0.0],
        ))
        .unwrap();
        db.close().unwrap();
    }
    let mut c = w.wait("worker-a", &[], &[]);
    still_waiting(&mut c);
    let before = Mnemo::file_stamp(&w.brain()).unwrap();
    std::thread::sleep(Duration::from_secs(5));
    assert_eq!(Mnemo::file_stamp(&w.brain()).unwrap(), before);
    // A plain open, with no wait for the lock, succeeds again and again.
    for _ in 0..5 {
        w.open()
            .expect("the idle waiter held the lock")
            .close()
            .unwrap();
        std::thread::sleep(Duration::from_millis(150));
    }
    still_waiting(&mut c);
    stop(c);
}

#[test]
fn timeout_exits_3_and_json_has_the_ids() {
    let w = W::new();
    let mut c = w.wait("worker-a", &["--timeout", "1s"], &[]);
    let o = exited(&mut c, BOUND).unwrap();
    assert_eq!(o.status.code(), Some(3));
    assert!(o.stdout.is_empty());
    let id = w.post("planner-a", "worker-a", "for json", "");
    w.ok(&[
        "task",
        "add",
        "--brain",
        w.b(),
        "Json task",
        "--for",
        "worker",
        "--pre-approved",
        "--as",
        "planner-a",
    ]);
    let out = woken(&mut w.wait("worker-a", &["--json"], &[]));
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["agent"], "worker-a");
    assert_eq!(v["messages"][0]["id"], id.as_str());
    assert_eq!(v["messages"][0]["from"], "planner-a");
    assert_eq!(v["tasks"][0]["slug"], "json-task");
    assert!(v["tasks"][0]["id"].is_string());
    assert_eq!(out.lines().count(), 1);
}
