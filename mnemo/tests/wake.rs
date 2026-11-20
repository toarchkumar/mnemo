//! 13a step 5 (v0.17): what's new for an agent (`new_work`, `mark_shown`)
//! and the wake-up guardrails (`wake`): only pre-approved tasks and
//! messages to the agent by id; the kill switch; the hop cap for explicit
//! replies; the pair burst; the per-recipient rate and cooldown.

use mnemo_engine::{
    KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, MnemoError, TaskSpec, Wake, WakeItems,
    WriteOutcome, HOP_CAP, PAIR_BURST, WAKE_COOLDOWN_SECS, WAKE_PER_HOUR,
};

const PW: &str = "wake-test-passphrase";

fn db(dir: &tempfile::TempDir, manifest: bool) -> Mnemo {
    let cfg = MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    let mut db = Mnemo::create(dir.path().join("w.mnemo"), PW, cfg).unwrap();
    if manifest {
        db.remember(Memory::scaffold_manifest(4)).unwrap();
    }
    db.remember(Memory::new(
        "a fact",
        MemoryType::Semantic,
        vec![1.0, 0.0, 0.0, 0.0],
    ))
    .unwrap();
    db
}

fn task(db: &mut Mnemo, title: &str, pre: bool) {
    db.set_author(Some("planner-a".into()));
    db.task_add(&{
        let mut built = TaskSpec::default();
        built.title = title.into();
        built.role = Some("worker".into());
        built.pre_approved = pre;
        built
    })
    .unwrap();
}

#[test]
fn only_pre_approved_tasks_and_messages_by_id_wake_and_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir, true);
    assert_eq!(db.wake("worker-a", Some("worker")).unwrap(), Wake::Nothing);
    task(&mut db, "Needs a sign-off", false);
    db.post(
        "role:worker",
        "to the role",
        "",
        &[],
        None,
        Some("planner-a"),
        None,
    )
    .unwrap();
    db.post("all", "to everyone", "", &[], None, Some("planner-a"), None)
        .unwrap();
    assert_eq!(db.wake("worker-a", Some("worker")).unwrap(), Wake::Nothing);
    task(&mut db, "Small fix", true);
    db.post("worker-a", "to you", "", &[], None, Some("planner-a"), None)
        .unwrap();
    assert_eq!(
        db.wake("worker-a", Some("worker")).unwrap(),
        Wake::Now {
            tasks: 1,
            messages: 1
        }
    );
    // Delivered: not again, even past the cooldown.
    db.__set_clock_offset_for_testing(WAKE_COOLDOWN_SECS + 1);
    assert_eq!(db.wake("worker-a", Some("worker")).unwrap(), Wake::Nothing);
    // Its own messages never wake it.
    db.post(
        "worker-a",
        "note to self",
        "",
        &[],
        None,
        Some("worker-a"),
        None,
    )
    .unwrap();
    assert_eq!(db.wake("worker-a", Some("worker")).unwrap(), Wake::Nothing);
}

#[test]
fn the_cooldown_and_the_hourly_limit() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir, true);
    db.post("worker-a", "m0", "", &[], None, Some("planner-a"), None)
        .unwrap();
    assert!(matches!(
        db.wake("worker-a", None).unwrap(),
        Wake::Now { .. }
    ));
    db.post("worker-a", "m1", "", &[], None, Some("reviewer-b"), None)
        .unwrap();
    assert_eq!(
        db.wake("worker-a", None).unwrap(),
        Wake::Held("the cooldown between wake-ups")
    );
    let mut t = 0;
    for i in 1..WAKE_PER_HOUR {
        t += WAKE_COOLDOWN_SECS;
        db.__set_clock_offset_for_testing(t);
        assert!(
            matches!(db.wake("worker-a", None).unwrap(), Wake::Now { .. }),
            "wake {i}"
        );
        db.post(
            "worker-a",
            &format!("m{}", i + 1),
            "",
            &[],
            None,
            Some(&format!("agent-{i}")),
            None,
        )
        .unwrap();
    }
    // Twelve in the hour: the thirteenth waits for the oldest to age out.
    t += WAKE_COOLDOWN_SECS;
    db.__set_clock_offset_for_testing(t);
    assert_eq!(
        db.wake("worker-a", None).unwrap(),
        Wake::Held("the hourly limit of wake-ups")
    );
    db.__set_clock_offset_for_testing(3_600 + 1);
    assert!(matches!(
        db.wake("worker-a", None).unwrap(),
        Wake::Now { .. }
    ));
}

#[test]
fn a_burst_between_two_agents_and_a_reply_past_the_hop_cap_wake_nobody() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir, true);
    // A loop: planner-a and worker-a trade PAIR_BURST messages in a minute.
    for i in 0..PAIR_BURST {
        let (from, to) = if i % 2 == 0 {
            ("planner-a", "worker-a")
        } else {
            ("worker-a", "planner-a")
        };
        db.post(to, &format!("ping {i}"), "", &[], None, Some(from), None)
            .unwrap();
    }
    assert_eq!(
        db.wake("worker-a", None).unwrap(),
        Wake::Held("a burst of messages between two agents")
    );
    // Ten minutes on, the burst is over: a fresh message wakes.
    db.__set_clock_offset_for_testing(601);
    let mut later = db;
    later
        .post(
            "worker-a",
            "after the burst",
            "",
            &[],
            None,
            Some("planner-a"),
            None,
        )
        .unwrap();
    assert!(
        matches!(later.wake("worker-a", None).unwrap(), Wake::Now { messages, .. } if messages >= 1)
    );

    // An explicit reply chain past the cap, slow enough to be no burst.
    let dir = tempfile::tempdir().unwrap();
    let mut db = self::db(&dir, true);
    let first = db
        .post("worker-b", "start", "", &[], None, Some("reviewer-b"), None)
        .unwrap();
    let mut t = 0;
    let mut last = first.clone();
    while last.hops <= HOP_CAP {
        t += 3_600; // an hour apart: no burst, no cooldown, no hourly limit
        db.__set_clock_offset_for_testing(t);
        let (from, to) = if last.from == "reviewer-b" {
            ("worker-b", "reviewer-b")
        } else {
            ("reviewer-b", "worker-b")
        };
        last = db
            .post(
                to,
                "re",
                "",
                &[],
                Some(&first.conversation),
                Some(from),
                None,
            )
            .unwrap();
        // Everything before it was delivered or is someone else's.
        let _ = db.wake("worker-b", None).unwrap();
        let _ = db.wake("reviewer-b", None).unwrap();
    }
    assert!(last.hops > HOP_CAP);
    let w = db.wake(&last.to.clone(), None).unwrap();
    assert_eq!(w, Wake::Held("the thread reached its hop cap"));
}

/// v0.20.1 (RB-017.1, RB-017.4): `wake_items` names what `wake` counts,
/// and `may_wake` reads the same candidates on a read-only handle.
#[test]
fn wake_items_names_the_items_and_may_wake_reads_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir, true);
    assert!(!db.may_wake("worker-a", Some("worker")).unwrap());
    assert_eq!(
        db.wake_items("worker-a", Some("worker")).unwrap(),
        WakeItems::Nothing
    );
    task(&mut db, "Small fix", true);
    let msg = db
        .post("worker-a", "to you", "", &[], None, Some("planner-a"), None)
        .unwrap();
    db.post(
        "role:worker",
        "to the role",
        "",
        &[],
        None,
        Some("planner-a"),
        None,
    )
    .unwrap();
    db.close().unwrap();
    let path = dir.path().join("w.mnemo");
    {
        let mut ro = Mnemo::open_read_only(&path, PW).unwrap();
        assert!(ro.may_wake("worker-a", Some("worker")).unwrap());
        assert!(!ro.may_wake("worker-b", None).unwrap());
        assert!(matches!(
            ro.wake_items("worker-a", Some("worker")),
            Err(MnemoError::ReadOnly)
        ));
    }
    let mut db = Mnemo::open(&path, PW).unwrap();
    match db.wake_items("worker-a", Some("worker")).unwrap() {
        WakeItems::Now { tasks, messages } => {
            assert_eq!(
                tasks.iter().map(|t| t.slug.as_str()).collect::<Vec<_>>(),
                ["small-fix"]
            );
            assert_eq!(messages.iter().map(|m| m.id).collect::<Vec<_>>(), [msg.id]);
            assert_eq!(messages[0].from, "planner-a");
        }
        other => panic!("{other:?}"),
    }
    // Delivered: nothing may wake it now, and `wake` agrees.
    assert!(!db.may_wake("worker-a", Some("worker")).unwrap());
    assert_eq!(db.wake("worker-a", Some("worker")).unwrap(), Wake::Nothing);
    // A guardrail holds it the same way for both.
    db.post("worker-a", "again", "", &[], None, Some("planner-a"), None)
        .unwrap();
    assert!(db.may_wake("worker-a", Some("worker")).unwrap());
    assert_eq!(
        db.wake_items("worker-a", Some("worker")).unwrap(),
        WakeItems::Held("the cooldown between wake-ups")
    );
}

#[test]
fn the_kill_switch() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir, true);
    db.set_author(Some("planner-a".into()));
    assert_eq!(db.wake_paused().unwrap(), None);
    assert_eq!(db.set_wake_paused(true).unwrap(), WriteOutcome::Updated);
    assert_eq!(db.set_wake_paused(true).unwrap(), WriteOutcome::Unchanged);
    assert_eq!(db.wake_paused().unwrap().unwrap().0, "planner-a");
    db.post(
        "worker-a",
        "while paused",
        "",
        &[],
        None,
        Some("planner-a"),
        None,
    )
    .unwrap();
    assert_eq!(db.wake("worker-a", None).unwrap(), Wake::Held("paused"));
    // Paused, the items stay sent, and the per-turn view still shows them.
    assert_eq!(
        db.new_work(Some("worker-a"), None).unwrap().messages.len(),
        1
    );
    assert_eq!(db.set_wake_paused(false).unwrap(), WriteOutcome::Updated);
    assert!(matches!(
        db.wake("worker-a", None).unwrap(),
        Wake::Now { .. }
    ));
    // No manifest: refused, and never paused.
    let dir = tempfile::tempdir().unwrap();
    let mut bare = self::db(&dir, false);
    assert!(matches!(
        bare.set_wake_paused(true),
        Err(MnemoError::Invalid(_))
    ));
    assert_eq!(bare.wake_paused().unwrap(), None);
}

#[test]
fn new_work_is_shown_once_and_a_lapsing_claim_once_per_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir, true);
    assert!(db
        .new_work(Some("worker-a"), Some("worker"))
        .unwrap()
        .is_empty());
    task(&mut db, "Step 5", false);
    task(&mut db, "Step 6", false);
    db.post("worker-a", "to you", "", &[], None, Some("planner-a"), None)
        .unwrap();
    db.post(
        "role:worker",
        "to the role",
        "",
        &[],
        None,
        Some("planner-a"),
        None,
    )
    .unwrap();
    db.post(
        "reviewer-b",
        "not for you",
        "",
        &[],
        None,
        Some("planner-a"),
        None,
    )
    .unwrap();
    let w = db.new_work(Some("worker-a"), Some("worker")).unwrap();
    assert_eq!(
        (w.messages.len(), w.tasks.len(), w.lapsing.len()),
        (2, 2, 0)
    );
    db.mark_shown(&w).unwrap();
    assert!(db
        .new_work(Some("worker-a"), Some("worker"))
        .unwrap()
        .is_empty());
    // A task taken: its claim lapses within 15 minutes 1 h 46 min later,
    // shown once; renewed, it's shown again when the new expiry nears.
    db.set_author(Some("worker-a".into()));
    db.task_next(Some("worker")).unwrap().unwrap();
    db.__set_clock_offset_for_testing(2 * 3_600 - 14 * 60);
    let w = db.new_work(Some("worker-a"), Some("worker")).unwrap();
    assert_eq!(w.lapsing.len(), 1, "{w:?}");
    assert_eq!(w.lapsing[0].topic, "task:step-5");
    db.mark_shown(&w).unwrap();
    assert!(db
        .new_work(Some("worker-a"), Some("worker"))
        .unwrap()
        .lapsing
        .is_empty());
    db.task_update("step-5", None, Some("still going"), None)
        .unwrap();
    assert!(db
        .new_work(Some("worker-a"), Some("worker"))
        .unwrap()
        .lapsing
        .is_empty());
    db.__set_clock_offset_for_testing(2 * (2 * 3_600) - 20 * 60);
    assert_eq!(
        db.new_work(Some("worker-a"), Some("worker"))
            .unwrap()
            .lapsing
            .len(),
        1
    );
}

/// RB-012.6 item 4 (SEEN-PER-AGENT): a message to a role or everyone, and
/// a role's ready task, are new to each agent until it has been shown them.
#[test]
fn broadcasts_and_role_tasks_are_seen_per_agent() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir, true);
    task(&mut db, "For any worker", false);
    db.post(
        "role:worker",
        "to the role",
        "",
        &[],
        None,
        Some("planner-a"),
        None,
    )
    .unwrap();
    db.post("all", "to everyone", "", &[], None, Some("planner-a"), None)
        .unwrap();
    db.post("worker-a", "to a", "", &[], None, Some("planner-a"), None)
        .unwrap();
    let shown = |db: &mut Mnemo, who: &str| {
        db.set_author(Some(who.into()));
        let w = db.new_work(Some(who), Some("worker")).unwrap();
        let n = (w.messages.len(), w.tasks.len());
        db.mark_shown(&w).unwrap();
        n
    };
    assert_eq!(shown(&mut db, "worker-a"), (3, 1));
    assert_eq!(shown(&mut db, "worker-a"), (0, 0));
    // worker-b still sees the broadcasts and the role's task.
    assert_eq!(shown(&mut db, "worker-b"), (2, 1));
    assert_eq!(shown(&mut db, "worker-b"), (0, 0));
}

/// READY-NOTICE (v0.17.2): a task its creator must close shows to the
/// creator once each time it becomes `ready`, never to another agent, and
/// never wakes a session.
#[test]
fn a_ready_task_is_new_to_its_creator_once_per_ready() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir, true);
    task(&mut db, "Ship it", false);
    // Queued: nothing for the creator yet.
    assert!(db
        .new_work(Some("planner-a"), Some("planner"))
        .unwrap()
        .is_empty());
    db.set_author(Some("worker-a".into()));
    let w = db.new_work(Some("worker-a"), Some("worker")).unwrap();
    db.mark_shown(&w).unwrap();
    db.task_next(Some("worker")).unwrap().unwrap();
    db.task_update("ship-it", Some("ready"), Some("PR 1"), None)
        .unwrap();
    // The creator: once.
    let w = db.new_work(Some("planner-a"), Some("planner")).unwrap();
    assert_eq!(w.ready_for_done.len(), 1, "{w:?}");
    assert_eq!(w.ready_for_done[0].slug, "ship-it");
    assert!(w.tasks.is_empty() && w.messages.is_empty());
    db.mark_shown(&w).unwrap();
    assert!(db
        .new_work(Some("planner-a"), Some("planner"))
        .unwrap()
        .is_empty());
    // Not the worker's, nor another agent's.
    for (a, r) in [("worker-a", "worker"), ("reviewer-b", "reviewer")] {
        assert!(
            db.new_work(Some(a), Some(r))
                .unwrap()
                .ready_for_done
                .is_empty(),
            "{a}"
        );
    }
    // It wakes nobody: it waits for a person's review.
    assert_eq!(
        db.wake("planner-a", Some("planner")).unwrap(),
        Wake::Nothing
    );
    // A creator of the task's own role was shown it queued (`seen_by`);
    // ready, it's still new to it as one to close.
    db.set_author(Some("worker-a".into()));
    db.task_add(&{
        let mut built = TaskSpec::default();
        built.title = "Tidy the tests".into();
        built.role = Some("worker".into());
        built
    })
    .unwrap();
    let w = db.new_work(Some("worker-a"), Some("worker")).unwrap();
    assert_eq!(w.tasks.len(), 1, "{w:?}");
    db.mark_shown(&w).unwrap();
    db.task_next(Some("worker")).unwrap().unwrap();
    db.task_update("tidy-the-tests", Some("ready"), None, None)
        .unwrap();
    let w = db.new_work(Some("worker-a"), Some("worker")).unwrap();
    assert_eq!(w.ready_for_done.len(), 1, "{w:?}");
    assert_eq!(w.ready_for_done[0].slug, "tidy-the-tests");
    db.mark_shown(&w).unwrap();
    assert!(db
        .new_work(Some("worker-a"), Some("worker"))
        .unwrap()
        .is_empty());
    // Done: gone from the list.
    db.set_author(Some("planner-a".into()));
    db.task_update("ship-it", Some("done"), None, None).unwrap();
    let t = db.task("ship-it").unwrap().unwrap();
    assert_eq!(t.status, "done");
}
