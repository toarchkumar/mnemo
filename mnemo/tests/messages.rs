//! 13a step 2 (v0.17): messages between agents (13.1): `post`, `inbox`,
//! `mark_seen`, `ack`, the hop count with its pair carry, and pruning.

use mnemo_engine::{
    delivery, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, MnemoError, WriteOutcome,
    MESSAGE_BODY_MAX,
};
use serde_json::{json, Map};

const PW: &str = "messages-test-passphrase";

fn db(dir: &tempfile::TempDir) -> Mnemo {
    let cfg = MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    Mnemo::create(dir.path().join("m.mnemo"), PW, cfg).unwrap()
}

fn post(db: &mut Mnemo, from: &str, to: &str, subject: &str) -> mnemo_engine::Message {
    db.post(to, subject, "", &[], None, Some(from), None)
        .unwrap()
}

#[test]
fn addressing_reading_and_acking() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir);
    db.set_author(Some("planner-a".into()));
    let a = db
        .post(
            "worker-a",
            "step 1 dispatched",
            "the plumbing",
            &["PR 105".into()],
            None,
            None,
            Some("planner"),
        )
        .unwrap();
    assert_eq!(
        (a.from.as_str(), a.status.as_str(), a.delivery.as_str()),
        ("planner-a", "unread", "sent")
    );
    assert_eq!(a.body, "the plumbing");
    assert_eq!(a.refs, ["PR 105"]);
    assert_eq!(
        a.conversation,
        a.id.to_string(),
        "a new message starts its own conversation"
    );
    let b = post(&mut db, "planner-a", "role:worker", "for any worker");
    let c = post(&mut db, "reviewer-b", "all", "for everyone");
    let _d = post(&mut db, "worker-a", "all", "worker-a's own");
    let _e = post(&mut db, "planner-a", "reviewer-b", "not for workers");

    let ids = |v: Vec<mnemo_engine::Message>| v.into_iter().map(|m| m.id).collect::<Vec<_>>();
    // By id, by role, everyone; never its own.
    assert_eq!(
        ids(db.inbox(Some("worker-a"), Some("worker"), true).unwrap()),
        [a.id, b.id, c.id]
    );
    assert_eq!(
        ids(db.inbox(Some("worker-a"), None, true).unwrap()),
        [a.id, c.id]
    );
    assert_eq!(ids(db.inbox(None, None, true).unwrap()), [c.id, _d.id]);

    // Seen, then acked once.
    assert_eq!(db.mark_seen(&[a.id, b.id]).unwrap(), 2);
    assert_eq!(db.mark_seen(&[a.id]).unwrap(), 0);
    assert_eq!(
        db.get(&a.id).unwrap().metadata["_mnemo.delivery"],
        json!(delivery::SEEN)
    );
    assert_eq!(
        db.ack(&a.id, Some("on it"), Some("worker-a")).unwrap(),
        WriteOutcome::Updated
    );
    assert_eq!(db.ack(&a.id, None, None).unwrap(), WriteOutcome::Unchanged);
    let m = db.get(&a.id).unwrap();
    assert_eq!(m.metadata["_mnemo.status"], json!("acked"));
    assert_eq!(m.metadata["_mnemo.delivery"], json!(delivery::DONE));
    let h = &m.metadata["_mnemo.status_history"][0];
    assert_eq!(
        (h["by"].as_str(), h["note"].as_str()),
        (Some("worker-a"), Some("on it"))
    );
    assert_eq!(
        ids(db.inbox(Some("worker-a"), Some("worker"), true).unwrap()),
        [b.id, c.id]
    );
    assert_eq!(
        db.inbox(Some("worker-a"), Some("worker"), false)
            .unwrap()
            .len(),
        3
    );
    // It's one record for its life, and survives a reopen.
    let p = dir.path().join("m.mnemo");
    db.close().unwrap();
    let mut db = Mnemo::open(&p, PW).unwrap();
    assert_eq!(
        db.inbox(Some("worker-a"), Some("worker"), false)
            .unwrap()
            .len(),
        3
    );
    // Not a message.
    let fact = db
        .remember(Memory::new(
            "a fact",
            MemoryType::Semantic,
            vec![1.0, 0.0, 0.0, 0.0],
        ))
        .unwrap();
    assert!(matches!(
        db.ack(&fact, None, None),
        Err(MnemoError::Invalid(_))
    ));
}

#[test]
fn what_post_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir);
    for (to, subject, body) in [
        ("worker a", "s", ""),
        ("role:", "s", ""),
        ("role:bad name", "s", ""),
        ("worker-a", "", ""),
        ("worker-a", "two\nlines", ""),
        ("worker-a", &"x".repeat(201), ""),
        ("worker-a", "s", &"x".repeat(MESSAGE_BODY_MAX + 1)),
    ] {
        let r = db.post(to, subject, body, &[], None, Some("planner-a"), None);
        assert!(
            matches!(r, Err(MnemoError::Invalid(_))),
            "{to:?} {subject:?}"
        );
    }
    assert!(db
        .post(
            "worker-a",
            "s",
            &"x".repeat(MESSAGE_BODY_MAX),
            &[],
            None,
            None,
            None
        )
        .is_ok());
}

#[test]
fn the_hop_count_and_its_pair_carry() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir);
    // Replies in one conversation count up.
    let m0 = post(&mut db, "planner-a", "worker-a", "start");
    assert_eq!(m0.hops, 0);
    let m1 = db
        .post(
            "planner-a",
            "re: start",
            "",
            &[],
            Some(&m0.conversation),
            Some("worker-a"),
            None,
        )
        .unwrap();
    assert_eq!(m1.hops, 1);
    let m2 = db
        .post(
            "worker-a",
            "re: re",
            "",
            &[],
            Some(&m0.conversation),
            Some("planner-a"),
            None,
        )
        .unwrap();
    assert_eq!(
        (m2.hops, m2.conversation.clone()),
        (2, m0.conversation.clone())
    );
    // A fresh message between the same pair, within 10 minutes, carries on
    // (either way), so trading new messages still reaches the cap.
    let f = post(&mut db, "worker-a", "planner-a", "a fresh one");
    assert_ne!(f.conversation, m0.conversation);
    assert_eq!(f.hops, 3);
    let g = post(&mut db, "planner-a", "worker-a", "and another");
    assert_eq!(g.hops, 4);
    // Another pair starts at 0.
    assert_eq!(post(&mut db, "reviewer-b", "worker-a", "unrelated").hops, 0);
}

#[test]
fn acked_messages_are_pruned_and_compaction_drops_old_ones() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir);
    let a = post(&mut db, "planner-a", "worker-a", "acked");
    let _b = post(&mut db, "planner-a", "worker-a", "unread");
    db.ack(&a.id, None, None).unwrap();
    // Just acked: kept at 30 days; anything acked goes at "-1 seconds".
    assert_eq!(db.prune_messages(30 * 86_400).unwrap(), 0);
    assert_eq!(db.prune_messages(-1).unwrap(), 1);
    assert_eq!(db.inbox(Some("worker-a"), None, false).unwrap().len(), 1);
    // A message acked 31 days ago is left out by compaction; one acked
    // today and an unread one stay.
    let old = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64)
        - 31 * 86_400;
    let mut meta = Map::new();
    for (k, v) in [
        ("_mnemo.to", json!("worker-a")),
        ("_mnemo.from", json!("planner-a")),
        ("_mnemo.status", json!("acked")),
        ("_mnemo.delivery", json!("done")),
        ("_mnemo.acked_at", json!(old)),
    ] {
        meta.insert(k.into(), v);
    }
    db.__put_coordination_for_testing("message", "an old acked one", meta)
        .unwrap();
    let recent = post(&mut db, "planner-a", "worker-a", "acked today");
    db.ack(&recent.id, None, None).unwrap();
    db.close().unwrap();
    let p = dir.path().join("m.mnemo");
    let report = Mnemo::compact_file(p.to_str().unwrap(), PW).unwrap();
    let _ = report;
    let mut db = Mnemo::open(&p, PW).unwrap();
    let left: Vec<String> = db
        .inbox(Some("worker-a"), None, false)
        .unwrap()
        .into_iter()
        .map(|m| m.subject)
        .collect();
    assert_eq!(left, ["unread", "acked today"]);
}
