//! v0.21.0 step 1 (spec.v0210, 13.7): the merge groundwork. Every write and
//! in-place update from this build carries `_mnemo.hlc`, which never goes
//! backwards; a copy of a record already in a file (compaction) keeps its
//! stamp. A delete leaves a tombstone, an engine record with the deleted id,
//! its key slot and the time and none of its content, which compaction
//! keeps and recall, `about`'s counts and the git export leave out.
//!
//! Step 1b (RB-019.1, .2, .7, .8): a turn's tombstone names its thread and
//! turn id; a message prune leaves none; the manifest records the first
//! stamp a v0.21 build wrote (`_mnemo.hlc_since`); a dev-hooks clock lets a
//! test run a copy ahead, behind or in the same millisecond; and a stamp far
//! ahead of the clock is gone past and said once.

use mnemo_engine::{
    EssenceScope, EssenceSection, KdfParams, Memory, MemoryPatch, MemoryType, Mnemo, MnemoConfig,
    RecallRequest, ThreadTurn, Ulid, HLC_AHEAD_NOTICE_MS, MNEMO_META_DELETED_AT, MNEMO_META_HLC,
    MNEMO_META_HLC_SINCE, MNEMO_META_KIND, MNEMO_META_TOMBSTONE_KEY, MNEMO_META_TOMBSTONE_OF,
    MNEMO_META_TOMBSTONE_OWNER, MNEMO_META_TOMBSTONE_THREAD, MNEMO_META_TOMBSTONE_TURN,
};
use serde_json::Value;

const PW: &str = "merge-groundwork-test-passphrase";

fn path(dir: &tempfile::TempDir) -> String {
    dir.path().join("m.mnemo").to_str().unwrap().to_string()
}

fn create(dir: &tempfile::TempDir) -> Mnemo {
    let cfg = MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    Mnemo::create(path(dir), PW, cfg).unwrap()
}

fn hlc(m: &Memory) -> u64 {
    m.metadata
        .get(MNEMO_META_HLC)
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("no _mnemo.hlc on {m:?}"))
}

fn tombstones(db: &mut Mnemo) -> Vec<Memory> {
    db.memories()
        .unwrap()
        .into_iter()
        .filter(|m| m.metadata.get(MNEMO_META_KIND).and_then(Value::as_str) == Some("tombstone"))
        .collect()
}

fn fact(db: &mut Mnemo, text: &str, v: f32) -> Ulid {
    db.remember(Memory::new(
        text,
        MemoryType::Semantic,
        vec![v, 1.0, 0.0, 0.0],
    ))
    .unwrap()
}

#[test]
fn every_write_and_update_is_stamped_and_stamps_only_go_up() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = create(&dir);
    let ids: Vec<Ulid> = (0..50)
        .map(|i| fact(&mut db, &format!("fact {i}"), i as f32 + 1.0))
        .collect();
    let stamps: Vec<u64> = ids.iter().map(|id| hlc(&db.get(id).unwrap())).collect();
    assert!(stamps.windows(2).all(|w| w[0] < w[1]), "{stamps:?}");
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    assert!(
        stamps[0] + 60_000 > now_ms && stamps[0] <= now_ms + 1_000,
        "wall ms: {stamps:?}"
    );
    // An update in place stamps again, past the record's own stamp and
    // every stamp this handle has written.
    let before = hlc(&db.get(&ids[0]).unwrap());
    db.update(
        &ids[0],
        MemoryPatch {
            importance: Some(0.9),
            ..Default::default()
        },
    )
    .unwrap();
    let after = hlc(&db.get(&ids[0]).unwrap());
    assert!(
        after > before && after > *stamps.last().unwrap(),
        "{before} {after}"
    );
    // A keyed write: the new version and the superseded one are stamped,
    // each past the last.
    let (v1, _) = db
        .remember_keyed(
            Memory::new("order: a", MemoryType::Episodic, vec![9.0, 1.0, 0.0, 0.0]),
            "decision.order",
        )
        .unwrap();
    let h1 = hlc(&db.get(&v1).unwrap());
    let (v2, _) = db
        .remember_keyed(
            Memory::new("order: b", MemoryType::Episodic, vec![8.0, 1.0, 0.0, 0.0]),
            "decision.order",
        )
        .unwrap();
    assert!(hlc(&db.get(&v2).unwrap()) > h1);
    assert!(
        hlc(&db.get(&v1).unwrap()) > h1,
        "the superseded version was rewritten"
    );
    // Essence and coordination writes too.
    let item = db
        .essence_add(EssenceScope::Project, EssenceSection::Next, "ship", None)
        .unwrap();
    let hi = hlc(&db.get(&item).unwrap());
    db.essence_resolve(&item).unwrap();
    assert!(hlc(&db.get(&item).unwrap()) > hi);
    // A record copied in with a stamp an hour ahead of this clock (as from
    // a copy whose clock runs fast; the hook copies it as compaction does).
    let ahead = now_ms + 3_600_000;
    let mut m = Memory::new(
        "from a fast clock",
        MemoryType::Semantic,
        vec![7.0, 1.0, 0.0, 0.0],
    );
    m.metadata.insert(MNEMO_META_HLC.into(), ahead.into());
    let fast = db.__put_unvalidated_for_testing(m).unwrap();
    assert_eq!(
        hlc(&db.get(&fast).unwrap()),
        ahead,
        "a copy keeps its stamp"
    );
    db.close().unwrap();

    // A new handle goes past what it reads: its first write is past every
    // stamp in the file that it has read, the one ahead of the wall clock
    // too.
    let mut db = Mnemo::open(path(&dir), PW).unwrap();
    let max = db.memories().unwrap().iter().map(hlc).max().unwrap();
    assert_eq!(max, ahead);
    let next = fact(&mut db, "after a reopen", 99.0);
    assert!(hlc(&db.get(&next).unwrap()) > max);
    db.close().unwrap();

    // Compaction copies records with their stamps.
    let stamps_before: Vec<(Ulid, u64)> = {
        let mut db = Mnemo::open(path(&dir), PW).unwrap();
        let mut v: Vec<(Ulid, u64)> = db
            .memories()
            .unwrap()
            .iter()
            .map(|m| (m.id, hlc(m)))
            .collect();
        v.sort();
        db.close().unwrap();
        v
    };
    Mnemo::compact_file(&path(&dir), PW).unwrap();
    let mut db = Mnemo::open(path(&dir), PW).unwrap();
    let mut stamps_after: Vec<(Ulid, u64)> = db
        .memories()
        .unwrap()
        .iter()
        .map(|m| (m.id, hlc(m)))
        .collect();
    stamps_after.sort();
    assert_eq!(stamps_after, stamps_before);
}

#[test]
fn a_delete_leaves_a_tombstone_with_no_content() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = create(&dir);
    let plain = fact(&mut db, "a secret that must stay gone: hunter2", 1.0);
    let (keyed, _) = db
        .remember_keyed(
            Memory::new(
                "staging resets on Mondays",
                MemoryType::Semantic,
                vec![2.0, 1.0, 0.0, 0.0],
            )
            .with_agent("cursor-agent"),
            "fact.staging",
        )
        .unwrap();
    let keyed_hlc = hlc(&db.get(&keyed).unwrap());
    let counts_before = db.kind_counts().unwrap();
    db.delete(&plain).unwrap();
    db.delete(&keyed).unwrap();
    // Deleting again is a no-op, as before, and leaves no second tombstone.
    db.delete(&plain).unwrap();
    let ts = tombstones(&mut db);
    assert_eq!(ts.len(), 2, "{ts:?}");
    let of = |t: &Memory| {
        t.metadata[MNEMO_META_TOMBSTONE_OF]
            .as_str()
            .unwrap()
            .to_string()
    };
    let t_plain = ts.iter().find(|t| of(t) == plain.to_string()).unwrap();
    let t_keyed = ts.iter().find(|t| of(t) == keyed.to_string()).unwrap();
    for t in &ts {
        assert_eq!(t.content, "", "a tombstone holds no content");
        assert!(t.metadata[MNEMO_META_DELETED_AT].is_i64());
        assert_eq!(t.memory_type, MemoryType::Working);
        let text = serde_json::to_string(&t.metadata).unwrap();
        assert!(
            !text.contains("hunter2") && !text.contains("Mondays"),
            "{text}"
        );
    }
    assert!(t_plain.metadata.get(MNEMO_META_TOMBSTONE_KEY).is_none());
    assert_eq!(t_keyed.metadata[MNEMO_META_TOMBSTONE_KEY], "fact.staging");
    assert_eq!(t_keyed.metadata[MNEMO_META_TOMBSTONE_OWNER], "cursor-agent");
    assert!(
        hlc(t_keyed) > keyed_hlc,
        "the tombstone's stamp passes the deleted record's"
    );
    // A tombstone stays.
    let e = db.delete(&t_keyed.id).unwrap_err().to_string();
    assert!(e.contains("tombstone"), "{e}");
    // Out of recall and of about's counts, counted on their own.
    let hits = db
        .recall(
            &RecallRequest::new(vec![1.0, 1.0, 0.0, 0.0])
                .top_k(50)
                .track_access(false),
        )
        .unwrap();
    assert!(hits
        .iter()
        .all(|h| h.memory.metadata.get(MNEMO_META_TOMBSTONE_OF).is_none()));
    let c = db.kind_counts().unwrap();
    assert_eq!(c.tombstones, 2);
    assert_eq!(c.coordination, counts_before.coordination);
    assert_eq!(c.facts, counts_before.facts - 2);
    db.close().unwrap();
    // Compaction keeps them; the deleted records stay gone.
    Mnemo::compact_file(&path(&dir), PW).unwrap();
    let mut db = Mnemo::open(path(&dir), PW).unwrap();
    let after: Vec<String> = tombstones(&mut db).iter().map(of).collect();
    assert_eq!(after.len(), 2, "{after:?}");
    assert!(db.get(&plain).is_err() && db.get(&keyed).is_err());
    assert_eq!(db.kind_counts().unwrap().tombstones, 2);
}

#[test]
fn essence_remove_and_thread_forget_leave_tombstones_and_a_prune_leaves_none() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = create(&dir);
    let item = db
        .essence_add(
            EssenceScope::Project,
            EssenceSection::Open,
            "a flaky test",
            None,
        )
        .unwrap();
    db.essence_remove(&item).unwrap();
    let turns: Vec<ThreadTurn> = (0..3)
        .map(|i| ThreadTurn {
            turn_id: format!("u-{i}"),
            role: "user".into(),
            content: format!("turn {i}"),
            timestamp: None,
        })
        .collect();
    let vecs: Vec<Vec<f32>> = (0..3)
        .map(|i| vec![i as f32 + 1.0, 1.0, 0.0, 0.0])
        .collect();
    db.ingest_thread_with_vectors("t1", &turns, &vecs).unwrap();
    let forgotten = db.thread_forget("t1").unwrap();
    assert!(forgotten >= 3, "{forgotten}");
    // An acked message, pruned at once (`keep` below zero).
    let msg = db
        .post(
            "planner-a",
            "DONE: x",
            "",
            &[],
            None,
            Some("worker-a"),
            None,
        )
        .unwrap();
    db.ack(&msg.id, None, Some("planner-a")).unwrap();
    assert_eq!(db.prune_messages(-1).unwrap(), 1);
    assert!(db.get(&msg.id).is_err(), "the message is gone");
    let ts = tombstones(&mut db);
    // RB-019.7: the prune left none (a merge drops what compaction drops).
    assert_eq!(ts.len(), 1 + forgotten, "{}", ts.len());
    assert!(ts
        .iter()
        .any(|t| t.metadata[MNEMO_META_TOMBSTONE_OF] == item.to_string()));
    assert!(ts
        .iter()
        .all(|t| t.metadata[MNEMO_META_TOMBSTONE_OF] != msg.id.to_string()));
    // RB-019.2: each turn's names its thread and turn id; the essence
    // item's names neither.
    let mut turn_ids: Vec<&str> = ts
        .iter()
        .filter_map(|t| {
            let thread = t.metadata.get(MNEMO_META_TOMBSTONE_THREAD)?;
            assert_eq!(thread, "t1");
            t.metadata.get(MNEMO_META_TOMBSTONE_TURN)?.as_str()
        })
        .collect();
    turn_ids.sort();
    assert_eq!(turn_ids, ["u-0", "u-1", "u-2"]);
    let essence_t = ts
        .iter()
        .find(|t| t.metadata[MNEMO_META_TOMBSTONE_OF] == item.to_string())
        .unwrap();
    assert!(essence_t
        .metadata
        .get(MNEMO_META_TOMBSTONE_THREAD)
        .is_none());
    // No content of a turn is on its tombstone.
    for t in &ts {
        let text = serde_json::to_string(&t.metadata).unwrap();
        assert!(!text.contains("turn 1"), "{text}");
    }
}

#[test]
fn a_single_turn_delete_names_its_turn_too() {
    // Through `delete`, not `thread_forget`, with the handle caches cold.
    let dir = tempfile::tempdir().unwrap();
    let mut db = create(&dir);
    let turns = [ThreadTurn {
        turn_id: "a-7".into(),
        role: "assistant".into(),
        content: "the answer".into(),
        timestamp: None,
    }];
    db.ingest_thread_with_vectors("t2", &turns, &[vec![1.0, 2.0, 0.0, 0.0]])
        .unwrap();
    let id = db
        .memories()
        .unwrap()
        .into_iter()
        .find(|m| m.metadata.get(MNEMO_META_KIND).and_then(Value::as_str) == Some("turn"))
        .expect("one turn written")
        .id;
    db.close().unwrap();
    let mut db = Mnemo::open(path(&dir), PW).unwrap();
    db.delete(&id).unwrap();
    let ts = tombstones(&mut db);
    assert_eq!(ts.len(), 1);
    assert_eq!(ts[0].metadata[MNEMO_META_TOMBSTONE_THREAD], "t2");
    assert_eq!(ts[0].metadata[MNEMO_META_TOMBSTONE_TURN], "a-7");
}

fn since(db: &mut Mnemo) -> Option<u64> {
    db.about_in(mnemo_engine::VisibilitySet::ALL)
        .unwrap()
        .into_iter()
        .find(|m| m.metadata.get("topic").and_then(Value::as_str) == Some("manifest"))
        .and_then(|m| m.metadata.get(MNEMO_META_HLC_SINCE).and_then(Value::as_u64))
}

#[test]
fn the_manifest_records_the_first_stamp_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = create(&dir);
    let manifest = db.remember(Memory::scaffold_manifest(4)).unwrap();
    let first = hlc(&db.get(&manifest).unwrap());
    db.flush().unwrap();
    assert_eq!(since(&mut db), Some(first));
    fact(&mut db, "a later fact", 2.0);
    db.close().unwrap();
    // Later handles leave it as it is, and so does compaction.
    let mut db = Mnemo::open(path(&dir), PW).unwrap();
    fact(&mut db, "another fact", 3.0);
    db.close().unwrap();
    Mnemo::compact_file(&path(&dir), PW).unwrap();
    let mut db = Mnemo::open(path(&dir), PW).unwrap();
    assert_eq!(since(&mut db), Some(first));
    // A read-only open writes nothing.
    db.close().unwrap();
    let before = std::fs::read(path(&dir)).unwrap();
    let mut ro = Mnemo::open_read_only(path(&dir), PW).unwrap();
    assert_eq!(since(&mut ro), Some(first));
    drop(ro);
    assert_eq!(std::fs::read(path(&dir)).unwrap(), before);
}

#[test]
fn a_file_with_no_manifest_gets_the_field_with_its_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = create(&dir);
    fact(&mut db, "before any manifest", 1.0);
    db.flush().unwrap();
    assert_eq!(since(&mut db), None);
    let manifest = db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.close().unwrap();
    let mut db = Mnemo::open(path(&dir), PW).unwrap();
    let got = since(&mut db).expect("set at the commit that wrote the manifest");
    assert!(got <= hlc(&db.get(&manifest).unwrap()));
}

#[test]
fn a_new_manifest_keeps_the_first_stamp() {
    // The live manifest lacks the field (an older build wrote it, or a
    // caller built it afresh): the earliest an older one held is kept.
    let dir = tempfile::tempdir().unwrap();
    let mut db = create(&dir);
    db.remember_keyed(Memory::scaffold_manifest(4), "manifest")
        .unwrap();
    db.close().unwrap();
    let mut db = Mnemo::open(path(&dir), PW).unwrap();
    let first = since(&mut db).unwrap();
    let mut newer = Memory::scaffold_manifest(4);
    newer.content = "MNEMO MANIFEST, rewritten".into();
    let (id, _) = db.remember_keyed(newer, "manifest").unwrap();
    db.close().unwrap();
    let mut db = Mnemo::open(path(&dir), PW).unwrap();
    let m = db.get(&id).unwrap();
    assert_eq!(m.metadata[MNEMO_META_HLC_SINCE], first);
    assert_eq!(since(&mut db), Some(first));
}

fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[test]
fn the_test_clock_runs_a_copy_ahead_behind_or_in_the_same_millisecond() {
    const HOUR: u64 = 3_600_000;
    // The same millisecond on two copies, then one apart.
    let (da, db_) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let at = 1_900_000_000_000u64;
    let (mut a, mut b) = (create(&da), create(&db_));
    a.__freeze_hlc_clock_for_testing(Some(at));
    b.__freeze_hlc_clock_for_testing(Some(at));
    let (ia, ib) = (fact(&mut a, "in a", 1.0), fact(&mut b, "in b", 1.0));
    assert_eq!(hlc(&a.get(&ia).unwrap()), at);
    assert_eq!(hlc(&b.get(&ib).unwrap()), at);
    let ia2 = fact(&mut a, "in a again", 2.0);
    assert_eq!(
        hlc(&a.get(&ia2).unwrap()),
        at + 1,
        "a held clock still rises"
    );
    // An hour ahead, and an hour behind, of the wall clock.
    let dc = tempfile::tempdir().unwrap();
    let mut c = create(&dc);
    c.__set_hlc_offset_for_testing(HOUR as i64);
    let before = wall_ms();
    let ic = fact(&mut c, "ahead", 1.0);
    let h = hlc(&c.get(&ic).unwrap());
    assert!(h >= before + HOUR && h <= wall_ms() + HOUR, "{h}");
    let dd = tempfile::tempdir().unwrap();
    let mut d = create(&dd);
    d.__set_hlc_offset_for_testing(-(HOUR as i64));
    let before = wall_ms();
    let id_ = fact(&mut d, "behind", 1.0);
    let h = hlc(&d.get(&id_).unwrap());
    assert!(h + HOUR >= before && h + HOUR <= wall_ms(), "{h}");
    // Behind its own clock, a copy still stamps past what it has seen.
    d.__set_hlc_offset_for_testing(-(3 * HOUR as i64));
    let id2 = fact(&mut d, "further behind", 2.0);
    assert!(hlc(&d.get(&id2).unwrap()) > h);
}

#[test]
fn a_stamp_far_ahead_is_gone_past_and_said_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = create(&dir);
    let ahead = wall_ms() + 3 * HLC_AHEAD_NOTICE_MS;
    let mut m = Memory::new("x", MemoryType::Semantic, vec![1.0, 1.0, 0.0, 0.0]);
    m.metadata.insert(MNEMO_META_HLC.into(), ahead.into());
    let id = db.__put_unvalidated_for_testing(m).unwrap();
    db.close().unwrap();
    let mut db = Mnemo::open(path(&dir), PW).unwrap();
    assert_eq!(db.clock_lead_ms(), 0, "nothing read yet");
    db.get(&id).unwrap();
    let lead = db.clock_lead_ms();
    assert!(
        lead > 2 * HLC_AHEAD_NOTICE_MS && lead <= 3 * HLC_AHEAD_NOTICE_MS,
        "{lead}"
    );
    assert!(db.notices().is_empty(), "said at a write, not a read");
    let n = fact(&mut db, "after it", 2.0);
    assert!(hlc(&db.get(&n).unwrap()) > ahead);
    fact(&mut db, "and again", 3.0);
    let said: Vec<&String> = db
        .notices()
        .iter()
        .filter(|n| n.contains("ahead of this computer's clock"))
        .collect();
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].contains("3 days") || said[0].contains("2 days"),
        "{}",
        said[0]
    );
}

#[test]
fn a_cli_write_says_once_when_a_stamp_is_far_ahead() {
    let dir = tempfile::tempdir().unwrap();
    create(&dir).close().unwrap();
    let p = path(&dir);
    let post = |offset_ms: Option<u64>| {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_mnemo"));
        c.args([
            "post",
            "planner-a",
            "DONE: x",
            "--brain",
            &p,
            "--as",
            "worker-a",
        ])
        .env("MNEMO_PASSPHRASE", PW)
        .env_remove("MNEMO_AGENT")
        .env_remove("MNEMO_ROLE")
        .env_remove("MNEMO_DEV_HLC_OFFSET_MS");
        if let Some(o) = offset_ms {
            c.env("MNEMO_DEV_HLC_OFFSET_MS", o.to_string());
        }
        let out = c.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    // A copy whose clock ran three days ahead wrote one message.
    let ahead = post(Some(3 * HLC_AHEAD_NOTICE_MS));
    assert!(!ahead.contains("ahead of this computer's clock"), "{ahead}");
    // This one reads it before it writes, and says so once.
    let err = post(None);
    assert_eq!(
        err.matches("ahead of this computer's clock").count(),
        1,
        "{err}"
    );
}

#[test]
fn a_rewrite_from_another_handle_goes_past_the_records_stamp() {
    // A record stamped ahead of the wall clock, read by one handle and
    // rewritten by another that never read it: the rewrite still goes past.
    let dir = tempfile::tempdir().unwrap();
    let mut db = create(&dir);
    let ahead = 4_000_000_000_000u64;
    let mut m = Memory::new("x", MemoryType::Semantic, vec![1.0, 1.0, 0.0, 0.0]);
    m.metadata.insert(MNEMO_META_HLC.into(), ahead.into());
    let id = db.__put_unvalidated_for_testing(m).unwrap();
    db.close().unwrap();
    let mut reader = Mnemo::open(path(&dir), PW).unwrap();
    let mut m = reader.get(&id).unwrap();
    reader.close().unwrap();
    let mut writer = Mnemo::open(path(&dir), PW).unwrap();
    m.content = "x, rewritten".into();
    writer.remember(m).unwrap();
    assert!(hlc(&writer.get(&id).unwrap()) > ahead);
}
