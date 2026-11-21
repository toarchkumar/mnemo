//! v0.21.0 step 1 (spec.v0210, 13.7): the merge groundwork. Every write and
//! in-place update from this build carries `_mnemo.hlc`, which never goes
//! backwards; a copy of a record already in a file (compaction) keeps its
//! stamp. A delete leaves a tombstone, an engine record with the deleted id,
//! its key slot and the time and none of its content, which compaction
//! keeps and recall, `about`'s counts and the git export leave out.

use mnemo_engine::{
    EssenceScope, EssenceSection, KdfParams, Memory, MemoryPatch, MemoryType, Mnemo, MnemoConfig,
    RecallRequest, ThreadTurn, Ulid, MNEMO_META_DELETED_AT, MNEMO_META_HLC, MNEMO_META_KIND,
    MNEMO_META_TOMBSTONE_KEY, MNEMO_META_TOMBSTONE_OF, MNEMO_META_TOMBSTONE_OWNER,
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
    let counts_before = db.kind_counts();
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
    let c = db.kind_counts();
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
    assert_eq!(db.kind_counts().tombstones, 2);
}

#[test]
fn essence_remove_thread_forget_and_message_prunes_leave_tombstones() {
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
    let ts = tombstones(&mut db);
    assert_eq!(ts.len(), 2 + forgotten, "{}", ts.len());
    for gone in [item, msg.id] {
        assert!(ts
            .iter()
            .any(|t| t.metadata[MNEMO_META_TOMBSTONE_OF] == gone.to_string()));
    }
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
