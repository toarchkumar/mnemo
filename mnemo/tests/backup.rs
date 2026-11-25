//! v0.21.1 (G13, spec.v0211 step 1): `Mnemo::backup_to` and `mnemo backup`.
//! A copy taken while a shared handle writes checks out and holds every
//! record committed before it; a second copy the same day replaces the
//! first; `keep` removes only dated copies; and each refusal.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mnemo_engine::{
    BackupOptions, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, SharedOptions,
};

const PW: &str = "backup-test-passphrase";

/// The failpoint test arms a process-wide hook, so the tests here take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn v(x: f32) -> Vec<f32> {
    vec![x + 1.0, 1.0, 0.5, 0.25]
}

/// A brain with the manifest and `n` facts.
fn brain(dir: &Path, n: usize) -> PathBuf {
    let path = dir.join("brain").join("project.mnemo");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut db = Mnemo::create(&path, PW, cfg()).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    for i in 0..n {
        db.remember(Memory::new(
            format!("fact {i}"),
            MemoryType::Semantic,
            v(i as f32),
        ))
        .unwrap();
    }
    db.close().unwrap();
    path
}

fn backup(
    brain: &Path,
    dir: &Path,
    opts: &BackupOptions,
) -> mnemo_engine::Result<mnemo_engine::BackupReport> {
    let mut db = Mnemo::open_read_only_waiting(brain, PW, SharedOptions::default())?;
    let r = db.backup_to(dir, opts);
    db.close()?;
    r
}

/// The live memories in a copy, opened read-only and verified.
fn memories_in(copy: &Path) -> usize {
    let mut db = Mnemo::open_read_only(copy, PW).unwrap();
    db.verify().unwrap();
    let n = db.len();
    db.close().unwrap();
    n
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn a_backup_taken_while_a_shared_handle_writes_holds_every_committed_record() {
    let _g = serial();
    let tmp = tempfile::tempdir().unwrap();
    let path = brain(tmp.path(), 0);
    let out = tmp.path().join("backups");
    // Memories committed so far: the manifest, then each write's fact.
    let committed = Arc::new(AtomicUsize::new(1));
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (path, committed, stop) = (path.clone(), committed.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut s = Mnemo::open_shared(&path, PW, SharedOptions::default()).unwrap();
            let mut i = 0usize;
            while !stop.load(Ordering::SeqCst) && i < 400 {
                s.write(|db| {
                    db.remember(Memory::new(
                        format!("written {i}"),
                        MemoryType::Semantic,
                        v(i as f32),
                    ))
                    .map(|_| ())
                })
                .unwrap();
                i += 1;
                committed.store(1 + i, Ordering::SeqCst);
            }
            s.close().unwrap();
        })
    };
    let mut seen = Vec::new();
    for day in 1..=8 {
        let before = committed.load(Ordering::SeqCst);
        let date = format!("2026-11-{day:02}");
        let r = backup(&path, &out, &BackupOptions::default().keep(8).date(&date)).unwrap();
        assert_eq!(r.path, out.join(format!("mnemo-{date}.mnemo")));
        assert_eq!(r.bytes, std::fs::metadata(&r.path).unwrap().len());
        let n = memories_in(&r.path);
        assert!(
            n >= before,
            "the copy of {date} has {n} memories, {before} were committed"
        );
        // The check counts the copy's records (memories here: no cache).
        assert_eq!(r.records, n);
        seen.push(n);
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    stop.store(true, Ordering::SeqCst);
    writer.join().unwrap();
    // The writes went on between the copies.
    assert!(seen.first() < seen.last(), "{seen:?}");
    assert!(memories_in(&path) >= *seen.last().unwrap());
}

#[test]
fn a_second_backup_the_same_day_replaces_that_days_copy() {
    let _g = serial();
    let tmp = tempfile::tempdir().unwrap();
    let path = brain(tmp.path(), 2);
    let out = tmp.path().join("backups");
    let opts = BackupOptions::default().date("2026-11-24");
    let first = backup(&path, &out, &opts).unwrap();
    assert_eq!(memories_in(&first.path), 3);
    assert!(!first.replaced);
    let mut db = Mnemo::open(&path, PW).unwrap();
    db.remember(Memory::new("one more", MemoryType::Semantic, v(9.0)))
        .unwrap();
    db.close().unwrap();
    let second = backup(&path, &out, &opts).unwrap();
    assert_eq!(second.path, first.path);
    assert!(second.replaced);
    assert_eq!(second.kept, vec![first.path.clone()]);
    assert!(second.removed.is_empty());
    assert_eq!(memories_in(&second.path), 4);
    // No temp file is left beside it.
    assert_eq!(names(&out), vec!["mnemo-2026-11-24.mnemo"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&second.path), 0o600);
        assert_eq!(mode(&out), 0o700);
    }
}

#[test]
fn keep_removes_only_the_oldest_dated_copies() {
    let _g = serial();
    let tmp = tempfile::tempdir().unwrap();
    let path = brain(tmp.path(), 1);
    let out = tmp.path().join("backups");
    std::fs::create_dir(&out).unwrap();
    for name in [
        "mnemo-2026-11-20.mnemo",
        "mnemo-2026-11-21.mnemo",
        "mnemo-2026-11-22.mnemo",
        // Not dated copies: never touched.
        "notes.txt",
        "mnemo-latest.mnemo",
        "mnemo-2026-11-19.mnemo.bak",
        "project.mnemo",
    ] {
        std::fs::write(out.join(name), b"x").unwrap();
    }
    // A directory with a dated copy's name isn't a copy either.
    std::fs::create_dir(out.join("mnemo-2026-11-18.mnemo")).unwrap();

    let r = backup(&path, &out, &BackupOptions::default().date("2026-11-24")).unwrap();
    let full = |n: &str| out.join(n);
    assert_eq!(
        r.kept,
        vec![
            full("mnemo-2026-11-24.mnemo"),
            full("mnemo-2026-11-22.mnemo"),
            full("mnemo-2026-11-21.mnemo"),
        ]
    );
    assert_eq!(r.removed, vec![full("mnemo-2026-11-20.mnemo")]);
    assert_eq!(
        names(&out),
        vec![
            "mnemo-2026-11-18.mnemo",
            "mnemo-2026-11-19.mnemo.bak",
            "mnemo-2026-11-21.mnemo",
            "mnemo-2026-11-22.mnemo",
            "mnemo-2026-11-24.mnemo",
            "mnemo-latest.mnemo",
            "notes.txt",
            "project.mnemo",
        ]
    );

    // keep 1 with an older date than the others: the new copy still stays.
    let r = backup(
        &path,
        &out,
        &BackupOptions::default().keep(1).date("2026-11-01"),
    )
    .unwrap();
    assert_eq!(r.kept, vec![full("mnemo-2026-11-01.mnemo")]);
    assert_eq!(
        r.removed,
        vec![
            full("mnemo-2026-11-24.mnemo"),
            full("mnemo-2026-11-22.mnemo"),
            full("mnemo-2026-11-21.mnemo"),
        ]
    );
}

#[test]
fn the_refusals() {
    let _g = serial();
    let tmp = tempfile::tempdir().unwrap();
    let path = brain(tmp.path(), 1);
    let refused = |dir: &Path, opts: &BackupOptions| -> String {
        let e = backup(&path, dir, opts).unwrap_err().to_string();
        assert!(e.contains("backup refused"), "{e}");
        e
    };

    // Inside a git working tree, at any depth, `.git` itself included.
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    for dir in [
        repo.join("backups"),
        repo.join("a/b"),
        repo.join(".git/backups"),
    ] {
        let e = refused(&dir, &BackupOptions::default());
        assert!(e.contains("inside a git working tree"), "{e}");
        assert!(!dir.exists(), "{} was created", dir.display());
    }
    // The message names the directory absolute and plain: a `.` is dropped,
    // a `..` resolved, and nothing is created on the way.
    let e = refused(&repo.join(".").join("backups"), &BackupOptions::default());
    assert!(
        e.contains(&format!("{} is inside", repo.join("backups").display())),
        "{e}"
    );
    let e = refused(&repo.join("a/../backups"), &BackupOptions::default());
    assert!(!e.contains(".."), "{e}");
    #[cfg(unix)]
    {
        let real = std::fs::canonicalize(&repo).unwrap().join("backups");
        assert!(e.contains(&format!("{} is inside", real.display())), "{e}");
    }
    assert!(!repo.join("a").exists());
    // A worktree's `.git` is a file.
    let wt = tmp.path().join("worktree");
    std::fs::create_dir(&wt).unwrap();
    std::fs::write(wt.join(".git"), "gitdir: elsewhere\n").unwrap();
    let e = refused(&wt.join("backups"), &BackupOptions::default());
    assert!(e.contains("inside a git working tree"), "{e}");

    // The brain's own directory.
    let e = refused(path.parent().unwrap(), &BackupOptions::default());
    assert!(e.contains("the brain's own directory"), "{e}");

    // keep 0, a malformed date, a file where the directory should be.
    let out = tmp.path().join("backups");
    let e = refused(&out, &BackupOptions::default().keep(0));
    assert!(e.contains("at least 1"), "{e}");
    let e = refused(&out, &BackupOptions::default().date("24-11-2026"));
    assert!(e.contains("isn't YYYY-MM-DD"), "{e}");
    let file = tmp.path().join("a-file");
    std::fs::write(&file, b"x").unwrap();
    let e = refused(&file, &BackupOptions::default());
    assert!(e.contains("isn't a directory"), "{e}");
    assert!(!out.exists());
}

/// `mnemo <args>` with the test passphrase and no key store.
#[cfg(feature = "failpoints")]
fn mnemo(args: &[&str]) -> std::process::Output {
    use std::process::{Command, Stdio};
    Command::new(env!("CARGO_BIN_EXE_mnemo"))
        .args(args)
        .env("MNEMO_DEV_NO_KEYCHAIN", "1")
        .env("MNEMO_PASSPHRASE", PW)
        .env_remove("MNEMO_KEY")
        .env_remove("MNEMO_KEY_FILE")
        .env_remove("MNEMO_KEY_COMMAND")
        .env_remove("MNEMO_AGENT")
        .env_remove("CLAUDE_PROJECT_DIR")
        .stdin(Stdio::null())
        .output()
        .expect("run mnemo")
}

#[cfg(feature = "failpoints")]
#[test]
fn a_brain_with_unfinished_work_is_refused_with_the_fix() {
    let _g = serial();
    let tmp = tempfile::tempdir().unwrap();
    let base = brain(tmp.path(), 1);
    let out = tmp.path().join("backups");
    let mut found = false;
    for n in 0..40 {
        let path = tmp.path().join(format!("pending-{n}.mnemo"));
        std::fs::copy(&base, &path).unwrap();
        let mut db = Mnemo::open(&path, PW).unwrap();
        db.remember(Memory::new("pending", MemoryType::Semantic, v(2.0)))
            .unwrap();
        mnemo_engine::__failpoints::set_writes_until_fail(n);
        let _ = db.flush();
        mnemo_engine::__failpoints::set_writes_until_fail(-1);
        drop(db);
        if !matches!(
            Mnemo::open_read_only(&path, PW),
            Err(mnemo_engine::MnemoError::NeedsWriteOpen { .. })
        ) {
            continue;
        }
        let p = path.to_str().unwrap();
        let o = mnemo(&["backup", p, out.to_str().unwrap()]);
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(!o.status.success());
        assert!(
            err.contains("backup refused")
                && err.contains("left unfinished")
                && err.contains(&format!("mnemo verify --recover {p}")),
            "{err}"
        );
        assert!(!out.exists());
        // What the hint does, then the backup goes.
        let o = mnemo(&["verify", "--recover", p]);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let o = mnemo(&["backup", p, out.to_str().unwrap()]);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        found = true;
        break;
    }
    assert!(found, "no failpoint left work pending");
}
