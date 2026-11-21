//! v0.21.0 step 2 (spec.v0210 rev 2, 13.7): `Mnemo::merge_files`. Two
//! copies of one brain diverge; the merge keeps both sides' writes, applies
//! each side's deletes by id, and resolves the same record written on both
//! sides by its kind. The same inputs the other way round give the same
//! records, and merging the output again with an input changes nothing.

use std::path::{Path, PathBuf};

use mnemo_engine::{
    CachePutOpts, Embedder, EmbedderIdentity, EssenceScope, EssenceSection, FindingSpec, KdfParams,
    KeyScope, Memory, MemoryPatch, MemoryType, MergeOptions, MergeReport, MergeSecret, MergeSide,
    Mnemo, MnemoConfig, RecallRequest, TaskSpec, ThreadTurn, Ulid, VisibilitySet,
    MERGE_LEAD_WARN_MS, MNEMO_META_MERGE_CONFLICT, MNEMO_META_MERGE_SOURCE,
};
use serde_json::Value;

const PW: &str = "merge-test-passphrase";

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn v(x: f32) -> Vec<f32> {
    vec![x, 1.0, 0.5, 0.25]
}

/// A brain with a manifest and one fact, closed. Returns the fact's id.
fn base(path: &Path) -> Ulid {
    let mut db = Mnemo::create(path, PW, cfg()).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    let id = db
        .remember(Memory::new("the base fact", MemoryType::Semantic, v(1.0)))
        .unwrap();
    db.close().unwrap();
    id
}

fn copy(from: &Path, to: &Path) {
    std::fs::copy(from, to).unwrap();
}

fn open(p: &Path) -> Mnemo {
    Mnemo::open(p, PW).unwrap()
}

fn merge(a: &Path, b: &Path, out: &Path, now: u64) -> MergeReport {
    Mnemo::merge_files(
        a,
        MergeSecret::Passphrase(PW),
        b,
        MergeSecret::Passphrase(PW),
        out,
        &MergeOptions::at(now),
    )
    .unwrap()
}

/// The records a merge reads from `p`, as JSON, by id, without claims and
/// presence (they come from the first input only).
fn records(p: &Path) -> Vec<Value> {
    let mut db = Mnemo::open_read_only(p, PW).unwrap();
    let (mut recs, _) = db.__merge_input_for_testing().unwrap();
    recs.retain(|m| {
        !matches!(
            m.metadata.get("_mnemo.kind").and_then(Value::as_str),
            Some("claim" | "presence")
        )
    });
    recs.sort_by_key(|m| m.id);
    recs.iter()
        .map(|m| {
            let mut j = serde_json::to_value(m).unwrap();
            // Vectors as the fp16 column keeps them, read back alike.
            j.as_object_mut().unwrap().remove("vector");
            j
        })
        .collect()
}

fn mirrored(r: &MergeReport) -> MergeReport {
    let mut m = r.clone();
    std::mem::swap(&mut m.a_only, &mut m.b_only);
    m.lead_ms.swap(0, 1);
    m.hlc_since.swap(0, 1);
    for k in &mut m.kept_keys {
        k.deleted_in = k.deleted_in.other();
    }
    m.kept_keys.sort();
    m
}

struct Pair {
    _dir: tempfile::TempDir,
    a: PathBuf,
    b: PathBuf,
    base_fact: Ulid,
}

fn pair() -> Pair {
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.mnemo");
    let base_fact = base(&base_path);
    let (a, b) = (dir.path().join("a.mnemo"), dir.path().join("b.mnemo"));
    copy(&base_path, &a);
    copy(&base_path, &b);
    Pair {
        _dir: dir,
        a,
        b,
        base_fact,
    }
}

impl Pair {
    fn out(&self, name: &str) -> PathBuf {
        self.a.with_file_name(name)
    }

    /// merge(a, b) and merge(b, a) give the same records and mirrored
    /// reports; merging the output with either input again changes nothing.
    fn check_laws(&self, now: u64) -> (PathBuf, MergeReport) {
        let (ab, ba) = (self.out("ab.mnemo"), self.out("ba.mnemo"));
        let r_ab = merge(&self.a, &self.b, &ab, now);
        let r_ba = merge(&self.b, &self.a, &ba, now);
        assert_eq!(records(&ab), records(&ba), "merge(a,b) = merge(b,a)");
        assert_eq!(mirrored(&r_ab), r_ba, "the report, mirrored");
        for (name, other) in [("abb.mnemo", &self.b), ("aba.mnemo", &self.a)] {
            let again = self.out(name);
            merge(&ab, other, &again, now);
            assert_eq!(
                records(&again),
                records(&ab),
                "merge(merge(a,b), x) = merge(a,b)"
            );
        }
        (ab, r_ab)
    }
}

fn meta<'a>(m: &'a Memory, k: &str) -> Option<&'a Value> {
    m.metadata.get(k)
}

#[test]
fn writes_on_both_sides_are_all_kept() {
    let p = pair();
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    let xa = a
        .remember(Memory::new("written in a", MemoryType::Semantic, v(2.0)))
        .unwrap();
    let xb = b
        .remember(Memory::new("written in b", MemoryType::Semantic, v(3.0)))
        .unwrap();
    a.close().unwrap();
    b.close().unwrap();
    let (out, r) = p.check_laws(wall_ms());
    assert_eq!(r.a_only, vec![xa]);
    assert_eq!(r.b_only, vec![xb]);
    assert!(r.conflicts.is_empty() && r.removed.is_empty());
    let mut db = Mnemo::open(&out, PW).unwrap();
    for id in [xa, xb, p.base_fact] {
        db.get(&id).unwrap();
    }
    assert_eq!(
        db.snapshots().len(),
        1,
        "the output starts with one snapshot"
    );
}

#[test]
fn an_edit_on_both_sides_keeps_the_newer_and_a_conflict_copy() {
    let p = pair();
    let t = wall_ms();
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    a.__freeze_hlc_clock_for_testing(Some(t));
    b.__freeze_hlc_clock_for_testing(Some(t + 1_000));
    let edit = |db: &mut Mnemo, text: &str| {
        db.update(
            &p.base_fact,
            MemoryPatch {
                content: Some(text.into()),
                vector: Some(v(5.0)),
                ..Default::default()
            },
        )
        .unwrap();
    };
    edit(&mut a, "edited in a");
    edit(&mut b, "edited in b, later");
    a.close().unwrap();
    b.close().unwrap();
    let (out, r) = p.check_laws(t + 2_000);
    assert_eq!(r.conflicts.get("fact"), Some(&1));
    let mut db = Mnemo::open(&out, PW).unwrap();
    assert_eq!(db.get(&p.base_fact).unwrap().content, "edited in b, later");
    let copies: Vec<Memory> = db
        .memories()
        .unwrap()
        .into_iter()
        .filter(|m| meta(m, MNEMO_META_MERGE_SOURCE).is_some())
        .collect();
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].content, "edited in a");
    assert_eq!(
        meta(&copies[0], MNEMO_META_MERGE_CONFLICT),
        Some(&Value::Bool(true))
    );
    assert_eq!(r.conflict_ids, vec![copies[0].id]);
}

#[test]
fn a_key_written_on_both_sides_is_one_chain_with_the_superseded_side_flagged() {
    let p = pair();
    let t = wall_ms();
    {
        let mut a = open(&p.a);
        a.remember_keyed(Memory::new("k is zero", MemoryType::Semantic, v(1.0)), "k")
            .unwrap();
        a.close().unwrap();
        copy(&p.a, &p.b);
    }
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    a.__freeze_hlc_clock_for_testing(Some(t));
    b.__freeze_hlc_clock_for_testing(Some(t + 1_000));
    let (va, _) = a
        .remember_keyed(Memory::new("k is a", MemoryType::Semantic, v(2.0)), "k")
        .unwrap();
    let (vb, _) = b
        .remember_keyed(Memory::new("k is b", MemoryType::Semantic, v(3.0)), "k")
        .unwrap();
    a.close().unwrap();
    b.close().unwrap();
    let (out, r) = p.check_laws(t + 2_000);
    assert_eq!(r.conflicts.get("key"), Some(&1));
    assert_eq!(r.conflict_ids, vec![va]);
    let mut db = Mnemo::open(&out, PW).unwrap();
    let h = db.history_in("k", KeyScope::private_default()).unwrap();
    let texts: Vec<&str> = h.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(texts, ["k is b", "k is a", "k is zero"]);
    assert_eq!(h[0].id, vb);
    assert_eq!(
        meta(&h[1], MNEMO_META_MERGE_CONFLICT),
        Some(&Value::Bool(true))
    );
}

#[test]
fn the_same_value_written_on_both_sides_is_no_conflict() {
    let p = pair();
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    for db in [&mut a, &mut b] {
        db.remember_keyed(Memory::new("same", MemoryType::Semantic, v(2.0)), "k")
            .unwrap();
    }
    a.close().unwrap();
    b.close().unwrap();
    let (out, r) = p.check_laws(wall_ms());
    assert!(r.conflicts.is_empty(), "{:?}", r.conflicts);
    let mut db = Mnemo::open(&out, PW).unwrap();
    let h = db.history_in("k", KeyScope::private_default()).unwrap();
    assert_eq!(h.len(), 2);
    assert!(h
        .iter()
        .all(|m| meta(m, MNEMO_META_MERGE_CONFLICT).is_none()));
}

#[test]
fn a_forget_on_one_side_removes_the_record_and_the_others_edit() {
    let p = pair();
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    a.delete(&p.base_fact).unwrap();
    b.update(
        &p.base_fact,
        MemoryPatch {
            content: Some("edited where it wasn't forgotten".into()),
            vector: Some(v(4.0)),
            ..Default::default()
        },
    )
    .unwrap();
    a.close().unwrap();
    b.close().unwrap();
    let (out, r) = p.check_laws(wall_ms());
    assert_eq!(r.removed, vec![p.base_fact]);
    assert!(r.conflicts.is_empty());
    let mut db = Mnemo::open(&out, PW).unwrap();
    assert!(db.get(&p.base_fact).is_err());
    assert!(db
        .memories()
        .unwrap()
        .iter()
        .all(|m| !m.content.contains("forgotten")));
}

#[test]
fn a_key_deleted_on_one_side_keeps_the_version_the_other_wrote() {
    let p = pair();
    let k0 = {
        let mut a = open(&p.a);
        let (k0, _) = a
            .remember_keyed(Memory::new("k is zero", MemoryType::Semantic, v(1.0)), "k")
            .unwrap();
        a.close().unwrap();
        copy(&p.a, &p.b);
        k0
    };
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    a.delete(&k0).unwrap();
    let (k1, _) = b
        .remember_keyed(Memory::new("k is one", MemoryType::Semantic, v(2.0)), "k")
        .unwrap();
    a.close().unwrap();
    b.close().unwrap();
    let (out, r) = p.check_laws(wall_ms());
    assert_eq!(r.removed, vec![k0]);
    assert_eq!(r.kept_keys.len(), 1);
    assert_eq!(r.kept_keys[0].key, "k");
    assert_eq!(r.kept_keys[0].deleted_in, MergeSide::A);
    assert_eq!(r.kept_keys[0].kept, vec![k1]);
    let mut db = Mnemo::open(&out, PW).unwrap();
    let h = db.history_in("k", KeyScope::private_default()).unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].id, k1);
}

#[test]
fn done_is_final_and_two_tasks_under_one_slug_are_both_kept() {
    let p = pair();
    {
        let mut a = open(&p.a);
        a.set_author(Some("planner-a".into()));
        let mut spec = TaskSpec::new("Ship it");
        spec.role = Some("worker".into());
        a.task_add(&spec).unwrap();
        a.close().unwrap();
        copy(&p.a, &p.b);
    }
    let t = wall_ms();
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    a.set_author(Some("worker-a".into()));
    a.task_next(Some("worker")).unwrap().unwrap();
    a.task_update("ship-it", Some("ready"), None, None).unwrap();
    a.set_author(Some("planner-a".into()));
    a.task_update("ship-it", Some("done"), Some("shipped"), None)
        .unwrap();
    // b never saw it done: a worker takes it later.
    b.__set_clock_offset_for_testing(60);
    b.set_author(Some("worker-b".into()));
    b.task_next(Some("worker")).unwrap().unwrap();
    // Each side adds a task under one new slug.
    for (db, title) in [(&mut a, "Write the notes"), (&mut b, "Write the notes")] {
        db.set_author(Some("planner-a".into()));
        let mut spec = TaskSpec::new(title);
        spec.role = Some("worker".into());
        db.task_add(&spec).unwrap();
    }
    a.close().unwrap();
    b.close().unwrap();
    let (out, r) = p.check_laws(t + 120_000);
    let mut db = Mnemo::open(&out, PW).unwrap();
    let tasks = db.tasks(None, None).unwrap();
    let ship = tasks.iter().find(|t| t.slug == "ship-it").unwrap();
    assert_eq!(ship.status, "done");
    let notes: Vec<&str> = tasks
        .iter()
        .filter(|t| t.title == "Write the notes")
        .map(|t| t.slug.as_str())
        .collect();
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(notes.contains(&"write-the-notes") && notes.contains(&"write-the-notes-merge-2"));
    assert_eq!(r.renamed.len(), 1);
    assert_eq!(r.renamed[0].to, "write-the-notes-merge-2");
}

#[test]
fn an_essence_item_resolved_on_one_side_is_resolved() {
    let p = pair();
    let item = {
        let mut a = open(&p.a);
        let id = a
            .essence_add(
                EssenceScope::Project,
                EssenceSection::Open,
                "a question",
                None,
            )
            .unwrap();
        a.close().unwrap();
        copy(&p.a, &p.b);
        id
    };
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    a.essence_resolve(&item).unwrap();
    b.essence_update(&item, "a question, reworded", None)
        .unwrap();
    a.close().unwrap();
    b.close().unwrap();
    let (out, _) = p.check_laws(wall_ms() + 1_000);
    let mut db = Mnemo::open(&out, PW).unwrap();
    let m = db.get(&item).unwrap();
    assert_eq!(m.metadata["_mnemo.status"], "resolved");
}

#[test]
fn each_refusal_says_why() {
    let p = pair();
    let other = p.out("other.mnemo");
    base(&other);
    let try_merge = |a: &Path, b: &Path, out: &Path| {
        Mnemo::merge_files(
            a,
            MergeSecret::Passphrase(PW),
            b,
            MergeSecret::Passphrase(PW),
            out,
            &MergeOptions::default(),
        )
        .unwrap_err()
        .to_string()
    };
    let e = try_merge(&p.a, &other, &p.out("o1.mnemo"));
    assert!(e.contains("aren't copies of one brain"), "{e}");
    let e = try_merge(&p.a, &p.b, &p.b);
    assert!(e.contains("is one of the inputs"), "{e}");
    let exists = p.out("exists.mnemo");
    std::fs::write(&exists, b"x").unwrap();
    let e = try_merge(&p.a, &p.b, &exists);
    assert!(e.contains("already exists"), "{e}");
    #[cfg(unix)]
    {
        let link = p.out("link.mnemo");
        std::os::unix::fs::symlink(&p.a, &link).unwrap();
        let e = try_merge(&p.a, &p.b, &link);
        assert!(e.contains("is one of the inputs"), "{e}");
    }
    let wide = p.out("wide.mnemo");
    {
        let mut c = cfg();
        c.dimensions = 8;
        Mnemo::create(&wide, PW, c).unwrap().close().unwrap();
    }
    let e = try_merge(&p.a, &wide, &p.out("o2.mnemo"));
    assert!(e.contains("different dimensions"), "{e}");
    // Nothing is left behind by a refusal.
    for n in ["o1.mnemo", "o2.mnemo"] {
        assert!(!p.out(n).exists());
    }
    let leftovers: Vec<_> = std::fs::read_dir(p.a.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("merge"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn the_inputs_bytes_are_unchanged() {
    let p = pair();
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    a.remember(Memory::new("in a", MemoryType::Semantic, v(2.0)))
        .unwrap();
    b.delete(&p.base_fact).unwrap();
    a.close().unwrap();
    b.close().unwrap();
    let before = (std::fs::read(&p.a).unwrap(), std::fs::read(&p.b).unwrap());
    merge(&p.a, &p.b, &p.out("o.mnemo"), wall_ms());
    let after = (std::fs::read(&p.a).unwrap(), std::fs::read(&p.b).unwrap());
    assert!(before == after, "a merge writes nothing to its inputs");
}

#[test]
fn a_rekeyed_copy_merges_with_its_own_secret_and_the_output_takes_the_first() {
    let p = pair();
    let mut b = open(&p.b);
    let xb = b
        .remember(Memory::new("in b", MemoryType::Semantic, v(2.0)))
        .unwrap();
    b.rekey("b-has-a-new-passphrase", KdfParams::fast())
        .unwrap();
    b.close().unwrap();
    let out = p.out("o.mnemo");
    Mnemo::merge_files(
        &p.a,
        MergeSecret::Passphrase(PW),
        &p.b,
        MergeSecret::Passphrase("b-has-a-new-passphrase"),
        &out,
        &MergeOptions::default(),
    )
    .unwrap();
    let mut db = Mnemo::open(&out, PW).unwrap();
    db.get(&xb).unwrap();
    assert!(Mnemo::open(&out, "b-has-a-new-passphrase").is_err());
}

#[test]
fn an_edit_on_one_side_only_is_no_conflict() {
    let p = pair();
    let mut a = open(&p.a);
    a.update(
        &p.base_fact,
        MemoryPatch {
            content: Some("edited in a only".into()),
            vector: Some(v(5.0)),
            ..Default::default()
        },
    )
    .unwrap();
    a.close().unwrap();
    let (out, r) = p.check_laws(wall_ms() + 1_000);
    let mut db = Mnemo::open(&out, PW).unwrap();
    assert_eq!(db.get(&p.base_fact).unwrap().content, "edited in a only");
    assert!(
        r.conflicts.is_empty(),
        "{:?} {:?}",
        r.conflicts,
        r.conflict_ids
    );
}

fn edit(db: &mut Mnemo, id: &Ulid, text: &str) {
    db.update(
        id,
        MemoryPatch {
            content: Some(text.into()),
            vector: Some(v(text.len() as f32)),
            ..Default::default()
        },
    )
    .unwrap();
}

fn copies(db: &mut Mnemo) -> Vec<Memory> {
    db.memories()
        .unwrap()
        .into_iter()
        .filter(|m| meta(m, MNEMO_META_MERGE_SOURCE).is_some())
        .collect()
}

#[test]
fn eight_edits_on_one_side_are_known_and_nine_fall_back_to_a_copy() {
    // decision.v0210-ancestry: a version keeps the 8 latest before it.
    for (n, want_copy) in [(8, false), (9, true)] {
        let p = pair();
        let mut a = open(&p.a);
        for i in 0..n {
            edit(&mut a, &p.base_fact, &format!("edit number {i}"));
        }
        a.close().unwrap();
        let (out, r) = p.check_laws(wall_ms() + 1_000);
        let mut db = Mnemo::open(&out, PW).unwrap();
        assert_eq!(
            db.get(&p.base_fact).unwrap().content,
            format!("edit number {}", n - 1)
        );
        let c = copies(&mut db);
        assert_eq!(
            c.len(),
            usize::from(want_copy),
            "{n} edits: {:?}",
            r.conflicts
        );
        if want_copy {
            assert_eq!(c[0].content, "the base fact");
        }
    }
}

#[test]
fn an_older_builds_edit_on_both_sides_is_a_conflict() {
    // An older build edits in place and keeps the stamp it found: both
    // sides end with the base's stamp and different content.
    let p = pair();
    for (path, text) in [(&p.a, "old build edit in a"), (&p.b, "old build edit in b")] {
        let mut db = open(path);
        let mut m = db.get(&p.base_fact).unwrap();
        m.content = text.into();
        db.__put_unvalidated_for_testing(m).unwrap();
        db.close().unwrap();
    }
    let (out, r) = p.check_laws(wall_ms() + 1_000);
    assert_eq!(r.conflicts.get("fact"), Some(&1));
    let mut db = Mnemo::open(&out, PW).unwrap();
    assert_eq!(copies(&mut db).len(), 1);
}

#[test]
fn equal_stamps_on_one_id_and_equal_times_in_histories() {
    let p = pair();
    {
        let mut a = open(&p.a);
        a.set_author(Some("planner-a".into()));
        let mut spec = TaskSpec::new("Same second");
        spec.role = Some("worker".into());
        a.task_add(&spec).unwrap();
        a.close().unwrap();
        copy(&p.a, &p.b);
    }
    let t = wall_ms();
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    for (db, who, text) in [
        (&mut a, "worker-a", "same ms, a"),
        (&mut b, "worker-b", "same ms, b"),
    ] {
        db.__freeze_hlc_clock_for_testing(Some(t));
        edit(db, &p.base_fact, text);
        db.set_author(Some(who.into()));
        db.task_next(Some("worker")).unwrap().unwrap();
    }
    a.close().unwrap();
    b.close().unwrap();
    let (out, r) = p.check_laws(t + 1_000);
    assert_eq!(
        r.conflicts.get("fact"),
        Some(&1),
        "equal stamps, different content"
    );
    let mut db = Mnemo::open(&out, PW).unwrap();
    let task = db
        .tasks(None, None)
        .unwrap()
        .into_iter()
        .find(|t| t.slug == "same-second")
        .unwrap();
    assert_eq!(task.status, "active");
}

#[test]
fn a_copy_an_hour_ahead_is_reported() {
    let p = pair();
    let mut b = open(&p.b);
    b.__set_hlc_offset_for_testing(2 * MERGE_LEAD_WARN_MS);
    b.remember(Memory::new("written ahead", MemoryType::Semantic, v(2.0)))
        .unwrap();
    b.close().unwrap();
    let (_, r) = p.check_laws(wall_ms());
    assert!(r.lead_ms[1] > MERGE_LEAD_WARN_MS, "{:?}", r.lead_ms);
    assert!(r.lead_ms[0] <= 0, "{:?}", r.lead_ms);
}

#[test]
fn merge_x_x_is_x_compacted() {
    // With deletes, a re-created id, an expired record and a pruned
    // message: merging a copy with itself gives what compaction keeps.
    let p = pair();
    let mut a = open(&p.a);
    let gone = a
        .remember(Memory::new("deleted", MemoryType::Semantic, v(2.0)))
        .unwrap();
    a.delete(&gone).unwrap();
    a.delete(&p.base_fact).unwrap();
    let mut back = Memory::new("the base fact, written again", MemoryType::Semantic, v(1.5));
    back.id = p.base_fact;
    a.remember(back).unwrap();
    let mut old = Memory::new("expired", MemoryType::Working, v(3.0));
    old.created_at -= 100;
    old.ttl_secs = Some(10);
    a.remember(old).unwrap();
    a.set_author(Some("worker-a".into()));
    let msg = a
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
    a.ack(&msg.id, None, Some("planner-a")).unwrap();
    a.prune_messages(-1).unwrap();
    a.close().unwrap();
    let compacted = p.out("compacted.mnemo");
    copy(&p.a, &compacted);
    Mnemo::compact_file(compacted.to_str().unwrap(), PW).unwrap();
    let out = p.out("xx.mnemo");
    let r = merge(&p.a, &p.a, &out, wall_ms());
    assert_eq!(records(&out), records(&compacted));
    assert!(r.conflicts.is_empty() && r.dropped == 1, "{r:?}");
    let mut db = Mnemo::open(&out, PW).unwrap();
    assert_eq!(
        db.get(&p.base_fact).unwrap().content,
        "the base fact, written again"
    );
}

#[test]
fn a_forgotten_record_is_gone_from_every_read() {
    let p = pair();
    let k = {
        let mut a = open(&p.a);
        let (k, _) = a
            .remember_keyed(
                Memory::new("zanzibar is the secret word", MemoryType::Semantic, v(7.0)),
                "word",
            )
            .unwrap();
        a.close().unwrap();
        copy(&p.a, &p.b);
        k
    };
    let turn = |text: &str| ThreadTurn {
        turn_id: "u1".into(),
        role: "user".into(),
        content: text.into(),
        timestamp: None,
    };
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    // The turn saved in both copies, under two ids.
    a.ingest_thread_with_vectors("t", &[turn("zanzibar in a turn")], &[v(8.0)])
        .unwrap();
    b.ingest_thread_with_vectors("t", &[turn("zanzibar in a turn")], &[v(8.0)])
        .unwrap();
    // a forgets both; b edits the fact and caches a result naming it.
    a.delete(&k).unwrap();
    a.thread_forget("t").unwrap();
    edit(&mut b, &k, "zanzibar, edited where it wasn't forgotten");
    b.cache_put("answers", "q", b"zanzibar", CachePutOpts::default())
        .unwrap();
    a.close().unwrap();
    b.close().unwrap();
    let (out, _) = p.check_laws(wall_ms() + 1_000);
    let mut db = Mnemo::open(&out, PW).unwrap();
    let all = db.memories().unwrap();
    assert!(
        all.iter().all(|m| !m.content.contains("zanzibar")),
        "{all:?}"
    );
    for req in [
        RecallRequest::new(v(7.0)).top_k(50),
        RecallRequest::keywords_only("zanzibar").top_k(50),
    ] {
        let hits = db
            .recall(&req.visibility(VisibilitySet::ALL).track_access(false))
            .unwrap();
        assert!(hits.iter().all(|h| !h.memory.content.contains("zanzibar")));
    }
    assert!(db
        .history_in("word", KeyScope::private_default())
        .unwrap()
        .is_empty());
    assert!(db.thread_export("t").unwrap().is_empty());
    assert!(copies(&mut db).is_empty());
    assert!(
        db.cache_get("answers", "q").unwrap().is_none(),
        "no cached result is carried"
    );
}

#[test]
fn a_supersede_against_an_edit_keeps_the_mark() {
    let p = pair();
    let g = {
        let mut a = open(&p.a);
        let g = a
            .remember(Memory::new("the newer fact", MemoryType::Semantic, v(2.0)))
            .unwrap();
        a.close().unwrap();
        copy(&p.a, &p.b);
        g
    };
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    a.supersede(&p.base_fact, &g).unwrap();
    edit(&mut b, &p.base_fact, "the base fact, edited");
    a.close().unwrap();
    b.close().unwrap();
    let (out, _) = p.check_laws(wall_ms() + 1_000);
    let mut db = Mnemo::open(&out, PW).unwrap();
    let f = db.get(&p.base_fact).unwrap();
    assert_eq!(f.metadata["_mnemo.superseded_by"], g.to_string());
}

#[test]
fn two_findings_with_one_id_and_an_after_cycle_are_reported() {
    let p = pair();
    {
        let mut a = open(&p.a);
        a.set_author(Some("planner-a".into()));
        for slug in ["t1", "t2"] {
            let mut spec = TaskSpec::new(format!("Task {slug}"));
            spec.slug = Some(slug.into());
            a.task_add(&spec).unwrap();
        }
        a.close().unwrap();
        copy(&p.a, &p.b);
    }
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    for (db, slug, after, title) in [
        (&mut a, "t1", "t2", "Seen in a"),
        (&mut b, "t2", "t1", "Seen in b"),
    ] {
        db.set_author(Some("planner-a".into()));
        let mut spec = TaskSpec::new(format!("Task {slug}"));
        spec.slug = Some(slug.into());
        spec.after = vec![after.into()];
        db.task_add(&spec).unwrap();
        db.set_author(Some("reviewer-b".into()));
        db.finding_add(&FindingSpec::new("RB-099.1", "CORRECTNESS", "LOW", title))
            .unwrap();
    }
    a.close().unwrap();
    b.close().unwrap();
    let (out, r) = p.check_laws(wall_ms() + 1_000);
    assert_eq!(
        r.after_cycles,
        vec![vec!["t1".to_string(), "t2".to_string()]]
    );
    assert_eq!(r.renamed.len(), 1);
    assert_eq!(r.renamed[0].kind, "finding");
    assert_eq!(r.renamed[0].to, "RB-099.1-merge-2");
    let mut db = Mnemo::open(&out, PW).unwrap();
    assert!(db.finding("RB-099.1").unwrap().is_some());
    assert!(db.finding("RB-099.1-merge-2").unwrap().is_some());
}

struct LenEmbedder;
impl Embedder for LenEmbedder {
    fn identity(&self) -> EmbedderIdentity {
        EmbedderIdentity {
            name: "len".into(),
            dimensions: 4,
            normalize: false,
            ..Default::default()
        }
    }
    fn embed(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| v(t.len() as f32)).collect())
    }
}

#[test]
fn more_refusals_say_why() {
    let p = pair();
    let try_merge = |a: &Path, sa: MergeSecret, b: &Path, sb: MergeSecret, out: &Path| {
        Mnemo::merge_files(a, sa, b, sb, out, &MergeOptions::default())
            .unwrap_err()
            .to_string()
    };
    let pw = MergeSecret::Passphrase(PW);
    // Different embedders.
    let mut b = open(&p.b);
    b.set_embedder(Box::new(LenEmbedder)).unwrap();
    b.close().unwrap();
    let e = try_merge(&p.a, pw, &p.b, pw, &p.out("o1.mnemo"));
    assert!(e.contains("embedders differ"), "{e}");
    // Float vectors on one side only.
    let floats = p.out("floats.mnemo");
    let mut c = cfg();
    c.store_float_vectors = true;
    Mnemo::create(&floats, PW, c).unwrap().close().unwrap();
    let e = try_merge(&p.a, pw, &floats, pw, &p.out("o2.mnemo"));
    assert!(e.contains("float vectors"), "{e}");
    // A v8 input (a read-only open takes it; the merge doesn't).
    let v8 = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v8/v8_passphrase.mnemo");
    let e = try_merge(
        &p.a,
        pw,
        &v8,
        MergeSecret::Passphrase("v8-fixture-passphrase"),
        &p.out("o3.mnemo"),
    );
    assert!(
        e.contains("older than format 9") && e.contains("mnemo compact"),
        "{e}"
    );
    // A wrong secret is the open's own error.
    let e = try_merge(
        &p.a,
        pw,
        &p.a,
        MergeSecret::Passphrase("wrong"),
        &p.out("o4.mnemo"),
    );
    assert!(e.to_lowercase().contains("passphrase"), "{e}");
}

#[cfg(feature = "failpoints")]
#[test]
fn an_input_that_needs_a_write_open_is_refused_with_the_fix() {
    let p = pair();
    let mut found = false;
    for n in 0..40 {
        let path = p.out(&format!("pending-{n}.mnemo"));
        copy(&p.a, &path);
        let mut db = open(&path);
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
        let e = Mnemo::merge_files(
            &p.a,
            MergeSecret::Passphrase(PW),
            &path,
            MergeSecret::Passphrase(PW),
            p.out("o.mnemo"),
            &MergeOptions::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            e.contains("needs a read-write open") && e.contains("mnemo index"),
            "{e}"
        );
        found = true;
        break;
    }
    assert!(found, "no n left a pending WAL");
}

#[cfg(feature = "failpoints")]
#[test]
fn a_failure_mid_merge_leaves_nothing() {
    let p = pair();
    let dir = p.a.parent().unwrap().to_path_buf();
    let names = || -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    };
    let before = names();
    let out = p.out("o.mnemo");
    // How many writes a whole merge makes, then each of them failing.
    mnemo_engine::__failpoints::set_writes_failing(&[]);
    merge(&p.a, &p.b, &p.out("counted.mnemo"), wall_ms());
    let writes = mnemo_engine::__failpoints::writes_seen() as i64;
    std::fs::remove_file(p.out("counted.mnemo")).unwrap();
    assert!(writes > 2, "{writes}");
    for n in 0..writes {
        mnemo_engine::__failpoints::set_writes_until_fail(n);
        let r = Mnemo::merge_files(
            &p.a,
            MergeSecret::Passphrase(PW),
            &p.b,
            MergeSecret::Passphrase(PW),
            &out,
            &MergeOptions::default(),
        );
        mnemo_engine::__failpoints::set_writes_until_fail(-1);
        assert!(r.is_err(), "write {n} failed, so the merge did");
        assert_eq!(names(), before, "nothing left after a failure at write {n}");
    }
}

#[test]
fn the_output_has_its_own_data_key_and_stamps_go_past_both_inputs() {
    let p = pair();
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    a.__set_hlc_offset_for_testing(MERGE_LEAD_WARN_MS);
    edit(&mut a, &p.base_fact, "edited ahead in a");
    b.remember(Memory::new("in b", MemoryType::Semantic, v(2.0)))
        .unwrap();
    a.close().unwrap();
    b.close().unwrap();
    let out = p.out("o.mnemo");
    merge(&p.a, &p.b, &out, wall_ms());
    let key = |x: &Path| Mnemo::__wrapped_key_for_testing(x).unwrap();
    assert_ne!(key(&out), key(&p.a));
    assert_ne!(key(&out), key(&p.b));
    let stamp = |m: &Memory| m.metadata["_mnemo.hlc"].as_u64().unwrap();
    let in_a = stamp(
        &Mnemo::open_read_only(&p.a, PW)
            .unwrap()
            .get(&p.base_fact)
            .unwrap(),
    );
    let mut db = Mnemo::open(&out, PW).unwrap();
    edit(&mut db, &p.base_fact, "edited again after the merge");
    assert!(stamp(&db.get(&p.base_fact).unwrap()) > in_a);
}
