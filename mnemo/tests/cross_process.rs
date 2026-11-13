//! v0.14.0 step 3: the cross-process suite for 12.4's shared mode (the
//! design note's step 3 list and REVIEWER-B's cases in RB-007).
//!
//! **Child processes** are this test binary run again: [`xp_child`] does
//! nothing in a normal run, and when the `MNEMO_XP_ROLE` variable is set it
//! plays that role (a writer, a reader, a compactor, a writer that dies...)
//! and exits. So the published crate gains no binary. Children report
//! through files in the test's directory: one line per acknowledged write,
//! and any unexpected error in `errors.log`, which every test checks.
//!
//! **Sizes.** The CI sizes keep the suite to well under a minute per OS.
//! `MNEMO_STRESS=1` runs the full sizes (the kill test for a minute, more
//! failure points, more proptest cases).
//!
//! **No retries in the tests.** A workload loop that meets `Locked` tries
//! its next call, as a caller would; no assertion is ever retried.
#![cfg(feature = "failpoints")]

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use fs4::fs_std::FileExt;
use mnemo_engine::__failpoints::set_writes_until_fail;
use mnemo_engine::__shared;
use mnemo_engine::{
    CachePutOpts, CancelToken, CommitBatch, KdfParams, KeyedMemory, Memory, MemoryPatch,
    MemoryType, Mnemo, MnemoConfig, MnemoError, RecallRequest, SharedMnemo, SharedOptions, Ulid,
};
use rand::Rng;
use tempfile::tempdir;

const ROLE: &str = "MNEMO_XP_ROLE";
const PW: &str = "xp-pw";
const NEW: &str = "xp-new-pw";

fn stress() -> bool {
    std::env::var("MNEMO_STRESS").as_deref() == Ok("1")
}

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

/// A v9 brain with `n` memories at `dir/name`, closed.
fn brain_at(dir: &Path, name: &str, n: usize) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    let mut db = Mnemo::create(&p, PW, cfg()).unwrap();
    for i in 0..n {
        db.remember(mem(&format!("seed {i}"))).unwrap();
    }
    db.flush().unwrap();
    db.close().unwrap();
    p
}

fn brain(dir: &Path, n: usize) -> PathBuf {
    brain_at(dir, "b.mnemo", n)
}

fn opts(deadline_ms: u64) -> SharedOptions {
    let mut o = SharedOptions::default();
    o.deadline = Duration::from_millis(deadline_ms);
    o
}

/// Whether this build refuses shared mode (Windows, Rust before 1.85): the
/// tests that need it skip.
fn refused() -> bool {
    if __shared::WINDOWS_OLD_RENAME {
        eprintln!("skipped: this build refuses shared mode");
    }
    __shared::WINDOWS_OLD_RENAME
}

/// Sorted contents of every live memory, from a read-write open of a copy
/// (a pending WAL is replayed in the copy, as any open would).
fn on_disk(p: &Path, secret: &str) -> Vec<String> {
    let copy = p.with_extension("view");
    std::fs::copy(p, &copy).unwrap();
    let mut db = Mnemo::open(&copy, secret).unwrap();
    let mut v: Vec<String> = db
        .memories()
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    drop(db);
    std::fs::remove_file(&copy).unwrap();
    v.sort();
    v
}

fn texts(s: &mut SharedMnemo) -> Vec<String> {
    let mut v: Vec<String> = s
        .read(|db| db.memories())
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    v.sort();
    v
}

// --- The child side ---------------------------------------------------------

/// The child entry point. In a normal run `MNEMO_XP_ROLE` isn't set and this
/// returns at once.
#[test]
fn xp_child() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    let code = match run_role(&role) {
        Ok(code) => code,
        Err(msg) => {
            report_error(&format!("{role} {}: {msg}", var_or("XP_ID", "-")));
            3
        }
    };
    std::process::exit(code);
}

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} not set"))
}

fn var_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn work_dir() -> PathBuf {
    PathBuf::from(var("XP_DIR"))
}

/// Append one line to `dir/name` (created if missing).
fn append(dir: &Path, name: &str, line: &str) {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(name))
        .unwrap();
    f.write_all(format!("{line}\n").as_bytes()).unwrap();
}

fn report_error(msg: &str) {
    append(&work_dir(), "errors.log", msg);
}

fn stopped() -> bool {
    work_dir().join("stop").exists()
}

fn child_opts() -> SharedOptions {
    opts(var_or("XP_DEADLINE_MS", "2000").parse().unwrap())
}

/// Open a shared handle, trying again while the file is busy (an open's
/// first read can meet `Locked` like any call). `None` when the child was
/// told to stop while it waited: `Locked` is the allowed outcome, and a
/// stopped child has nothing left to do, so the role ends cleanly (exit 0)
/// without writing. Each open that waited out its deadline is counted in
/// `open_locked.log`.
fn open_child(path: &Path, secret: &str) -> Result<Option<SharedMnemo>, String> {
    loop {
        match Mnemo::open_shared(path, secret, child_opts()) {
            Ok(s) => return Ok(Some(s)),
            Err(MnemoError::Locked { .. }) => {
                append(&work_dir(), "open_locked.log", &var_or("XP_ID", "-"));
                if stopped() {
                    return Ok(None);
                }
            }
            Err(e) => return Err(format!("open_shared: {e}")),
        }
    }
}

type RoleResult = Result<i32, String>;

fn run_role(role: &str) -> RoleResult {
    match role {
        "writer" => role_writer(),
        "reader" => role_reader(),
        "compactor" => role_compactor(),
        "plain_holder" => role_plain_holder(),
        "rekey" => role_rekey(),
        "die_after_commit" => role_die_after_commit(),
        "crash_op" => role_crash_op(),
        #[cfg(target_os = "linux")]
        "full_disk" => role_full_disk(),
        other => Err(format!("unknown role {other}")),
    }
}

/// Writes `<id>-<n>` until the stop file appears (or `XP_COUNT` writes are
/// acknowledged), logging each acknowledged one to `acks/<id>`. Every tenth
/// write is read back. Only `Locked` is expected; with `XP_ALLOW_REPLACED`,
/// `ReplacedDuringWrite` is counted in `replaced/<id>`, never acknowledged.
fn role_writer() -> RoleResult {
    let dir = work_dir();
    let id = var("XP_ID");
    let path = PathBuf::from(var("XP_PATH"));
    let count: Option<usize> = std::env::var("XP_COUNT").ok().map(|c| c.parse().unwrap());
    let allow_replaced = std::env::var("XP_ALLOW_REPLACED").is_ok();
    std::fs::create_dir_all(dir.join("acks")).unwrap();
    std::fs::create_dir_all(dir.join("replaced")).unwrap();
    let Some(mut s) = open_child(&path, &var_or("XP_SECRET", PW))? else {
        return Ok(0);
    };
    let mut n = 0usize;
    let mut acked = 0usize;
    while !stopped() && count.map_or(true, |c| acked < c) {
        let content = format!("{id}-{n}");
        n += 1;
        let m = mem(&content);
        match s.write(move |db| db.remember(m)) {
            Ok(uid) => {
                append(&dir.join("acks"), &id, &content);
                acked += 1;
                if acked % 10 == 0 {
                    match s.read(|db| db.get(&uid)) {
                        Ok(back) if back.content == content => {}
                        Ok(back) => {
                            return Err(format!("read back {} for {content}", back.content))
                        }
                        Err(MnemoError::Locked { .. }) => {}
                        Err(e) => return Err(format!("reading back {content}: {e}")),
                    }
                }
            }
            Err(MnemoError::Locked { .. }) => {}
            Err(MnemoError::ReplacedDuringWrite { .. }) if allow_replaced => {
                append(&dir.join("replaced"), &id, &content);
            }
            Err(e) => return Err(format!("writing {content}: {e}")),
        }
    }
    let notices = s.notices().join(" | ");
    std::fs::create_dir_all(dir.join("notices")).unwrap();
    append(&dir.join("notices"), &id, &notices);
    Ok(0)
}

/// Counts the memories in a loop; the count must never go down.
fn role_reader() -> RoleResult {
    let path = PathBuf::from(var("XP_PATH"));
    let Some(mut s) = open_child(&path, PW)? else {
        return Ok(0);
    };
    let mut last = 0usize;
    while !stopped() {
        match s.read(|db| Ok(db.len())) {
            Ok(n) if n < last => return Err(format!("count went from {last} to {n}")),
            Ok(n) => last = n,
            Err(MnemoError::Locked { .. }) => {}
            Err(e) => return Err(format!("reading: {e}")),
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(0)
}

/// Compacts in a loop (a plain compaction, one try at the lock), through
/// `XP_PATH` and, if set, `XP_PATH2` in turn, until the stop file appears or
/// `XP_COUNT` compactions are done. Logs each to `compactions.log`.
fn role_compactor() -> RoleResult {
    let dir = work_dir();
    let mut paths = vec![var("XP_PATH")];
    if let Ok(p) = std::env::var("XP_PATH2") {
        paths.push(p);
    }
    let count: Option<usize> = std::env::var("XP_COUNT").ok().map(|c| c.parse().unwrap());
    let secret = var_or("XP_SECRET", PW);
    let mut done = 0usize;
    while !stopped() && count.map_or(true, |c| done < c) {
        let p = &paths[done % paths.len()];
        match Mnemo::compact_file(p, &secret) {
            Ok(_) => {
                done += 1;
                append(&dir, "compactions.log", p);
                std::thread::sleep(Duration::from_millis(rand::thread_rng().gen_range(20..120)));
            }
            Err(MnemoError::Locked { .. }) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(format!("compacting {p}: {e}")),
        }
    }
    Ok(0)
}

/// Holds a plain read-write handle (the file's lock) until the stop file
/// appears; `ready` says it holds it.
fn role_plain_holder() -> RoleResult {
    let db = Mnemo::open(var("XP_PATH"), PW).map_err(|e| format!("open: {e}"))?;
    append(&work_dir(), "ready", "held");
    while !stopped() {
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(db);
    Ok(0)
}

/// A plain rekey from `XP_SECRET` to `XP_SECRET2`, then, with
/// `XP_COMPACT`, a compaction under the new secret.
fn role_rekey() -> RoleResult {
    let path = var("XP_PATH");
    let new = var("XP_SECRET2");
    let mut db = Mnemo::open(&path, &var("XP_SECRET")).map_err(|e| format!("open: {e}"))?;
    db.rekey(&new, KdfParams::fast())
        .map_err(|e| format!("rekey: {e}"))?;
    db.close().map_err(|e| format!("close: {e}"))?;
    if std::env::var("XP_COMPACT").is_ok() {
        Mnemo::compact_file(&path, &new).map_err(|e| format!("compact: {e}"))?;
    }
    Ok(0)
}

/// A plain writer whose flush fails at write `XP_N`; the process then exits
/// at once (code 10 if the flush failed), without closing anything.
fn role_die_after_commit() -> RoleResult {
    let mut db = Mnemo::open(var("XP_PATH"), &var("XP_SECRET")).map_err(|e| e.to_string())?;
    db.remember(mem("dying")).map_err(|e| e.to_string())?;
    set_writes_until_fail(var("XP_N").parse().unwrap());
    let r = db.flush();
    std::process::exit(if r.is_err() { 10 } else { 0 });
}

/// One operation (`XP_OP`: remember, update or cache_put) through a shared
/// handle, with write `XP_N` failed. `XP_MODE=die` exits at once after it;
/// `live` keeps calling: the handle must show exactly what a fresh open of
/// the file shows, then write again. The outcome goes to `outcome`.
fn role_crash_op() -> RoleResult {
    let dir = work_dir();
    let path = PathBuf::from(var("XP_PATH"));
    // No stop file in this role's test, so `None` can't happen; the rule is
    // the same as the others'.
    let Some(mut s) = open_child(&path, PW)? else {
        return Ok(0);
    };
    let op = var("XP_OP");
    let target: Option<Ulid> = std::env::var("XP_TARGET").ok().map(|t| t.parse().unwrap());
    set_writes_until_fail(var("XP_N").parse().unwrap());
    let r = match op.as_str() {
        "remember" => s.write(|db| db.remember(mem("from the child")).map(|_| ())),
        "update" => s.write(|db| {
            let patch = MemoryPatch {
                content: Some("updated by the child".into()),
                vector: Some(vec![0.0, 1.0]),
                ..Default::default()
            };
            db.update(&target.unwrap(), patch).map(|_| ())
        }),
        "cache_put" => s.write(|db| {
            db.cache_put(
                "ns",
                "k",
                b"from the child",
                CachePutOpts {
                    content_type: "text".into(),
                    ttl_secs: None,
                    cost_hint_ms: None,
                },
            )
        }),
        other => return Err(format!("unknown op {other}")),
    };
    set_writes_until_fail(-1);
    let outcome = match &r {
        Ok(()) => "ok",
        Err(MnemoError::MaybeStored(_)) => "maybe",
        Err(_) => "err",
    };
    append(&dir, "outcome", outcome);
    if var("XP_MODE") == "die" {
        std::process::exit(0);
    }
    let mine = texts(&mut s);
    let disk = on_disk(&path, PW);
    if mine != disk {
        return Err(format!(
            "the living handle shows {mine:?}, the file {disk:?}"
        ));
    }
    s.write(|db| db.remember(mem("the child again")).map(|_| ()))
        .map_err(|e| format!("the next write: {e}"))?;
    Ok(0)
}

/// A real full disk (`RLIMIT_FSIZE`; the parent starts this with `SIGXFSZ`
/// ignored, so a write past the limit fails with `EFBIG`, part-way where it
/// crosses it). For each limit, a few writes through a shared handle that
/// lives on; then the limit goes and the handle must match a fresh open:
/// `Ok` stored, a plain error not stored, `MaybeStored` either way.
#[cfg(target_os = "linux")]
fn role_full_disk() -> RoleResult {
    use rustix::process::{getrlimit, setrlimit, Resource, Rlimit};
    let dir = work_dir();
    let limits: usize = var("XP_COUNT").parse().unwrap();
    let step: u64 = var("XP_STEP").parse().unwrap();
    let hard = getrlimit(Resource::Fsize).maximum;
    let (mut ok, mut plain, mut maybe) = (0, 0, 0);
    for k in 0..limits {
        let p = brain_at(&dir.join(format!("fd{k}")), "b.mnemo", 2);
        let mut s = Mnemo::open_shared(&p, PW, opts(2000)).map_err(|e| e.to_string())?;
        let len = std::fs::metadata(&p).unwrap().len();
        // From just past the file's end to about 160 KiB beyond it (enough
        // for every write to fit), so the limit lands inside every kind of
        // write and past them.
        let limit = len + (k as u64) * step;
        setrlimit(
            Resource::Fsize,
            Rlimit {
                current: Some(limit),
                maximum: hard,
            },
        )
        .map_err(|e| format!("setrlimit: {e}"))?;
        let mut outcomes = Vec::new();
        for i in 0..4 {
            let text = format!("k{k}-{i}");
            let m = Memory::new(text.clone(), MemoryType::Semantic, vec![1.0, i as f32]);
            outcomes.push((text, s.write(move |db| db.remember(m))));
        }
        setrlimit(
            Resource::Fsize,
            Rlimit {
                current: hard,
                maximum: hard,
            },
        )
        .map_err(|e| format!("setrlimit back: {e}"))?;
        let disk = on_disk(&p, PW);
        for (text, r) in &outcomes {
            let stored = disk.contains(text);
            match r {
                Ok(_) if !stored => return Err(format!("limit {limit}: {text} Ok but not stored")),
                Ok(_) => ok += 1,
                Err(MnemoError::MaybeStored(_)) => maybe += 1,
                Err(e) if stored => {
                    return Err(format!(
                        "limit {limit}: {text} stored after a plain error: {e}"
                    ))
                }
                Err(_) => plain += 1,
            }
        }
        let mine = texts(&mut s);
        if mine != disk {
            return Err(format!(
                "limit {limit}: the handle shows {mine:?}, the file {disk:?}"
            ));
        }
        s.write(|db| db.remember(mem("after the limit")))
            .map_err(|e| format!("limit {limit}: the next write: {e}"))?;
        let mut db = Mnemo::open(&p, PW).map_err(|e| e.to_string())?;
        db.verify()
            .map_err(|e| format!("limit {limit}: verify: {e}"))?;
    }
    append(&dir, "outcome", &format!("{ok} {plain} {maybe}"));
    Ok(0)
}

// --- The parent side --------------------------------------------------------

/// Start a child in `role`, with `dir` as its work directory.
fn spawn(role: &str, dir: &Path, vars: &[(&str, String)]) -> Child {
    spawn_with(role, dir, vars, false)
}

fn spawn_with(role: &str, dir: &Path, vars: &[(&str, String)], ignore_xfsz: bool) -> Child {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let exe = std::env::current_exe().unwrap();
    let stderr = File::create(dir.join(format!(
        "child-{role}-{}.stderr",
        N.fetch_add(1, Ordering::Relaxed)
    )))
    .unwrap();
    let args = [
        "xp_child",
        "--exact",
        "--nocapture",
        "--test-threads=1",
        "-q",
    ];
    let mut cmd = if ignore_xfsz {
        // An ignored signal stays ignored across exec.
        let mut c = Command::new("sh");
        c.arg("-c")
            .arg("trap '' XFSZ; exec \"$0\" \"$@\"")
            .arg(&exe)
            .args(args);
        c
    } else {
        let mut c = Command::new(&exe);
        c.args(args);
        c
    };
    cmd.env(ROLE, role)
        .env("XP_DIR", dir)
        .envs(vars.iter().map(|(k, v)| (*k, v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .unwrap()
}

/// Wait for a child, up to `limit`, and return its exit code. A child still
/// running then is killed and the test fails.
fn wait_for(mut c: Child, dir: &Path, limit: Duration) -> i32 {
    let start = Instant::now();
    loop {
        if let Some(status) = c.try_wait().unwrap() {
            return status.code().unwrap_or(-1);
        }
        if start.elapsed() > limit {
            let _ = c.kill();
            let _ = c.wait();
            panic!("a child hung for {limit:?}; {}", child_logs(dir));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_ok(c: Child, dir: &Path) {
    let code = wait_for(c, dir, Duration::from_secs(120));
    assert_eq!(code, 0, "a child failed; {}", child_logs(dir));
}

/// `errors.log` and the children's stderr, for a failure message.
fn child_logs(dir: &Path) -> String {
    let mut out = format!(
        "errors.log: {:?}",
        std::fs::read_to_string(dir.join("errors.log")).unwrap_or_default()
    );
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".stderr") {
                let text = std::fs::read_to_string(e.path()).unwrap_or_default();
                let tail: String = text
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .rev()
                    .take(6)
                    .collect::<Vec<_>>()
                    .join(" / ");
                if text.contains("panicked") || text.contains("error") {
                    out.push_str(&format!("\n{name}: {tail}"));
                }
            }
        }
    }
    out
}

fn no_errors(dir: &Path) {
    let errors = std::fs::read_to_string(dir.join("errors.log")).unwrap_or_default();
    assert!(errors.is_empty(), "children reported: {errors}");
}

/// Every line of every file in `dir/sub`.
fn lines_in(dir: &Path, sub: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir.join(sub)) {
        for e in rd.flatten() {
            let text = std::fs::read_to_string(e.path()).unwrap();
            out.extend(text.lines().filter(|l| !l.is_empty()).map(str::to_string));
        }
    }
    out
}

/// Every acknowledged write is in the file exactly once, no content is
/// stored twice, and `verify` passes. Returns (acknowledged, in the file
/// but never acknowledged).
fn check_acks(dir: &Path, p: &Path, secret: &str) -> (usize, usize) {
    let mut db = Mnemo::open(p, secret).unwrap();
    db.verify().unwrap();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for m in db.memories().unwrap() {
        *counts.entry(m.content).or_default() += 1;
    }
    drop(db);
    let twice: Vec<_> = counts.iter().filter(|(_, n)| **n > 1).collect();
    assert!(twice.is_empty(), "stored twice: {twice:?}");
    let acks: BTreeSet<String> = lines_in(dir, "acks").into_iter().collect();
    let missing: Vec<_> = acks.iter().filter(|a| !counts.contains_key(*a)).collect();
    assert!(
        missing.is_empty(),
        "{} of {} acknowledged writes are missing: {:?}",
        missing.len(),
        acks.len(),
        missing.iter().take(10).collect::<Vec<_>>()
    );
    let unacked = counts
        .keys()
        .filter(|c| !c.starts_with("seed ") && !acks.contains(*c))
        .count();
    (acks.len(), unacked)
}

fn stop(dir: &Path) {
    std::fs::write(dir.join("stop"), b"").unwrap();
}

// --- 1. Kills ---------------------------------------------------------------

/// Three writers, two readers and a compactor in separate processes; one
/// is killed at random (`SIGKILL`, `TerminateProcess` on Windows) every 50
/// to 400 ms and started again. Every acknowledged write is in the file
/// once, `verify` passes, no reader's count goes down, and nothing fails
/// but `Locked`.
#[test]
fn kills_lose_no_acknowledged_write() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let d = dir.path();
    let p = brain(d, 3);
    let run_for = if stress() {
        Duration::from_secs(60)
    } else {
        Duration::from_secs(6)
    };
    #[derive(Clone)]
    struct Slot {
        role: &'static str,
        name: String,
        gen: usize,
    }
    let mut slots: Vec<Slot> = (0..3)
        .map(|i| Slot {
            role: "writer",
            name: format!("w{i}"),
            gen: 0,
        })
        .chain((0..2).map(|i| Slot {
            role: "reader",
            name: format!("r{i}"),
            gen: 0,
        }))
        .chain(std::iter::once(Slot {
            role: "compactor",
            name: "c".into(),
            gen: 0,
        }))
        .collect();
    let start_slot = |s: &Slot| {
        spawn(
            s.role,
            d,
            &[
                ("XP_PATH", p.display().to_string()),
                ("XP_ID", format!("{}g{}", s.name, s.gen)),
            ],
        )
    };
    let mut children: Vec<Child> = slots.iter().map(start_slot).collect();
    let mut rng = rand::thread_rng();
    let mut kills = 0;
    let start = Instant::now();
    while start.elapsed() < run_for {
        std::thread::sleep(Duration::from_millis(rng.gen_range(50..400)));
        let i = rng.gen_range(0..slots.len());
        // A child that already failed is reported at the end; don't hide
        // its exit code behind a kill.
        if let Some(status) = children[i].try_wait().unwrap() {
            panic!(
                "{} exited by itself ({status}); {}",
                slots[i].name,
                child_logs(d)
            );
        }
        children[i].kill().unwrap();
        children[i].wait().unwrap();
        kills += 1;
        slots[i].gen += 1;
        children[i] = start_slot(&slots[i]);
    }
    stop(d);
    for c in children {
        wait_ok(c, d);
    }
    no_errors(d);
    let (acked, unacked) = check_acks(d, &p, PW);
    let count_lines = |name: &str| {
        std::fs::read_to_string(d.join(name))
            .unwrap_or_default()
            .lines()
            .count()
    };
    let compactions = count_lines("compactions.log");
    let open_locked = count_lines("open_locked.log");
    eprintln!(
        "kill test: {run_for:?}, {kills} kills, {compactions} compactions, {acked} writes \
         acknowledged, {unacked} in the file but never acknowledged, {open_locked} child \
         opens waited out their 2 s deadline"
    );
    assert!(acked > 0, "no write was acknowledged");
}

// --- 2. A real full disk ------------------------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn a_real_full_disk_in_a_process_that_lives_on() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let d = dir.path();
    // 160 KiB past the file's end either way, in 1 KiB steps at full size.
    let (limits, step) = if stress() { (160, 1024) } else { (40, 4096) };
    let c = spawn_with(
        "full_disk",
        d,
        &[
            ("XP_COUNT", limits.to_string()),
            ("XP_STEP", step.to_string()),
        ],
        true,
    );
    wait_ok(c, d);
    no_errors(d);
    let outcome = std::fs::read_to_string(d.join("outcome")).unwrap();
    eprintln!(
        "full disk: ok / plain error / maybe stored = {}",
        outcome.trim()
    );
    let n: Vec<usize> = outcome
        .split_whitespace()
        .map(|x| x.parse().unwrap())
        .collect();
    assert!(n[1] + n[2] > 0, "no limit made a write fail");
    assert!(n[0] > 0, "no limit let a write through");
}

// --- 3. Rekey across processes ----------------------------------------------

/// Another process's rekey keeps the data key, so a handle with the old
/// secret carries on; a compaction under the new secret then revokes it:
/// its next call is `WrongPassphrase`, and stays so.
#[test]
fn another_processs_rekey_then_compaction_revokes_the_old_secret() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let d = dir.path();
    let p = brain(d, 1);
    let mut a = Mnemo::open_shared(&p, PW, SharedOptions::default()).unwrap();
    let vars = |compact: bool| {
        let mut v = vec![
            ("XP_PATH", p.display().to_string()),
            ("XP_SECRET", PW.to_string()),
            ("XP_SECRET2", NEW.to_string()),
        ];
        if compact {
            v.push(("XP_COMPACT", "1".to_string()));
        }
        v
    };
    wait_ok(spawn("rekey", d, &vars(false)), d);
    assert_eq!(texts(&mut a), vec!["seed 0"]);
    a.write(|db| db.remember(mem("after the rekey"))).unwrap();

    // Back to PW, then rekey and compact under NEW in one child.
    let mut back = Mnemo::open(&p, NEW).unwrap();
    back.rekey(PW, KdfParams::fast()).unwrap();
    back.close().unwrap();
    wait_ok(spawn("rekey", d, &vars(true)), d);
    for _ in 0..2 {
        let e = a.read(|db| db.memories()).unwrap_err();
        assert!(matches!(e, MnemoError::WrongPassphrase), "{e}");
    }
    let mut b = Mnemo::open_shared(&p, NEW, SharedOptions::default()).unwrap();
    assert_eq!(texts(&mut b), vec!["after the rekey", "seed 0"]);
}

/// Another process rekeys, a third dies after its commit point under the
/// new secret: the old-secret handle replays the WAL with the data key it
/// holds.
#[test]
fn a_writer_dying_after_another_processs_rekey_is_replayed_with_the_held_key() {
    if refused() {
        return;
    }
    for n in 0..40 {
        let dir = tempdir().unwrap();
        let d = dir.path();
        let p = brain(d, 1);
        let mut a = Mnemo::open_shared(&p, PW, SharedOptions::default()).unwrap();
        let vars = [
            ("XP_PATH", p.display().to_string()),
            ("XP_SECRET", PW.to_string()),
            ("XP_SECRET2", NEW.to_string()),
        ];
        wait_ok(spawn("rekey", d, &vars), d);
        let dying = [
            ("XP_PATH", p.display().to_string()),
            ("XP_SECRET", NEW.to_string()),
            ("XP_N", n.to_string()),
        ];
        let code = wait_for(
            spawn("die_after_commit", d, &dying),
            d,
            Duration::from_secs(60),
        );
        let pending = matches!(
            Mnemo::open_read_only(&p, NEW).err(),
            Some(MnemoError::NeedsWriteOpen { .. })
        );
        if code != 10 || !pending {
            continue;
        }
        assert_eq!(texts(&mut a), vec!["dying", "seed 0"], "n={n}");
        assert!(
            Mnemo::open_read_only(&p, NEW).is_ok(),
            "the WAL was replayed"
        );
        return;
    }
    panic!("no child left a pending WAL");
}

// --- 4. Two paths to one brain --------------------------------------------

/// A writer on a symlink and one on the real path, in two processes, while
/// a third compacts through each in turn. Every acknowledged write is in
/// the file, the link stays a link, and there's one sidecar.
#[cfg(unix)]
#[test]
fn two_paths_to_one_brain_across_processes() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let d = dir.path();
    let real = brain(&d.join("real"), 1);
    let link = d.join("link.mnemo");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let writes = if stress() { 300 } else { 40 };
    let w1 = spawn(
        "writer",
        d,
        &[
            ("XP_PATH", link.display().to_string()),
            ("XP_ID", "link".into()),
            ("XP_COUNT", writes.to_string()),
        ],
    );
    let w2 = spawn(
        "writer",
        d,
        &[
            ("XP_PATH", real.display().to_string()),
            ("XP_ID", "real".into()),
            ("XP_COUNT", writes.to_string()),
        ],
    );
    let c = spawn(
        "compactor",
        d,
        &[
            ("XP_PATH", link.display().to_string()),
            ("XP_PATH2", real.display().to_string()),
            ("XP_COUNT", if stress() { "60" } else { "10" }.into()),
        ],
    );
    wait_ok(w1, d);
    wait_ok(w2, d);
    wait_ok(c, d);
    no_errors(d);
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    let (acked, _) = check_acks(d, &real, PW);
    assert_eq!(acked, 2 * writes);
    assert!(!__shared::sidecar_path(&link).exists(), "one sidecar only");
}

// --- 5. The sidecar removed under running processes -------------------------

/// Delete `dir`, which live children may still be creating names in: retry
/// only on `DirectoryNotEmpty` (a name created mid-delete), at most 50
/// times 10 ms apart, and panic once that's used up. `NotFound` is done.
#[cfg(unix)]
fn remove_dir_racing_children(dir: &Path) {
    let mut last = None;
    for _ in 0..50 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            // `ErrorKind::DirectoryNotEmpty` needs Rust 1.83 (the MSRV is
            // 1.75), so the errno.
            Err(e) if e.raw_os_error() == Some(rustix::io::Errno::NOTEMPTY.raw_os_error()) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("deleting {}: {e}", dir.display()),
        }
    }
    panic!("deleting {} after 50 tries: {last:?}", dir.display());
}

#[cfg(unix)]
#[test]
fn the_sidecar_removed_while_two_processes_write() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let d = dir.path();
    let p = brain(d, 1);
    let side = __shared::sidecar_path(&std::fs::canonicalize(&p).unwrap());
    let writer = |id: &str| {
        spawn(
            "writer",
            d,
            &[("XP_PATH", p.display().to_string()), ("XP_ID", id.into())],
        )
    };
    let (w1, w2) = (writer("a"), writer("b"));
    let rounds = if stress() { 30 } else { 5 };
    for i in 0..rounds {
        std::thread::sleep(Duration::from_millis(150));
        // Moved away, then deleted. The children keep a descriptor on the
        // moved directory and, until a lock file can't be opened there, go
        // on creating `intent.lock` and lock-info's temp name in it (as
        // built: they reopen by path only then). A name created between
        // `remove_dir_all`'s listing and its `rmdir` gives ENOTEMPTY, so
        // the delete, this test's own action, is retried on that error
        // alone, at most 50 times 10 ms apart; running out fails the test.
        // No assertion or engine call is retried.
        let gone = d.join(format!("gone{i}"));
        if std::fs::rename(&side, &gone).is_ok() {
            remove_dir_racing_children(&gone);
        }
    }
    // The children make it again at their next calls. Wait until each has
    // acknowledged two more writes: the first may come from a call that
    // started before the removal, but the second's call started after the
    // first's acknowledgement, so after the removal. Capped at 10 s, so a
    // child that stops making progress still fails the test.
    let removed_at = Instant::now();
    let acks = |id: &str| {
        std::fs::read_to_string(d.join("acks").join(id))
            .unwrap_or_default()
            .lines()
            .count()
    };
    let (a0, b0) = (acks("a"), acks("b"));
    while acks("a") < a0 + 2 || acks("b") < b0 + 2 {
        assert!(
            removed_at.elapsed() < Duration::from_secs(10),
            "the children didn't make progress after the last removal: a {} -> {}, b {} -> {}; {}",
            a0,
            acks("a"),
            b0,
            acks("b"),
            child_logs(d)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    eprintln!(
        "sidecar removal: two writes per child {:?} after the last removal",
        removed_at.elapsed()
    );
    assert!(side.is_dir(), "the sidecar wasn't made again");
    stop(d);
    wait_ok(w1, d);
    wait_ok(w2, d);
    no_errors(d);
    check_acks(d, &p, PW);
    for n in lines_in(d, "notices") {
        assert!(n.is_empty(), "a notice: {n}");
    }
}

/// A handle that started while the sidecar couldn't be used (a file at its
/// name) keeps taking the main lock alone after the name is freed and
/// another process makes the directory: its writes and the other's are all
/// stored. It doesn't pick the directory up (it says why in its notices);
/// that's for step 7.
#[cfg(unix)]
#[test]
fn a_sidecar_that_appears_later_for_a_handle_that_started_without_one() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let d = dir.path();
    let p = brain(d, 1);
    let side = __shared::sidecar_path(&std::fs::canonicalize(&p).unwrap());
    std::fs::write(&side, b"in the way").unwrap();
    let writes = if stress() { 200 } else { 30 };
    let w1 = spawn(
        "writer",
        d,
        &[
            ("XP_PATH", p.display().to_string()),
            ("XP_ID", "early".into()),
            ("XP_COUNT", writes.to_string()),
        ],
    );
    // Wait until it has written, then free the name for the second.
    let start = Instant::now();
    while lines_in(d, "acks").is_empty() && start.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_millis(5));
    }
    std::fs::remove_file(&side).unwrap();
    let w2 = spawn(
        "writer",
        d,
        &[
            ("XP_PATH", p.display().to_string()),
            ("XP_ID", "late".into()),
            ("XP_COUNT", writes.to_string()),
        ],
    );
    wait_ok(w1, d);
    wait_ok(w2, d);
    no_errors(d);
    let (acked, _) = check_acks(d, &p, PW);
    assert_eq!(acked, 2 * writes);
    assert!(side.is_dir());
    let early = std::fs::read_to_string(d.join("notices").join("early")).unwrap();
    assert!(early.contains("sidecar"), "{early}");
    let late = std::fs::read_to_string(d.join("notices").join("late")).unwrap();
    assert!(late.trim().is_empty(), "{late}");
}

// --- 6. A cancel while holding the intent lock ------------------------------

/// A writer waits for the main lock, which another process holds, keeping
/// the intent lock as writers do. A cancel then returns at once, and the
/// intent lock is free straight after.
#[test]
fn a_cancel_while_holding_the_intent_lock_frees_it() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let d = dir.path();
    let p = brain(d, 1);
    let token = CancelToken::new();
    let mut o = opts(30_000);
    o.cancel = Some(token.clone());
    let mut a = Mnemo::open_shared(&p, PW, o).unwrap();
    let holder = spawn("plain_holder", d, &[("XP_PATH", p.display().to_string())]);
    let start = Instant::now();
    while !d.join("ready").exists() {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "{}",
            child_logs(d)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let intent_path =
        __shared::sidecar_path(&std::fs::canonicalize(&p).unwrap()).join(__shared::INTENT);
    let watcher = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&intent_path)
            .unwrap();
        let held_while_waiting = FileExt::try_lock_exclusive(&f).is_err();
        token.cancel();
        (f, held_while_waiting)
    });
    let e = a.write(|db| db.remember(mem("cancelled"))).unwrap_err();
    assert!(matches!(e, MnemoError::Cancelled), "{e}");
    let (f, held_while_waiting) = watcher.join().unwrap();
    assert!(
        held_while_waiting,
        "the waiting writer held the intent lock"
    );
    FileExt::try_lock_exclusive(&f).expect("the intent lock is free straight after");
    FileExt::unlock(&f).unwrap();
    stop(d);
    wait_ok(holder, d);
    a.set_cancel(None);
    a.write(|db| db.remember(mem("after"))).unwrap();
}

// --- 7. A closure that commits part-way with commit_deltas ------------------

#[test]
fn a_commit_deltas_window_that_commits_part_way_is_never_a_plain_error_with_part_stored() {
    if refused() {
        return;
    }
    let (mut part, mut done) = (0, false);
    for n in 0..80 {
        let dir = tempdir().unwrap();
        let p = brain(dir.path(), 1);
        let mut a = Mnemo::open_shared(&p, PW, SharedOptions::default()).unwrap();
        set_writes_until_fail(n);
        let r = a.write(|db| {
            db.commit_deltas(CommitBatch {
                facts: vec![KeyedMemory {
                    memory: mem("the batch's fact"),
                    key: Some("fact-key".into()),
                }],
                thread_id: Some("t".into()),
                ..Default::default()
            })?;
            db.remember(mem("after the batch"))?;
            Ok(())
        });
        set_writes_until_fail(-1);
        let disk = on_disk(&p, PW);
        let batch = disk.contains(&"the batch's fact".to_string());
        let after = disk.contains(&"after the batch".to_string());
        match r {
            Ok(()) => {
                assert!(batch && after, "n={n}: Ok with {disk:?}");
                done = true;
                break;
            }
            Err(MnemoError::MaybeStored(msg)) => {
                if msg.contains("part of the window") {
                    part += 1;
                }
            }
            Err(e) => assert!(
                !batch && !after,
                "n={n}: a plain error ({e}) with {disk:?} in the file"
            ),
        }
        assert_eq!(texts(&mut a), disk, "n={n}: the handle matches the file");
    }
    assert!(done, "no window finished");
    assert!(part > 0, "no failure said part of the window was stored");
}

// --- 8. Linux inode reuse -----------------------------------------------------

/// Compactions until the path gets the inode number the handle last saw
/// back (ext4 reuses a freed one), so the identity matches and only the
/// seal tells the file changed. Reports whether it could be forced.
#[cfg(target_os = "linux")]
#[test]
fn a_reused_inode_number_is_caught_by_the_seal() {
    use std::os::unix::fs::MetadataExt;
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 2);
    let mut a = Mnemo::open_shared(&p, PW, SharedOptions::default()).unwrap();
    a.write(|db| db.remember(mem("before"))).unwrap();
    let ino = std::fs::metadata(&p).unwrap().ino();
    let mut forced = false;
    for i in 0..if stress() { 200 } else { 40 } {
        let mut db = Mnemo::open(&p, PW).unwrap();
        db.remember(mem(&format!("between {i}"))).unwrap();
        db.close().unwrap();
        Mnemo::compact_file(p.to_str().unwrap(), PW).unwrap();
        if std::fs::metadata(&p).unwrap().ino() == ino {
            forced = true;
            break;
        }
    }
    eprintln!("inode reuse forced: {forced}");
    let want = on_disk(&p, PW);
    assert_eq!(texts(&mut a), want);
    a.write(|db| db.remember(mem("after"))).unwrap();
    assert!(on_disk(&p, PW).contains(&"after".to_string()));
}

// --- 9. A live handle through copies, compactions and a deleted file ---------

/// The handle follows a copy of an older state written over the file in
/// place (same file, a header that verifies under its key), a compacted
/// copy (a new data key), two compactions in a row, and a deleted file
/// (an error, never a new file) brought back. Runs on Windows CI too.
#[test]
fn a_live_handle_through_copies_compactions_and_a_deleted_file() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let d = dir.path();
    let p = brain(d, 1);
    let mut a = Mnemo::open_shared(&p, PW, SharedOptions::default()).unwrap();
    a.write(|db| db.remember(mem("first"))).unwrap();
    let older = d.join("older.mnemo");
    std::fs::copy(&p, &older).unwrap();
    a.write(|db| db.remember(mem("second"))).unwrap();

    // An older copy over the file in place.
    std::fs::copy(&older, &p).unwrap();
    assert_eq!(texts(&mut a), vec!["first", "seed 0"]);

    // A compacted copy (a new data key) over the file in place.
    let other = d.join("other.mnemo");
    std::fs::copy(&p, &other).unwrap();
    Mnemo::compact_file(other.to_str().unwrap(), PW).unwrap();
    {
        let mut db = Mnemo::open(&other, PW).unwrap();
        db.remember(mem("in the compacted copy")).unwrap();
        db.close().unwrap();
    }
    std::fs::copy(&other, &p).unwrap();
    assert_eq!(
        texts(&mut a),
        vec!["first", "in the compacted copy", "seed 0"]
    );

    // Two compactions in a row.
    Mnemo::compact_file(p.to_str().unwrap(), PW).unwrap();
    Mnemo::compact_file(p.to_str().unwrap(), PW).unwrap();
    a.write(|db| db.remember(mem("after two"))).unwrap();
    assert_eq!(on_disk(&p, PW), texts(&mut a));

    // Deleted: an error, and nothing is created at the path.
    let keep = d.join("keep.mnemo");
    std::fs::rename(&p, &keep).unwrap();
    let e = a.read(|db| db.memories()).unwrap_err();
    assert!(
        matches!(&e, MnemoError::Io(io) if io.kind() == std::io::ErrorKind::NotFound),
        "{e}"
    );
    assert!(!p.exists());
    std::fs::rename(&keep, &p).unwrap();
    assert_eq!(texts(&mut a).len(), 4);
}

// --- 10. The 0.13.2 compactor against shared writers ------------------------

/// The released 0.13.2 CLI's `mnemo compact` releases the lock before its
/// rename. Against shared writers, every write it loses must be reported
/// (`ReplacedDuringWrite`), never acknowledged. Needs `MNEMO_V0_13_2_BIN`
/// (its own CI job); skipped without it.
#[test]
fn the_0_13_2_compactor_never_loses_an_acknowledged_write() {
    let Ok(bin) = std::env::var("MNEMO_V0_13_2_BIN") else {
        eprintln!("skipped: MNEMO_V0_13_2_BIN not set");
        return;
    };
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let d = dir.path();
    let p = brain(d, 3);
    let writers: Vec<Child> = (0..3)
        .map(|i| {
            spawn(
                "writer",
                d,
                &[
                    ("XP_PATH", p.display().to_string()),
                    ("XP_ID", format!("w{i}")),
                    ("XP_ALLOW_REPLACED", "1".into()),
                ],
            )
        })
        .collect();
    let run_for = if stress() {
        Duration::from_secs(60)
    } else {
        Duration::from_secs(15)
    };
    let start = Instant::now();
    let (mut compactions, mut busy) = (0, 0);
    while start.elapsed() < run_for {
        let out = Command::new(&bin)
            .arg("compact")
            .arg(&p)
            .env("MNEMO_PASSPHRASE", PW)
            .output()
            .unwrap();
        if out.status.success() {
            compactions += 1;
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(err.contains("lock"), "0.13.2 compact failed: {err}");
            busy += 1;
        }
        std::thread::sleep(Duration::from_millis(rand::thread_rng().gen_range(10..80)));
    }
    stop(d);
    for c in writers {
        wait_ok(c, d);
    }
    no_errors(d);
    let (acked, _) = check_acks(d, &p, PW);
    let reported = lines_in(d, "replaced").len();
    eprintln!(
        "0.13.2 compactor: {compactions} compactions ({busy} found the file busy), {acked} \
         writes acknowledged, {reported} reported as ReplacedDuringWrite"
    );
    assert!(compactions > 0 && acked > 0);
}

// --- 11. Mixed plain and shared handles --------------------------------------

/// A plain long-lived handle holds the lock for its life: shared callers
/// wait their deadline and get `Locked`, in-process and across processes.
#[test]
fn a_plain_handle_makes_shared_callers_wait_their_deadline() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let d = dir.path();
    let p = brain(d, 1);
    let mut a = Mnemo::open_shared(&p, PW, opts(300)).unwrap();
    let check_waits = |a: &mut SharedMnemo| {
        for write in [false, true] {
            let t = Instant::now();
            let e = if write {
                a.write(|db| db.remember(mem("refused"))).map(|_| ())
            } else {
                a.read(|db| db.memories()).map(|_| ())
            }
            .unwrap_err();
            let waited = t.elapsed();
            assert!(matches!(e, MnemoError::Locked { .. }), "{e}");
            assert!(waited >= Duration::from_millis(290), "waited {waited:?}");
            assert!(waited < Duration::from_secs(5), "waited {waited:?}");
        }
    };

    // In this process.
    let plain = Mnemo::open(&p, PW).unwrap();
    check_waits(&mut a);
    drop(plain);
    assert_eq!(texts(&mut a), vec!["seed 0"]);

    // In another process.
    let holder = spawn("plain_holder", d, &[("XP_PATH", p.display().to_string())]);
    let start = Instant::now();
    while !d.join("ready").exists() {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "{}",
            child_logs(d)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    check_waits(&mut a);
    stop(d);
    wait_ok(holder, d);
    a.write(|db| db.remember(mem("after"))).unwrap();

    // A shared handle holds nothing between calls: a plain open gets in.
    let plain = Mnemo::open(&p, PW).unwrap();
    drop(plain);
}

// --- 13. Crash injection at every write, in a child that dies or lives on ---

/// For each operation (remember, update, cache_put) and each write it makes,
/// a child fails that write and then either exits at once or keeps calling.
/// A handle in this process, live throughout, must never return a memory
/// (or a cache value) that a fresh open of the file doesn't have, and must
/// see what the child stored.
#[test]
fn a_child_failing_at_every_write_never_shows_another_handle_a_phantom() {
    if refused() {
        return;
    }
    let max_n = if stress() { 60 } else { 30 };
    let mut cases = 0;
    for op in ["remember", "update", "cache_put"] {
        for mode in ["die", "live"] {
            for n in 0..max_n {
                let dir = tempdir().unwrap();
                let d = dir.path();
                let p = brain(d, 1);
                let mut a = Mnemo::open_shared(&p, PW, SharedOptions::default()).unwrap();
                let target = a.write(|db| db.remember(mem("the target"))).unwrap();
                let c = spawn(
                    "crash_op",
                    d,
                    &[
                        ("XP_PATH", p.display().to_string()),
                        ("XP_OP", op.into()),
                        ("XP_MODE", mode.into()),
                        ("XP_N", n.to_string()),
                        ("XP_TARGET", target.to_string()),
                    ],
                );
                wait_ok(c, d);
                no_errors(d);
                cases += 1;
                let outcome = std::fs::read_to_string(d.join("outcome")).unwrap();
                let disk = on_disk(&p, PW);
                assert_eq!(texts(&mut a), disk, "{op} {mode} n={n}");
                // Recall's hits all exist in the file.
                let hits = a
                    .read(|db| db.recall(&RecallRequest::new(vec![0.0, 1.0]).top_k(10)))
                    .unwrap();
                for h in hits {
                    assert!(disk.contains(&h.memory.content), "{op} {mode} n={n}: {h:?}");
                }
                let cached = a
                    .read(|db| db.cache_get("ns", "k"))
                    .unwrap()
                    .map(|v| v.value);
                let mut fresh = Mnemo::open(&p, PW).unwrap();
                let fresh_cached = fresh.cache_get("ns", "k").unwrap().map(|v| v.value);
                drop(fresh);
                assert_eq!(cached, fresh_cached, "{op} {mode} n={n}");
                // A plain error stored nothing (MaybeStored may go either way).
                let effect = match op {
                    "remember" => disk.contains(&"from the child".to_string()),
                    "update" => disk.contains(&"updated by the child".to_string()),
                    _ => cached.as_deref() == Some(&b"from the child"[..]),
                };
                if outcome.trim() == "err" {
                    assert!(
                        !effect,
                        "{op} {mode} n={n}: a plain error, but it was stored"
                    );
                }
                if outcome.trim() == "ok" {
                    // Past the last write: nothing failed. Stored, and seen.
                    match op {
                        "remember" => assert!(disk.contains(&"from the child".to_string())),
                        "update" => assert!(disk.contains(&"updated by the child".to_string())),
                        _ => assert_eq!(cached.as_deref(), Some(&b"from the child"[..])),
                    }
                    break;
                }
            }
        }
    }
    eprintln!("crash injection: {cases} child runs");
}
