//! v0.14.0 step 4b (12.4): the CLI's commands wait for the brain's lock
//! through the lock layer (`--wait`), reads alongside other readers and
//! writes with writer preference, and `Locked` names the holder;
//! `serve --mcp --shared` holds no lock between tool calls; `inspect`
//! waits, and explains a pending WAL instead of failing. Failpoints builds
//! only (the test hooks).
#![cfg(feature = "failpoints")]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use mnemo_engine::__failpoints::set_writes_until_fail;
use mnemo_engine::__shared;
use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, MnemoError, SharedOptions};
use tempfile::tempdir;

const PW: &str = "cli-shared-pw";

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 2,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn mem(text: &str) -> Memory {
    Memory::new(text, MemoryType::Semantic, vec![1.0, 0.0])
}

fn brain(dir: &Path, n: usize) -> PathBuf {
    let p = dir.join("b.mnemo");
    let mut db = Mnemo::create(&p, PW, cfg()).unwrap();
    for i in 0..n {
        db.remember(mem(&format!("seed {i}"))).unwrap();
    }
    db.close().unwrap();
    p
}

fn refused() -> bool {
    if __shared::WINDOWS_OLD_RENAME {
        eprintln!("skipped: this build refuses shared mode");
    }
    __shared::WINDOWS_OLD_RENAME
}

fn mnemo(args: &[&str]) -> (Output, Duration) {
    let t = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_mnemo"))
        .args(args)
        .env("MNEMO_PASSPHRASE", PW)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (out, t.elapsed())
}

fn ok(out: &Output) -> String {
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A one-memory import file.
fn jsonl(dir: &Path, text: &str) -> String {
    let f = dir.join(format!("{}.jsonl", text.replace(' ', "_")));
    std::fs::write(
        &f,
        format!("{{\"content\":\"{text}\",\"memory_type\":\"semantic\",\"vector\":[0.5,0.5]}}\n"),
    )
    .unwrap();
    f.to_str().unwrap().to_string()
}

/// Hold `f`'s handle for `hold` in another thread; returns once it holds.
fn hold<T: Send + 'static>(
    make: impl FnOnce() -> T + Send + 'static,
    hold: Duration,
) -> std::thread::JoinHandle<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let t = std::thread::spawn(move || {
        let h = make();
        tx.send(()).unwrap();
        std::thread::sleep(hold);
        drop(h);
    });
    rx.recv().unwrap();
    t
}

/// A shared handle holding a read window for `d`.
fn hold_shared_read(p: &Path, d: Duration) -> std::thread::JoinHandle<()> {
    let p = p.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    let t = std::thread::spawn(move || {
        let mut s = Mnemo::open_shared(&p, PW, SharedOptions::default()).unwrap();
        s.read(|_| {
            tx.send(()).unwrap();
            std::thread::sleep(d);
            Ok(())
        })
        .unwrap();
    });
    rx.recv().unwrap();
    t
}

#[test]
fn wait_takes_a_duration() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let path = p.to_str().unwrap();
    for w in ["2s", "500ms", "1m", "3", "0"] {
        ok(&mnemo(&["--wait", w, "info", path]).0);
    }
    // After the subcommand too (a global flag).
    ok(&mnemo(&["info", path, "--wait", "2s"]).0);
    let (out, _) = mnemo(&["--wait", "soon", "info", path]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not a duration"));
}

/// A read waits for a plain writer to finish, and gives up at `--wait`.
#[test]
fn a_read_waits_for_a_writer_then_gives_up_at_its_deadline() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let path = p.to_str().unwrap().to_string();

    let pp = p.clone();
    let w = hold(
        move || Mnemo::open(&pp, PW).unwrap(),
        Duration::from_millis(1200),
    );
    let (out, took) = mnemo(&["--wait", "10s", "list", &path]);
    w.join().unwrap();
    assert!(ok(&out).contains("seed 0"));
    assert!(took >= Duration::from_millis(800), "it waited: {took:?}");

    let pp = p.clone();
    let w = hold(
        move || Mnemo::open(&pp, PW).unwrap(),
        Duration::from_millis(3000),
    );
    let (out, took) = mnemo(&["--wait", "300ms", "info", &path]);
    w.join().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("locked"), "{err}");
    assert!(
        took < Duration::from_millis(2500),
        "gave up at the deadline: {took:?}"
    );
}

/// Reads run alongside a shared reader; writes wait for it and get in.
#[test]
fn reads_run_alongside_readers_and_writes_wait_for_them() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let path = p.to_str().unwrap();

    let r = hold_shared_read(&p, Duration::from_millis(4000));
    for args in [
        vec!["list", path],
        vec!["info", path],
        vec!["verify", path],
        vec!["recall", path, "--query", "1,0", "--no-track"],
    ] {
        let (out, took) = mnemo(&args);
        ok(&out);
        assert!(
            took < Duration::from_millis(3000),
            "{args:?} waited {took:?}"
        );
    }
    r.join().unwrap();

    let jsonl = jsonl(dir.path(), "from the cli");
    let r = hold_shared_read(&p, Duration::from_millis(1500));
    let (out, took) = mnemo(&["import", path, &jsonl]);
    r.join().unwrap();
    ok(&out);
    assert!(
        took >= Duration::from_millis(1000),
        "the write waited: {took:?}"
    );
    let mut db = Mnemo::open(&p, PW).unwrap();
    assert!(db
        .memories()
        .unwrap()
        .iter()
        .any(|m| m.content == "from the cli"));
}

/// A write that times out names what holds the file.
#[test]
fn locked_names_the_holder() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    // A shared handle makes the sidecar, where the note goes.
    drop(Mnemo::open_shared(&p, PW, SharedOptions::default()).unwrap());
    let pp = p.clone();
    let h = hold(
        move || Mnemo::open_exclusive(&pp, PW, "import", SharedOptions::default()).unwrap(),
        Duration::from_millis(3000),
    );
    let jsonl = jsonl(dir.path(), "x");
    let (out, _) = mnemo(&["--wait", "300ms", "import", p.to_str().unwrap(), &jsonl]);
    h.join().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("held by import since"), "{err}");
}

fn mcp_line(id: u32, tool: &str, args: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{tool}","arguments":{args}}}}}"#
    )
}

/// `serve --mcp --shared` holds nothing between tool calls: another
/// process writes meanwhile, and the server sees it and writes too.
#[test]
fn a_shared_mcp_server_holds_no_lock_between_calls() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    // A file at the sidecar's name, so there's a notice to print.
    let side = __shared::sidecar_path(&std::fs::canonicalize(&p).unwrap());
    std::fs::write(&side, b"in the way").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mnemo"))
        .args([
            "--wait",
            "5s",
            "serve",
            "--mcp",
            "--shared",
            p.to_str().unwrap(),
        ])
        .env("MNEMO_PASSPHRASE", PW)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut call = |line: String| -> serde_json::Value {
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
        serde_json::from_str(&stdout.next().unwrap().unwrap()).unwrap()
    };
    let init = call(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{}}}"#
            .to_string(),
    );
    assert!(init["result"].is_object(), "{init}");

    // The server is up and idle: a plain write from here gets in at once.
    let mut db = Mnemo::open(&p, PW).unwrap();
    db.remember(mem("from outside")).unwrap();
    db.close().unwrap();

    let r = call(mcp_line(2, "list", "{}"));
    assert!(r["result"].to_string().contains("from outside"), "{r}");
    let r = call(mcp_line(
        3,
        "remember",
        r#"{"content":"from the server","memory_type":"semantic","vector":[0.5,0.5]}"#,
    ));
    assert!(r["result"].is_object(), "{r}");
    // A tool error aborts its window and comes back as the tool's error.
    let r = call(mcp_line(4, "forget", r#"{"id":"not-an-id"}"#));
    assert!(r["error"].is_object(), "{r}");
    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success());
    let mut err = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut err).unwrap();
    assert_eq!(err.matches("mnemo mcp: note:").count(), 1, "{err}");
    assert!(err.contains("shared mode"), "{err}");

    let mut db = Mnemo::open(&p, PW).unwrap();
    let texts: Vec<String> = db
        .memories()
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    assert!(texts.contains(&"from the server".to_string()), "{texts:?}");
    assert!(texts.contains(&"from outside".to_string()));
}

/// Start `mnemo inspect` and wait for its link, or its exit.
fn inspect(p: &Path, wait: &str) -> Result<std::process::Child, String> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mnemo"))
        .args(["--wait", wait, "inspect", p.to_str().unwrap()])
        .env("MNEMO_PASSPHRASE", PW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    loop {
        match lines.next() {
            Some(Ok(l)) if l.starts_with("http://") => {
                std::thread::spawn(move || for _ in lines {});
                return Ok(child);
            }
            Some(Ok(_)) => continue,
            _ => {
                let mut err = String::new();
                std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut err).unwrap();
                child.wait().unwrap();
                return Err(err);
            }
        }
    }
}

#[test]
fn inspect_waits_for_a_writer_and_explains_a_pending_wal() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let pp = p.clone();
    let w = hold(
        move || Mnemo::open(&pp, PW).unwrap(),
        Duration::from_millis(1000),
    );
    let t = Instant::now();
    let mut child = inspect(&p, "10s").expect("inspect waited for the writer");
    assert!(t.elapsed() >= Duration::from_millis(700));
    w.join().unwrap();
    child.kill().unwrap();
    child.wait().unwrap();

    // A writer that died after its commit point leaves a pending WAL.
    for n in 0..40 {
        let d = dir.path().join(format!("wal{n}"));
        std::fs::create_dir(&d).unwrap();
        let q = brain(&d, 1);
        let mut db = Mnemo::open(&q, PW).unwrap();
        db.remember(mem("dying")).unwrap();
        set_writes_until_fail(n);
        let r = db.flush();
        set_writes_until_fail(-1);
        drop(db);
        let pending = matches!(
            Mnemo::open_read_only(&q, PW).err(),
            Some(MnemoError::NeedsWriteOpen { .. })
        );
        if r.is_err() && pending {
            let err = inspect(&q, "2s").expect_err("inspect can't open it");
            assert!(err.contains("any writer's next call recovers it"), "{err}");
            return;
        }
    }
    panic!("no write left a pending WAL");
}
