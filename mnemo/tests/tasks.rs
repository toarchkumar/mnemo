//! 13a step 4 (v0.17): tasks (13.1): `task_add`, `task_next` (which claims
//! `task:<slug>`), `task_update`, `tasks`, `task`; `after` and
//! `pre_approved`; R-010.7's `active → queued`, the reclaim of a task whose
//! owner's claim lapsed, and a lost owner's update refused; and a
//! crash-and-reclaim test with a real process killed mid-task.

use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::{Command, Stdio};
use std::time::Duration;

#[cfg(unix)]
use mnemo_engine::task_status;
use mnemo_engine::{
    KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, MnemoError, SharedOptions, TaskOutcome,
    TaskSpec, WriteOutcome, TASK_CLAIM_TTL,
};

const PW: &str = "tasks-test-passphrase";

fn brain(dir: &Path) -> PathBuf {
    let p = dir.join("t.mnemo");
    let cfg = MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    let mut db = Mnemo::create(&p, PW, cfg).unwrap();
    db.remember(Memory::new(
        "a fact",
        MemoryType::Semantic,
        vec![1.0, 0.0, 0.0, 0.0],
    ))
    .unwrap();
    db.close().unwrap();
    p
}

fn spec(title: &str, role: Option<&str>, after: &[&str]) -> TaskSpec {
    TaskSpec {
        title: title.into(),
        role: role.map(str::to_string),
        after: after.iter().map(|a| a.to_string()).collect(),
        ..Default::default()
    }
}

fn as_agent(db: &mut Mnemo, a: &str) {
    db.set_author(Some(a.into()));
}

#[test]
fn add_next_update_list_and_get() {
    let dir = tempfile::tempdir().unwrap();
    let p = brain(dir.path());
    let mut db = Mnemo::open(&p, PW).unwrap();
    as_agent(&mut db, "planner-a");
    let (s1, o) = db
        .task_add(&spec("Step 1: the plumbing", Some("worker"), &[]))
        .unwrap();
    assert_eq!(
        (s1.slug.as_str(), o),
        ("step-1-the-plumbing", WriteOutcome::Added)
    );
    assert_eq!(
        (s1.status.as_str(), s1.creator.as_str()),
        ("queued", "planner-a")
    );
    let (s2, _) = db
        .task_add(&TaskSpec {
            body: "messages".into(),
            slug: Some("step-2".into()),
            ..spec("Step 2", Some("worker"), &["step-1-the-plumbing"])
        })
        .unwrap();
    let (_r, _) = db
        .task_add(&spec("Review step 1", Some("reviewer"), &[]))
        .unwrap();
    // Re-adding a slug updates it in place, or is unchanged.
    let (_, o) = db
        .task_add(&TaskSpec {
            body: "messages".into(),
            slug: Some("step-2".into()),
            ..spec("Step 2", Some("worker"), &["step-1-the-plumbing"])
        })
        .unwrap();
    assert_eq!(o, WriteOutcome::Unchanged);
    let (u, o) = db
        .task_add(&TaskSpec {
            body: "messages, with the hop count".into(),
            slug: Some("step-2".into()),
            ..spec("Step 2", Some("worker"), &["step-1-the-plumbing"])
        })
        .unwrap();
    assert_eq!(
        (o, u.id, u.body.as_str()),
        (WriteOutcome::Updated, s2.id, "messages, with the hop count")
    );
    // What add refuses.
    for bad in [
        spec("", None, &[]),
        spec("x", None, &["no-such-task"]),
        TaskSpec {
            slug: Some("Bad Slug".into()),
            ..spec("x", None, &[])
        },
        TaskSpec {
            slug: Some("self".into()),
            ..spec("x", None, &["self"])
        },
        spec("x", Some("bad role"), &[]),
        spec("!!!", None, &[]),
    ] {
        assert!(
            matches!(db.task_add(&bad), Err(MnemoError::Invalid(_))),
            "{bad:?}"
        );
    }
    // next: by role, oldest first, only once its `after` tasks are done.
    as_agent(&mut db, "worker-a");
    let t = db.task_next(Some("worker")).unwrap().unwrap();
    assert_eq!(t.task.slug, "step-1-the-plumbing");
    assert_eq!(
        (t.task.status.as_str(), t.task.owner.as_deref()),
        ("active", Some("worker-a"))
    );
    assert!(t.reclaimed_from.is_none());
    assert_eq!(t.token.len(), 32);
    // step-2 waits for step 1; the reviewer's isn't a worker's.
    assert!(db.task_next(Some("worker")).unwrap().is_none());
    assert_eq!(db.ready_tasks(Some("reviewer")).unwrap().len(), 1);
    // The owner: notes and status changes renew the claim.
    assert!(matches!(
        db.task_update("step-1-the-plumbing", None, Some("halfway"), None)
            .unwrap(),
        TaskOutcome::Updated(_)
    ));
    assert!(matches!(
        db.task_update(
            "step-1-the-plumbing",
            Some("blocked"),
            Some("CI down"),
            Some(&t.token)
        )
        .unwrap(),
        TaskOutcome::Updated(_)
    ));
    assert!(matches!(
        db.task_update("step-1-the-plumbing", Some("active"), None, None)
            .unwrap(),
        TaskOutcome::Updated(_)
    ));
    // A wrong token is Lost; a skipped transition is refused.
    assert_eq!(
        db.task_update("step-1-the-plumbing", Some("ready"), None, Some("nope"))
            .unwrap(),
        TaskOutcome::Lost
    );
    assert!(matches!(
        db.task_update("step-1-the-plumbing", Some("done"), None, None),
        Err(MnemoError::Invalid(_))
    ));
    // Another agent is Lost; queued tasks are taken only with next.
    as_agent(&mut db, "worker-b");
    assert_eq!(
        db.task_update("step-1-the-plumbing", Some("ready"), None, None)
            .unwrap(),
        TaskOutcome::Lost
    );
    assert!(matches!(
        db.task_update("step-2", Some("active"), None, None),
        Err(MnemoError::Invalid(_))
    ));
    // ready waits for its creator's done.
    as_agent(&mut db, "worker-a");
    let TaskOutcome::Updated(r) = db
        .task_update("step-1-the-plumbing", Some("ready"), None, None)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(r.status, "ready");
    assert!(
        db.claims()
            .unwrap()
            .iter()
            .all(|c| c.topic != "task:step-1-the-plumbing"),
        "ready releases the claim"
    );
    assert!(matches!(
        db.task_update("step-1-the-plumbing", Some("done"), None, None),
        Err(MnemoError::Invalid(e)) if e.contains("creator")
    ));
    as_agent(&mut db, "planner-a");
    let TaskOutcome::Updated(d) = db
        .task_update("step-1-the-plumbing", Some("done"), Some("merged"), None)
        .unwrap()
    else {
        panic!()
    };
    let hist: Vec<(String, String)> = d
        .history
        .iter()
        .map(|h| {
            (
                h["from"].as_str().unwrap().into(),
                h["to"].as_str().unwrap().into(),
            )
        })
        .collect();
    let want = [
        ("queued", "active"),
        ("active", "active"),
        ("active", "blocked"),
        ("blocked", "active"),
        ("active", "ready"),
        ("ready", "done"),
    ];
    assert_eq!(hist, want.map(|(a, b)| (a.to_string(), b.to_string())));
    // Now step 2 is ready; a pre_approved task's owner sets done itself.
    as_agent(&mut db, "worker-a");
    assert_eq!(
        db.task_next(Some("worker")).unwrap().unwrap().task.slug,
        "step-2"
    );
    as_agent(&mut db, "planner-a");
    db.task_add(&TaskSpec {
        pre_approved: true,
        ..spec("Small fix", None, &[])
    })
    .unwrap();
    as_agent(&mut db, "worker-b");
    let t = db.task_next(Some("worker")).unwrap().unwrap();
    assert_eq!(
        t.task.slug, "small-fix",
        "a task with no role is any role's"
    );
    db.task_update("small-fix", Some("ready"), None, None)
        .unwrap();
    assert!(matches!(
        db.task_update("small-fix", Some("done"), None, None)
            .unwrap(),
        TaskOutcome::Updated(_)
    ));
    // The owner hands step 2 back (R-010.7): queued, no owner, claim gone.
    as_agent(&mut db, "worker-a");
    let TaskOutcome::Updated(q) = db
        .task_update("step-2", Some("queued"), Some("handing back"), None)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!((q.status.as_str(), q.owner.as_deref()), ("queued", None));
    assert_eq!(
        db.task_next(Some("worker")).unwrap().unwrap().task.slug,
        "step-2"
    );
    // Lists, and a reopen.
    assert_eq!(db.tasks(Some("done"), None).unwrap().len(), 2);
    assert_eq!(
        db.tasks(None, Some("reviewer")).unwrap().len(),
        2,
        "with the no-role task"
    );
    db.close().unwrap();
    let mut db = Mnemo::open(&p, PW).unwrap();
    assert_eq!(
        db.task("step-2").unwrap().unwrap().owner.as_deref(),
        Some("worker-a")
    );
    assert!(db.task("nope").unwrap().is_none());
    // Never a recall hit, never in about.
    assert!(db
        .about()
        .unwrap()
        .iter()
        .all(|m| !mnemo_engine::is_coordination(m)));
}

#[test]
fn a_lapsed_owner_is_reclaimed_and_its_late_update_is_lost() {
    let dir = tempfile::tempdir().unwrap();
    let p = brain(dir.path());
    let mut db = Mnemo::open(&p, PW).unwrap();
    as_agent(&mut db, "planner-a");
    db.task_add(&spec("Step 4", Some("worker"), &[])).unwrap();
    db.task_add(&spec("Later", Some("worker"), &[])).unwrap();
    as_agent(&mut db, "worker-a");
    let a = db.task_next(Some("worker")).unwrap().unwrap();
    // While worker-a's claim is live, worker-b gets the next queued one.
    as_agent(&mut db, "worker-b");
    assert_eq!(
        db.task_next(Some("worker")).unwrap().unwrap().task.slug,
        "later"
    );
    // Two hours on: worker-a's claim lapsed; worker-b takes step 4 back.
    db.__set_clock_offset_for_testing(TASK_CLAIM_TTL + 1);
    let b = db.task_next(Some("worker")).unwrap().unwrap();
    assert_eq!(b.task.slug, a.task.slug);
    assert_eq!(b.reclaimed_from.as_deref(), Some("worker-a"));
    let last = b.task.history.last().unwrap();
    assert_eq!(
        (last["by"].as_str(), last["reclaimed_from"].as_str()),
        (Some("worker-b"), Some("worker-a"))
    );
    // worker-a wakes up: Lost, with or without its token; nothing changed.
    as_agent(&mut db, "worker-a");
    for tk in [None, Some(a.token.as_str())] {
        assert_eq!(
            db.task_update("step-4", Some("ready"), None, tk).unwrap(),
            TaskOutcome::Lost
        );
    }
    assert_eq!(
        db.task("step-4").unwrap().unwrap().owner.as_deref(),
        Some("worker-b")
    );
    // An owner whose claim lapsed with nobody taking it carries on (and
    // takes the claim again).
    as_agent(&mut db, "worker-b");
    db.__set_clock_offset_for_testing(3 * TASK_CLAIM_TTL);
    assert!(matches!(
        db.task_update("later", None, Some("still on it"), None)
            .unwrap(),
        TaskOutcome::Updated(_)
    ));
    assert!(db
        .claims()
        .unwrap()
        .iter()
        .any(|c| c.topic == "task:later" && c.holder == "worker-b"));
    // An abandoned task can be requeued by anyone once its claim lapsed.
    db.__set_clock_offset_for_testing(5 * TASK_CLAIM_TTL);
    as_agent(&mut db, "planner-a");
    let TaskOutcome::Updated(q) = db
        .task_update("step-4", Some("queued"), Some("abandoned"), None)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(q.status, "queued");
}

// --- Crash and reclaim: a real process killed holding a task -------------------

const ROLE: &str = "MNEMO_TASKS_ROLE";

fn opts() -> SharedOptions {
    let mut o = SharedOptions::default();
    o.deadline = Duration::from_secs(10);
    o
}

/// The child: takes the next task as worker-a with a presence lock (as a
/// long-lived MCP server would), says so, and waits to be killed.
#[test]
fn tasks_child() {
    let Ok(_) = std::env::var(ROLE) else {
        return;
    };
    let path = PathBuf::from(std::env::var("TK_PATH").unwrap());
    let mut s = Mnemo::open_shared(&path, PW, opts()).unwrap();
    s.set_author(Some("worker-a".into()));
    s.hold_presence().unwrap();
    let t = s.write(|db| db.task_next(Some("worker"))).unwrap().unwrap();
    std::fs::write(std::env::var("TK_OUT").unwrap(), &t.token).unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(unix)]
#[test]
fn a_crashed_owners_task_is_reclaimed_and_it_cant_overwrite_the_new_owner() {
    let dir = tempfile::tempdir().unwrap();
    let p = brain(dir.path());
    {
        let mut db = Mnemo::open(&p, PW).unwrap();
        as_agent(&mut db, "planner-a");
        db.task_add(&spec("Step 4", Some("worker"), &[])).unwrap();
        db.close().unwrap();
    }
    let out = dir.path().join("token");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "tasks_child",
            "--exact",
            "--nocapture",
            "--test-threads=1",
            "-q",
        ])
        .env(ROLE, "1")
        .env("TK_PATH", &p)
        .env("TK_OUT", &out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while !out.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the child never took the task"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(50));
    let a_token = std::fs::read_to_string(&out).unwrap();
    // Alive: worker-b can't take it.
    let mut b = Mnemo::open_shared(&p, PW, opts()).unwrap();
    b.set_author(Some("worker-b".into()));
    assert!(b
        .write(|db| db.task_next(Some("worker")))
        .unwrap()
        .is_none());
    // kill -9: its presence lock is free. The first look records it, and
    // ten seconds later the task is taken back, well before its 2 h lease.
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(b
        .write(|db| db.task_next(Some("worker")))
        .unwrap()
        .is_none());
    drop(b);
    let mut db = Mnemo::open(&p, PW).unwrap();
    as_agent(&mut db, "worker-b");
    db.__set_clock_offset_for_testing(10);
    let t = db.task_next(Some("worker")).unwrap().unwrap();
    assert_eq!(t.reclaimed_from.as_deref(), Some("worker-a"));
    // worker-a, restarted without its task, can't overwrite worker-b's.
    as_agent(&mut db, "worker-a");
    assert_eq!(
        db.task_update(
            "step-4",
            Some("ready"),
            Some("done, I think"),
            Some(&a_token)
        )
        .unwrap(),
        TaskOutcome::Lost
    );
    as_agent(&mut db, "worker-b");
    let TaskOutcome::Updated(r) = db
        .task_update("step-4", Some("ready"), None, Some(&t.token))
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        (r.status.as_str(), r.owner.as_deref()),
        (task_status::READY, Some("worker-b"))
    );
}
