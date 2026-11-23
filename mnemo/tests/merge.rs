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

/// `records(p) == records(q)`, naming the first record that differs.
fn assert_same_records(p: &Path, q: &Path, what: &str) {
    let (x, y) = (records(p), records(q));
    for (l, r) in x.iter().zip(&y) {
        assert_eq!(l, r, "{what}");
    }
    assert_eq!(x.len(), y.len(), "{what}: the number of records");
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
fn a_renamed_task_whose_rival_is_forgotten_takes_its_name_back_and_its_flag_goes() {
    // Two tasks under one slug: the merge renames the later one and flags
    // it. Then one copy of the output forgets the first, and another edits
    // the renamed one. The renamed task takes the slug back, and its flag
    // goes with the rename, so merging the output again changes nothing.
    let p = pair();
    let t = wall_ms();
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    b.__set_clock_offset_for_testing(60);
    for db in [&mut a, &mut b] {
        db.set_author(Some("planner-a".into()));
        let mut spec = TaskSpec::new("Write the notes");
        spec.role = Some("worker".into());
        db.task_add(&spec).unwrap();
    }
    a.close().unwrap();
    b.close().unwrap();
    let (out, _) = p.check_laws(t + 120_000);
    let ids = |path: &Path| -> Vec<(String, Ulid)> {
        let mut db = Mnemo::open(path, PW).unwrap();
        let tasks = db.tasks(None, None).unwrap();
        tasks.into_iter().map(|t| (t.slug, t.id)).collect()
    };
    let before = ids(&out);
    let first = before
        .iter()
        .find(|(s, _)| s == "write-the-notes")
        .unwrap()
        .1;
    let renamed = before
        .iter()
        .find(|(s, _)| s == "write-the-notes-merge-2")
        .unwrap()
        .1;
    assert!(flagged(&out, &renamed) && !flagged(&out, &first));
    let qdir = tempfile::tempdir().unwrap();
    let q = Pair {
        a: qdir.path().join("x.mnemo"),
        b: qdir.path().join("y.mnemo"),
        _dir: qdir,
        base_fact: p.base_fact,
    };
    copy(&out, &q.a);
    copy(&out, &q.b);
    let (mut x, mut y) = (open(&q.a), open(&q.b));
    x.delete(&first).unwrap();
    y.set_author(Some("planner-a".into()));
    y.task_update("write-the-notes-merge-2", None, Some("a note"), None)
        .unwrap();
    x.close().unwrap();
    y.close().unwrap();
    let (out2, _) = q.check_laws(t + 240_000);
    let after = ids(&out2);
    assert_eq!(after, vec![("write-the-notes".to_string(), renamed)]);
    assert!(!flagged(&out2, &renamed));
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

#[test]
fn claims_come_from_the_first_input_only() {
    let p = pair();
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    a.set_author(Some("worker-a".into()));
    a.claim("tree:a", Some(600)).unwrap();
    b.set_author(Some("worker-b".into()));
    b.claim("tree:b", Some(600)).unwrap();
    a.close().unwrap();
    b.close().unwrap();
    for (first, second, want) in [(&p.a, &p.b, "tree:a"), (&p.b, &p.a, "tree:b")] {
        let out = p.out(&format!("claims-{want}.mnemo"));
        merge(first, second, &out, wall_ms());
        let mut db = Mnemo::open(&out, PW).unwrap();
        let topics: Vec<String> = db.claims().unwrap().into_iter().map(|c| c.topic).collect();
        assert_eq!(topics, [want]);
    }
}

#[test]
fn a_record_deleted_in_both_leaves_one_tombstone() {
    let p = pair();
    for path in [&p.a, &p.b] {
        let mut db = open(path);
        db.delete(&p.base_fact).unwrap();
        db.close().unwrap();
    }
    let (out, _) = p.check_laws(wall_ms());
    let mut db = Mnemo::open(&out, PW).unwrap();
    assert_eq!(db.kind_counts().unwrap().tombstones, 1);
}

/// Edits `id` to `text` with the copy's clock frozen at `at_ms`.
fn edit_at(path: &Path, id: &Ulid, text: &str, at_ms: u64) {
    let mut db = open(path);
    db.__freeze_hlc_clock_for_testing(Some(at_ms));
    edit(&mut db, id, text);
    db.close().unwrap();
}

#[test]
fn an_edit_in_the_same_millisecond_on_each_side_is_never_ancestry() {
    // Both copies edit at one stamp, then one edits again: its list names
    // its own first edit by stamp and hash, never the other copy's.
    let p = pair();
    let t = wall_ms() + 10_000;
    edit_at(&p.a, &p.base_fact, "first edit in a", t);
    edit_at(&p.b, &p.base_fact, "the edit in b", t);
    edit_at(&p.a, &p.base_fact, "second edit in a", t);
    let (out, r) = p.check_laws(t + 1_000);
    assert_eq!(r.conflicts.get("fact"), Some(&1), "{r:?}");
    let mut db = Mnemo::open(&out, PW).unwrap();
    assert_eq!(db.get(&p.base_fact).unwrap().content, "second edit in a");
    let c = copies(&mut db);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].content, "the edit in b");
}

#[test]
fn a_forgotten_record_takes_its_conflict_copies_with_it() {
    let p = pair();
    let t = wall_ms();
    edit_at(&p.a, &p.base_fact, "edited in a", t);
    edit_at(&p.b, &p.base_fact, "edited in b", t + 1_000);
    let m = p.out("m.mnemo");
    merge(&p.a, &p.b, &m, t + 2_000);
    let copy_id = copies(&mut open(&m))[0].id;
    let m2 = p.out("m2.mnemo");
    copy(&m, &m2);
    let mut db = open(&m2);
    db.delete(&p.base_fact).unwrap();
    db.close().unwrap();
    for (x, y, name) in [(&m2, &m, "fx.mnemo"), (&m, &m2, "fy.mnemo")] {
        let out = p.out(name);
        let r = merge(x, y, &out, t + 3_000);
        assert!(r.removed.contains(&copy_id), "{r:?}");
        let mut db = Mnemo::open(&out, PW).unwrap();
        assert!(db.get(&p.base_fact).is_err());
        assert!(copies(&mut db).is_empty());
    }
}

#[test]
fn three_copies_two_writing_the_same_text_merge_alike_in_either_grouping() {
    // a and b edit in one millisecond, c later writes b's text: whichever
    // two merge first, the output holds c's version and one copy, of a's.
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.mnemo");
    let id = base(&base_path);
    let [a, b, c] = ["a", "b", "c"].map(|n| dir.path().join(format!("{n}.mnemo")));
    for x in [&a, &b, &c] {
        copy(&base_path, x);
    }
    let t = wall_ms() + 10_000;
    edit_at(&a, &id, "edited to 3", t);
    edit_at(&b, &id, "edited to 0", t);
    edit_at(&c, &id, "edited to 0", t + 40);
    let o = |n: &str| dir.path().join(n);
    let now = t + 1_000;
    merge(&a, &b, &o("ab.mnemo"), now);
    merge(&o("ab.mnemo"), &c, &o("ab_c.mnemo"), now);
    merge(&b, &c, &o("bc.mnemo"), now);
    merge(&a, &o("bc.mnemo"), &o("a_bc.mnemo"), now);
    merge(&a, &c, &o("ac.mnemo"), now);
    merge(&o("ac.mnemo"), &b, &o("ac_b.mnemo"), now);
    let want = records(&o("ab_c.mnemo"));
    assert_eq!(records(&o("a_bc.mnemo")), want, "merge(a, merge(b, c))");
    assert_eq!(records(&o("ac_b.mnemo")), want, "merge(merge(a, c), b)");
    let mut db = Mnemo::open(o("ab_c.mnemo"), PW).unwrap();
    assert_eq!(db.get(&id).unwrap().content, "edited to 0");
    let c = copies(&mut db);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(c[0].content, "edited to 3");
}

#[test]
fn an_output_edited_afterwards_merges_with_an_input_without_a_new_conflict() {
    // The merged version lists both inputs' versions, so an edit made on
    // the output descends from both: no new conflict, and the first one's
    // copy goes.
    let p = pair();
    let t = wall_ms();
    edit_at(&p.a, &p.base_fact, "edited in a", t);
    edit_at(&p.b, &p.base_fact, "edited in b", t + 1_000);
    let m = p.out("m.mnemo");
    merge(&p.a, &p.b, &m, t + 2_000);
    edit_at(&m, &p.base_fact, "edited after the merge", t + 3_000);
    for (input, name) in [(&p.a, "ma.mnemo"), (&p.b, "mb.mnemo")] {
        let out = p.out(name);
        let r = merge(&m, input, &out, t + 4_000);
        assert!(r.conflicts.is_empty(), "{name}: {:?}", r.conflicts);
        let mut db = Mnemo::open(&out, PW).unwrap();
        assert_eq!(
            db.get(&p.base_fact).unwrap().content,
            "edited after the merge"
        );
        // The edit descends from both versions, so the copy goes too
        // (decision.v0210-conflict-flags).
        assert!(copies(&mut db).is_empty(), "{name}");
    }
}

/// Two versions of one fact, by hand: `(content, stamp, version hash)`.
fn versions(spec: [(&str, u64, u64); 2]) -> [Memory; 2] {
    let first = Memory::new(spec[0].0, MemoryType::Semantic, v(1.0));
    spec.map(|(text, hlc, ver)| {
        let mut m = first.clone();
        m.content = text.into();
        m.metadata.insert("_mnemo.hlc".into(), Value::from(hlc));
        m.metadata.insert("_mnemo.ver".into(), Value::from(ver));
        m
    })
}

#[test]
fn a_conflict_flag_stays_whichever_version_wins() {
    let [mut older, newer] = versions([("older, flagged", 1_000, 11), ("newer", 2_000, 22)]);
    older
        .metadata
        .insert(MNEMO_META_MERGE_CONFLICT.into(), Value::Bool(true));
    let (out, _) =
        Mnemo::__merge_records_for_testing(vec![older], vec![], vec![newer.clone()], vec![], 3_000);
    let won = out.iter().find(|m| m.id == newer.id).unwrap();
    assert_eq!(won.content, "newer");
    assert_eq!(
        meta(won, MNEMO_META_MERGE_CONFLICT),
        Some(&Value::Bool(true))
    );
}

#[test]
fn a_version_a_merge_made_is_never_listed_as_an_earlier_one() {
    const VER_MERGED: u64 = 1 << 62;
    let [x, y] = versions([
        ("made by a merge", 1_000, VER_MERGED | 11),
        ("a write", 2_000, 22),
    ]);
    let (out, _) =
        Mnemo::__merge_records_for_testing(vec![x], vec![], vec![y.clone()], vec![], 3_000);
    let won = out.iter().find(|m| m.id == y.id).unwrap();
    let prev = meta(won, "_mnemo.hlc_prev").and_then(Value::as_array);
    assert!(
        prev.into_iter()
            .flatten()
            .all(|p| p[1].as_u64().unwrap() & VER_MERGED == 0),
        "{prev:?}"
    );
}

/// Three copies of a base holding one essence item; returns their paths
/// and the item.
fn three_with_an_item(dir: &Path) -> ([PathBuf; 3], Ulid) {
    let base_path = dir.join("base.mnemo");
    base(&base_path);
    let mut db = open(&base_path);
    let item = db
        .essence_add(
            EssenceScope::Project,
            EssenceSection::Open,
            "the item",
            None,
        )
        .unwrap();
    db.close().unwrap();
    let paths = ["a", "b", "c"].map(|n| dir.join(format!("{n}.mnemo")));
    for x in &paths {
        copy(&base_path, x);
    }
    (paths, item)
}

fn flagged(p: &Path, id: &Ulid) -> bool {
    let mut db = Mnemo::open_read_only(p, PW).unwrap();
    meta(&db.get(id).unwrap(), MNEMO_META_MERGE_CONFLICT) == Some(&Value::Bool(true))
}

#[test]
fn an_essence_flag_follows_its_texts_in_every_grouping() {
    // The proptest's seed 083bdc4a: a edits the item to one text, b to
    // another, c resolves it and writes b's text. Two concurrent texts:
    // flagged and resolved, whichever two merge first.
    let dir = tempfile::tempdir().unwrap();
    let ([a, b, c], item) = three_with_an_item(dir.path());
    let t = wall_ms() + 10_000;
    for (path, resolve, text) in [
        (&a, false, "the item, take 0"),
        (&b, false, "the item, take 2"),
        (&c, true, "the item, take 2"),
    ] {
        let mut db = open(path);
        db.__freeze_hlc_clock_for_testing(Some(t));
        if resolve {
            db.essence_resolve(&item).unwrap();
        }
        db.essence_update(&item, text, None).unwrap();
        db.close().unwrap();
    }
    let o = |n: &str| dir.path().join(n);
    let now = t + 1_000;
    merge(&a, &b, &o("ab.mnemo"), now);
    merge(&o("ab.mnemo"), &c, &o("ab_c.mnemo"), now);
    merge(&b, &c, &o("bc.mnemo"), now);
    merge(&a, &o("bc.mnemo"), &o("a_bc.mnemo"), now);
    assert_eq!(records(&o("ab_c.mnemo")), records(&o("a_bc.mnemo")));
    assert!(flagged(&o("ab_c.mnemo"), &item));
    assert!(!flagged(&o("bc.mnemo"), &item), "one text: no flag");
    let mut db = Mnemo::open_read_only(o("ab_c.mnemo"), PW).unwrap();
    assert_eq!(db.get(&item).unwrap().metadata["_mnemo.status"], "resolved");
}

#[test]
fn an_essence_flag_goes_once_an_edit_descends_from_both_texts() {
    let dir = tempfile::tempdir().unwrap();
    let ([a, b, _], item) = three_with_an_item(dir.path());
    let t = wall_ms();
    for (path, at, text) in [(&a, t, "text in a"), (&b, t + 1_000, "text in b")] {
        let mut db = open(path);
        db.__freeze_hlc_clock_for_testing(Some(at));
        db.essence_update(&item, text, None).unwrap();
        db.close().unwrap();
    }
    let m = dir.path().join("m.mnemo");
    merge(&a, &b, &m, t + 2_000);
    assert!(flagged(&m, &item));
    let mut db = open(&m);
    db.__freeze_hlc_clock_for_testing(Some(t + 3_000));
    db.essence_update(&item, "one text for both", None).unwrap();
    db.close().unwrap();
    for (input, name) in [(&a, "ma.mnemo"), (&b, "mb.mnemo")] {
        let out = dir.path().join(name);
        let r = merge(&m, input, &out, t + 4_000);
        assert!(r.conflicts.is_empty(), "{name}: {:?}", r.conflicts);
        let mut db = Mnemo::open_read_only(&out, PW).unwrap();
        let got = db.get(&item).unwrap();
        assert_eq!(got.content, "one text for both");
        // The edit descends from both texts: the flag and the list go.
        assert!(!flagged(&out, &item), "{name}");
        assert!(got.metadata.get("_mnemo.merge_heads").is_none(), "{name}");
    }
}

#[test]
fn a_copy_of_a_copy_merges_alike_in_either_grouping() {
    // x writes 'text p'; y, copied from x, rewrites it 'text q'; z, from
    // the base, writes 'text q'. y was edited from x's text, so no grouping
    // keeps a copy of it.
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.mnemo");
    let id = base(&base_path);
    let o = |n: &str| dir.path().join(n);
    let t = wall_ms() + 10_000;
    copy(&base_path, &o("x.mnemo"));
    edit_at(&o("x.mnemo"), &id, "text p", t);
    copy(&o("x.mnemo"), &o("y.mnemo"));
    edit_at(&o("y.mnemo"), &id, "text q", t + 10);
    copy(&base_path, &o("z.mnemo"));
    edit_at(&o("z.mnemo"), &id, "text q", t + 5);
    let now = t + 1_000;
    let (x, y, z) = (o("x.mnemo"), o("y.mnemo"), o("z.mnemo"));
    merge(&x, &z, &o("xz.mnemo"), now);
    merge(&o("xz.mnemo"), &y, &o("xz_y.mnemo"), now);
    merge(&z, &y, &o("zy.mnemo"), now);
    merge(&x, &o("zy.mnemo"), &o("x_zy.mnemo"), now);
    assert_eq!(records(&o("xz_y.mnemo")), records(&o("x_zy.mnemo")));
    let mut db = Mnemo::open_read_only(o("xz_y.mnemo"), PW).unwrap();
    assert_eq!(db.get(&id).unwrap().content, "text q");
    assert!(copies(&mut db).is_empty());
    // Before y arrived, x and z were a conflict.
    assert_eq!(copies(&mut open(&o("xz.mnemo"))).len(), 1);
}

#[test]
fn a_copy_of_the_text_that_wins_goes() {
    // a writes 'text one', b later 'text two', c later still 'text one'.
    // merge(a, b) keeps a copy of a's text; once c's version of that text
    // wins, the copy goes and the one left holds b's.
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.mnemo");
    let id = base(&base_path);
    let o = |n: &str| dir.path().join(n);
    let t = wall_ms() + 10_000;
    for (n, text, at) in [
        ("a", "text one", t),
        ("b", "text two", t + 10),
        ("c", "text one", t + 40),
    ] {
        let p = o(&format!("{n}.mnemo"));
        copy(&base_path, &p);
        edit_at(&p, &id, text, at);
    }
    let now = t + 1_000;
    let (a, b, c) = (o("a.mnemo"), o("b.mnemo"), o("c.mnemo"));
    merge(&a, &b, &o("ab.mnemo"), now);
    let mut db = Mnemo::open_read_only(o("ab.mnemo"), PW).unwrap();
    assert_eq!(
        copies(&mut db)
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>(),
        ["text one"]
    );
    merge(&o("ab.mnemo"), &c, &o("ab_c.mnemo"), now);
    merge(&b, &c, &o("bc.mnemo"), now);
    merge(&a, &o("bc.mnemo"), &o("a_bc.mnemo"), now);
    assert_eq!(records(&o("ab_c.mnemo")), records(&o("a_bc.mnemo")));
    let mut db = Mnemo::open_read_only(o("ab_c.mnemo"), PW).unwrap();
    assert_eq!(db.get(&id).unwrap().content, "text one");
    let c = copies(&mut db);
    assert_eq!(
        c.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(),
        ["text two"],
        "{c:?}"
    );
}

#[test]
fn an_output_merged_with_its_own_later_edit_keeps_no_copy() {
    // Both files hold the merge's copy unchanged; the edit descends from
    // both versions, so the copy goes.
    let p = pair();
    let t = wall_ms();
    edit_at(&p.a, &p.base_fact, "edited in a", t);
    edit_at(&p.b, &p.base_fact, "edited in b", t + 1_000);
    let (m, m2) = (p.out("m.mnemo"), p.out("m2.mnemo"));
    merge(&p.a, &p.b, &m, t + 2_000);
    copy(&m, &m2);
    edit_at(&m2, &p.base_fact, "edited after the merge", t + 3_000);
    assert_eq!(
        copies(&mut open(&m2)).len(),
        1,
        "the copy is still in the edited file"
    );
    for (x, y, name) in [(&m, &m2, "mm2.mnemo"), (&m2, &m, "m2m.mnemo")] {
        let out = p.out(name);
        let r = merge(x, y, &out, t + 4_000);
        assert!(r.conflicts.is_empty(), "{name}: {:?}", r.conflicts);
        let mut db = Mnemo::open_read_only(&out, PW).unwrap();
        assert_eq!(
            db.get(&p.base_fact).unwrap().content,
            "edited after the merge"
        );
        assert!(copies(&mut db).is_empty(), "{name}");
    }
}

#[test]
fn an_ack_and_the_agents_a_message_was_shown_to_are_kept_from_either_side() {
    // a acks a message and b, later, marks it seen; a broadcast is shown
    // to worker-a in a and to reviewer-b in b. Whichever side is newer,
    // the message stays acked and the broadcast lists both agents.
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.mnemo");
    base(&base_path);
    let mut db = open(&base_path);
    let direct = db
        .post(
            "planner-a",
            "DONE: x",
            "",
            &[],
            None,
            Some("worker-a"),
            None,
        )
        .unwrap()
        .id;
    let all = db
        .post("all", "a broadcast", "", &[], None, Some("planner-a"), None)
        .unwrap()
        .id;
    db.close().unwrap();
    let o = |n: &str| dir.path().join(n);
    let (a, b) = (o("a.mnemo"), o("b.mnemo"));
    copy(&base_path, &a);
    copy(&base_path, &b);
    let t = wall_ms() + 10_000;
    let mut db = open(&a);
    db.__freeze_hlc_clock_for_testing(Some(t));
    db.ack(&direct, None, Some("planner-a")).unwrap();
    db.set_author(Some("worker-a".into()));
    db.mark_seen(&[all]).unwrap();
    db.close().unwrap();
    let mut db = open(&b);
    db.__freeze_hlc_clock_for_testing(Some(t + 10));
    db.set_author(Some("reviewer-b".into()));
    assert_eq!(db.mark_seen(&[direct, all]).unwrap(), 2);
    db.close().unwrap();
    for (x, y, name) in [(&a, &b, "ab.mnemo"), (&b, &a, "ba.mnemo")] {
        let out = o(name);
        merge(x, y, &out, t + 1_000);
        let mut db = Mnemo::open_read_only(&out, PW).unwrap();
        let d = db.get(&direct).unwrap();
        assert_eq!(
            meta(&d, "_mnemo.status").and_then(Value::as_str),
            Some("acked"),
            "{name}"
        );
        let bc = db.get(&all).unwrap();
        let seen: Vec<&str> = meta(&bc, "_mnemo.seen_by")
            .and_then(Value::as_array)
            .map(|v| v.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert_eq!(seen, ["reviewer-b", "worker-a"], "{name}");
    }
}

#[test]
fn a_turn_saved_on_both_sides_is_kept_once() {
    // Each copy saves the same turn of one thread, under its own id.
    let p = pair();
    for (path, n) in [(&p.a, 5.0), (&p.b, 6.0)] {
        let mut db = open(path);
        db.ingest_thread_with_vectors(
            "t",
            &[ThreadTurn {
                turn_id: "u1".into(),
                role: "user".into(),
                content: "one turn".into(),
                timestamp: None,
            }],
            &[v(n)],
        )
        .unwrap();
        db.close().unwrap();
    }
    let turns = |path: &Path| {
        records(path)
            .into_iter()
            .filter(|m| m["metadata"]["_mnemo.turn_id"] == "u1")
            .count()
    };
    assert_eq!((turns(&p.a), turns(&p.b)), (1, 1));
    let (ab, _) = p.check_laws(wall_ms());
    assert_eq!(turns(&ab), 1);
}

#[test]
fn a_link_a_merge_made_to_a_version_another_copy_deleted_goes_in_every_grouping() {
    // a and b each write key k; d, copied from a, deletes a's version.
    // Merging a and b first links b's version after a's; merging b and d
    // first never does. The link to the deleted version goes either way.
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.mnemo");
    base(&base_path);
    let o = |n: &str| dir.path().join(n);
    let (a, b, d) = (o("a.mnemo"), o("b.mnemo"), o("d.mnemo"));
    let mut ids = Vec::new();
    for (p, x, who) in [(&a, 2.0, "worker-a"), (&b, 3.0, "worker-b")] {
        copy(&base_path, p);
        let mut db = open(p);
        db.set_author(Some(who.into()));
        let (id, _) = db
            .remember_keyed(Memory::new("k is 0", MemoryType::Semantic, v(x)), "k")
            .unwrap();
        ids.push(id);
        db.close().unwrap();
    }
    copy(&a, &d);
    let mut db = open(&d);
    db.delete(&ids[0]).unwrap();
    db.close().unwrap();
    let now = wall_ms() + 1_000;
    merge(&a, &b, &o("ab.mnemo"), now);
    let mut db = Mnemo::open_read_only(o("ab.mnemo"), PW).unwrap();
    let linked = db.get(&ids[1]).unwrap();
    assert!(
        meta(&linked, "_mnemo.supersedes").is_some(),
        "merge(a, b) links them"
    );
    merge(&o("ab.mnemo"), &d, &o("ab_d.mnemo"), now);
    merge(&b, &d, &o("bd.mnemo"), now);
    merge(&a, &o("bd.mnemo"), &o("a_bd.mnemo"), now);
    merge(&a, &d, &o("ad.mnemo"), now);
    merge(&o("ad.mnemo"), &b, &o("ad_b.mnemo"), now);
    assert_same_records(&o("a_bd.mnemo"), &o("ab_d.mnemo"), "merge(a, merge(b, d))");
    assert_same_records(&o("ad_b.mnemo"), &o("ab_d.mnemo"), "merge(merge(a, d), b)");
    let mut db = Mnemo::open_read_only(o("ab_d.mnemo"), PW).unwrap();
    assert!(db.get(&ids[0]).is_err(), "a's version is deleted");
    let left = db.get(&ids[1]).unwrap();
    assert!(
        meta(&left, "_mnemo.supersedes").is_none(),
        "{:?}",
        left.metadata
    );
}

#[test]
fn a_deleted_middle_version_keeps_the_links_around_it() {
    // A copy that deleted the middle of three versions keeps the newest
    // one's link to it, and merging it with itself keeps that too.
    let p = pair();
    let mut db = open(&p.a);
    let ids: Vec<Ulid> = (0..3)
        .map(|i| {
            db.remember_keyed(
                Memory::new(format!("k is {i}"), MemoryType::Semantic, v(2.0 + i as f32)),
                "k",
            )
            .unwrap()
            .0
        })
        .collect();
    db.delete(&ids[1]).unwrap();
    let newest = db.get(&ids[2]).unwrap();
    let to_middle = Some(Value::String(ids[1].to_string()));
    assert_eq!(meta(&newest, "_mnemo.supersedes").cloned(), to_middle);
    db.close().unwrap();
    let out = p.out("aa.mnemo");
    merge(&p.a, &p.a, &out, wall_ms() + 1_000);
    let mut db = Mnemo::open_read_only(&out, PW).unwrap();
    let newest = db.get(&ids[2]).unwrap();
    assert_eq!(meta(&newest, "_mnemo.supersedes").cloned(), to_middle);
}

#[test]
fn a_chain_closes_up_around_a_version_another_copy_deleted() {
    // The key has a base version; a (its clock an hour behind) and b each
    // write a new one, and d, copied from a, deletes a's. The base and
    // b's version keep one order in every grouping.
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.mnemo");
    base(&base_path);
    let mut db = open(&base_path);
    let (first, _) = db
        .remember_keyed(Memory::new("k, base", MemoryType::Semantic, v(1.5)), "k")
        .unwrap();
    db.close().unwrap();
    let o = |n: &str| dir.path().join(n);
    let (a, b, d) = (o("a.mnemo"), o("b.mnemo"), o("d.mnemo"));
    let t = wall_ms();
    let mut ids = Vec::new();
    for (p, at, x) in [(&a, t - 3_600_000, 2.0), (&b, t, 3.0)] {
        copy(&base_path, p);
        let mut db = open(p);
        db.__freeze_hlc_clock_for_testing(Some(at));
        let (id, _) = db
            .remember_keyed(Memory::new("k is 0", MemoryType::Semantic, v(x)), "k")
            .unwrap();
        ids.push(id);
        db.close().unwrap();
    }
    copy(&a, &d);
    let mut db = open(&d);
    db.delete(&ids[0]).unwrap();
    db.close().unwrap();
    let now = t + 1_000;
    merge(&a, &b, &o("ab.mnemo"), now);
    merge(&o("ab.mnemo"), &d, &o("ab_d.mnemo"), now);
    merge(&b, &d, &o("bd.mnemo"), now);
    merge(&a, &o("bd.mnemo"), &o("a_bd.mnemo"), now);
    merge(&a, &d, &o("ad.mnemo"), now);
    merge(&o("ad.mnemo"), &b, &o("ad_b.mnemo"), now);
    assert_same_records(&o("a_bd.mnemo"), &o("ab_d.mnemo"), "merge(a, merge(b, d))");
    assert_same_records(&o("ad_b.mnemo"), &o("ab_d.mnemo"), "merge(merge(a, d), b)");
    let mut db = Mnemo::open_read_only(o("ab_d.mnemo"), PW).unwrap();
    let to_first = Some(Value::String(first.to_string()));
    assert_eq!(
        meta(&db.get(&ids[1]).unwrap(), "_mnemo.supersedes").cloned(),
        to_first
    );
}

#[test]
fn three_copies_writing_one_key_in_one_millisecond_name_its_versions_alike() {
    // Each copy's write rewrites the base version's link to it, in the
    // same millisecond. The merged base version is named from what it
    // holds once the chain is relinked, so every grouping names it alike.
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.mnemo");
    base(&base_path);
    let mut db = open(&base_path);
    db.remember_keyed(Memory::new("k, base", MemoryType::Semantic, v(1.5)), "k")
        .unwrap();
    db.close().unwrap();
    let o = |n: &str| dir.path().join(n);
    let t = wall_ms() + 10_000;
    for (n, x) in [("b", 2.0), ("c", 3.0), ("d", 4.0)] {
        let p = o(&format!("{n}.mnemo"));
        copy(&base_path, &p);
        let mut db = open(&p);
        db.set_author(Some(format!("worker-{n}")));
        db.__freeze_hlc_clock_for_testing(Some(t));
        db.remember_keyed(Memory::new("k is 0", MemoryType::Semantic, v(x)), "k")
            .unwrap();
        db.close().unwrap();
    }
    let (b, c, d) = (o("b.mnemo"), o("c.mnemo"), o("d.mnemo"));
    let now = t + 1_000;
    merge(&b, &c, &o("bc.mnemo"), now);
    merge(&o("bc.mnemo"), &d, &o("bc_d.mnemo"), now);
    merge(&c, &d, &o("cd.mnemo"), now);
    merge(&b, &o("cd.mnemo"), &o("b_cd.mnemo"), now);
    merge(&b, &d, &o("bd.mnemo"), now);
    merge(&o("bd.mnemo"), &c, &o("bd_c.mnemo"), now);
    assert_same_records(&o("b_cd.mnemo"), &o("bc_d.mnemo"), "merge(b, merge(c, d))");
    assert_same_records(&o("bd_c.mnemo"), &o("bc_d.mnemo"), "merge(merge(b, d), c)");
}

/// The three groupings of `a`, `b` and `c` give the same records; returns
/// the path of `merge(merge(a, b), c)`.
fn assert_three_groupings(dir: &Path, a: &Path, b: &Path, c: &Path, now: u64) -> PathBuf {
    let o = |n: &str| dir.join(n);
    merge(a, b, &o("ab.mnemo"), now);
    merge(&o("ab.mnemo"), c, &o("ab_c.mnemo"), now);
    merge(b, c, &o("bc.mnemo"), now);
    merge(a, &o("bc.mnemo"), &o("a_bc.mnemo"), now);
    merge(a, c, &o("ac.mnemo"), now);
    merge(&o("ac.mnemo"), b, &o("ac_b.mnemo"), now);
    assert_same_records(&o("a_bc.mnemo"), &o("ab_c.mnemo"), "merge(a, merge(b, c))");
    assert_same_records(&o("ac_b.mnemo"), &o("ab_c.mnemo"), "merge(merge(a, c), b)");
    o("ab_c.mnemo")
}

#[test]
fn a_key_written_again_after_its_live_version_was_forgotten_merges_with_itself_unchanged() {
    // Forgetting a key's live version ends its chain, and the version
    // written after starts a new one. Merged with itself, the copy is
    // unchanged, though its clock is an hour behind its ids' time.
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.mnemo");
    let mut db = Mnemo::create(&a, PW, cfg()).unwrap();
    db.__freeze_hlc_clock_for_testing(Some(wall_ms() - 3_600_000));
    db.remember_keyed(Memory::new("k, first", MemoryType::Semantic, v(1.0)), "k")
        .unwrap();
    let (gone, _) = db
        .remember_keyed(Memory::new("k, second", MemoryType::Semantic, v(2.0)), "k")
        .unwrap();
    db.delete(&gone).unwrap();
    let (again, _) = db
        .remember_keyed(Memory::new("k, again", MemoryType::Semantic, v(3.0)), "k")
        .unwrap();
    db.close().unwrap();
    let (a2, out) = (dir.path().join("a2.mnemo"), dir.path().join("aa.mnemo"));
    copy(&a, &a2);
    merge(&a, &a2, &out, wall_ms());
    assert_same_records(&out, &a, "merge(a, a)");
    let mut db = Mnemo::open_read_only(&out, PW).unwrap();
    assert_eq!(meta(&db.get(&again).unwrap(), "_mnemo.supersedes"), None);
}

#[test]
fn a_version_superseded_in_a_later_copy_keeps_its_place_and_loses_its_flag() {
    // a and c each write the key, c a millisecond later; d, copied from a,
    // writes over a's version, which takes a later stamp there. Every
    // grouping orders a's version by the stamp it was written with, and a
    // merge's flag on it goes once d's version of it, which descends from
    // the one flagged, arrives.
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.mnemo");
    base(&base_path);
    let o = |n: &str| dir.path().join(n);
    let (a, c, d) = (o("a.mnemo"), o("c.mnemo"), o("d.mnemo"));
    let t = wall_ms() + 10_000;
    let write = |p: &Path, who: &str, at: u64, text: &str, x: f32| {
        let mut db = open(p);
        db.set_author(Some(who.into()));
        db.__freeze_hlc_clock_for_testing(Some(at));
        let (id, _) = db
            .remember_keyed(Memory::new(text, MemoryType::Semantic, v(x)), "k")
            .unwrap();
        db.close().unwrap();
        id
    };
    copy(&base_path, &a);
    copy(&base_path, &c);
    let ka = write(&a, "worker-a", t + 1, "k is a", 2.0);
    let kc = write(&c, "worker-c", t + 1, "k is c", 3.0);
    copy(&a, &d);
    let kd = write(&d, "worker-d", t, "k is d", 4.0);
    let out = assert_three_groupings(dir.path(), &a, &c, &d, t + 1_000);
    let mut db = Mnemo::open_read_only(&out, PW).unwrap();
    let id = |u: Ulid| Some(Value::String(u.to_string()));
    let first = db.get(&ka).unwrap();
    assert_eq!(meta(&first, "_mnemo.superseded_by").cloned(), id(kc));
    assert_eq!(meta(&first, "_mnemo.merge_conflict"), None);
    assert_eq!(
        meta(&db.get(&kd).unwrap(), "_mnemo.supersedes").cloned(),
        id(kc)
    );
}

#[test]
fn a_flag_goes_when_the_version_that_beat_it_is_deleted() {
    // a (its clock an hour ahead) and c each write the key; d, copied from
    // a, deletes a's version. c's version is live in every grouping, and
    // unflagged, as if the delete had come first.
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.mnemo");
    base(&base_path);
    let o = |n: &str| dir.path().join(n);
    let (a, c, d) = (o("a.mnemo"), o("c.mnemo"), o("d.mnemo"));
    let t = wall_ms();
    let mut ids = Vec::new();
    for (p, at, x) in [(&a, t + 3_600_000, 2.0), (&c, t, 3.0)] {
        copy(&base_path, p);
        let mut db = open(p);
        db.__freeze_hlc_clock_for_testing(Some(at));
        let (id, _) = db
            .remember_keyed(
                Memory::new(format!("k is {x}"), MemoryType::Semantic, v(x)),
                "k",
            )
            .unwrap();
        ids.push(id);
        db.close().unwrap();
    }
    copy(&a, &d);
    let mut db = open(&d);
    db.delete(&ids[0]).unwrap();
    db.close().unwrap();
    let out = assert_three_groupings(dir.path(), &a, &c, &d, t + 1_000);
    let mut db = Mnemo::open_read_only(&out, PW).unwrap();
    let live = db.get(&ids[1]).unwrap();
    assert_eq!(meta(&live, "_mnemo.superseded_by"), None);
    assert_eq!(meta(&live, "_mnemo.merge_conflict"), None);
}

#[test]
fn a_stamp_is_never_before_the_writes_own_update_time() {
    // A record's number is the larger of its stamp and its update time.
    // Kept one, a later version always outnumbers the one it was edited
    // from, so a merge never lets an earlier text win over its own edit,
    // even with the clock an hour behind.
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.mnemo");
    base(&p);
    let mut db = open(&p);
    db.__freeze_hlc_clock_for_testing(Some(wall_ms() - 3_600_000));
    let id = db
        .essence_add(EssenceScope::Project, EssenceSection::Open, "an item", None)
        .unwrap();
    db.essence_update(&id, "the item, edited", None).unwrap();
    let m = db.get(&id).unwrap();
    let stamp = meta(&m, "_mnemo.hlc").and_then(Value::as_u64).unwrap();
    let updated = meta(&m, "_mnemo.updated_at")
        .and_then(Value::as_i64)
        .unwrap();
    assert!(stamp >= updated as u64 * 1000, "{stamp} before {updated}");
}

#[test]
fn one_text_written_in_two_copies_in_one_millisecond_merges_alike_in_every_grouping() {
    // a and c reword an essence item alike, a second apart by the wall
    // clock but in one millisecond by theirs; b resolves it. The merged
    // item takes the later update time whichever body wins, so every
    // grouping gives the same record.
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.mnemo");
    base(&base_path);
    let mut db = open(&base_path);
    let item = db
        .essence_add(EssenceScope::Project, EssenceSection::Open, "an item", None)
        .unwrap();
    db.close().unwrap();
    let o = |n: &str| dir.path().join(n);
    let (a, b, c) = (o("a.mnemo"), o("b.mnemo"), o("c.mnemo"));
    let t = wall_ms() + 3_600_000;
    let secs = || wall_ms() / 1000;
    for p in [&a, &b, &c] {
        copy(&base_path, p);
        let mut db = open(p);
        db.__freeze_hlc_clock_for_testing(Some(t));
        if p == &b {
            db.essence_resolve(&item).unwrap();
        } else {
            let s = secs();
            while secs() == s {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            db.essence_update(&item, "the item, reworded", None)
                .unwrap();
        }
        db.close().unwrap();
    }
    let out = assert_three_groupings(dir.path(), &a, &b, &c, t + 1_000);
    let mut db = Mnemo::open_read_only(&out, PW).unwrap();
    // One stamp: which text wins is the tie-break's, the status is final.
    let m = db.get(&item).unwrap();
    assert_eq!(m.metadata["_mnemo.status"], "resolved");
}

#[test]
fn a_key_written_alone_in_two_copies_stays_apart_from_a_chain_a_copy_ended() {
    // a and c each write the key, which had no version; d, copied from a,
    // writes over a's version and forgets its own. a's chain ends at d's
    // forgotten version in every grouping, though merging a with c alone
    // links the two: a later merge reads the links a write made, never the
    // ones a merge made (decision.v0210-chain-rule).
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.mnemo");
    base(&base_path);
    let o = |n: &str| dir.path().join(n);
    let (a, c, d) = (o("a.mnemo"), o("c.mnemo"), o("d.mnemo"));
    let t = wall_ms();
    let write = |p: &Path, at: u64, text: &str, x: f32| {
        let mut db = open(p);
        db.__freeze_hlc_clock_for_testing(Some(at));
        let (id, _) = db
            .remember_keyed(Memory::new(text, MemoryType::Semantic, v(x)), "k")
            .unwrap();
        db.close().unwrap();
        id
    };
    copy(&base_path, &a);
    copy(&base_path, &c);
    let ka = write(&a, t - 3_600_000, "k is 0", 2.0);
    let kc = write(&c, t, "k is 0", 3.0);
    copy(&a, &d);
    let kd = write(&d, t, "k is 1", 4.0);
    let mut db = open(&d);
    db.delete(&kd).unwrap();
    db.close().unwrap();
    let out = assert_three_groupings(dir.path(), &a, &c, &d, t + 1_000);
    let mut db = Mnemo::open_read_only(&out, PW).unwrap();
    let first = db.get(&ka).unwrap();
    assert_eq!(
        meta(&first, "_mnemo.superseded_by").cloned(),
        Some(Value::String(kd.to_string()))
    );
    let lone = db.get(&kc).unwrap();
    assert_eq!(meta(&lone, "_mnemo.supersedes"), None);
    assert_eq!(meta(&lone, "_mnemo.merge_linked"), None);
}

#[test]
fn a_link_a_merge_made_is_marked_and_a_tombstone_keeps_what_was_written() {
    // Both copies write over the key's base version. The merge links the
    // later after the earlier and marks it with the version it was written
    // over; forgetting it in the merged file leaves that on its tombstone.
    let p = pair();
    let base_k = {
        let mut a = open(&p.a);
        let (k, _) = a
            .remember_keyed(Memory::new("k, base", MemoryType::Semantic, v(1.0)), "k")
            .unwrap();
        a.close().unwrap();
        copy(&p.a, &p.b);
        k
    };
    let t = wall_ms() + 10_000;
    let mut ids = Vec::new();
    for (path, at, x) in [(&p.a, t, 2.0), (&p.b, t + 5, 3.0)] {
        let mut db = open(path);
        db.__freeze_hlc_clock_for_testing(Some(at));
        let (id, _) = db
            .remember_keyed(
                Memory::new(format!("k is {x}"), MemoryType::Semantic, v(x)),
                "k",
            )
            .unwrap();
        ids.push(id);
        db.close().unwrap();
    }
    let (out, _) = p.check_laws(t + 1_000);
    let mut db = open(&out);
    let later = db.get(&ids[1]).unwrap();
    let id = |u: Ulid| Some(Value::String(u.to_string()));
    assert_eq!(meta(&later, "_mnemo.supersedes").cloned(), id(ids[0]));
    assert_eq!(meta(&later, "_mnemo.merge_linked").cloned(), id(base_k));
    assert_eq!(meta(&db.get(&ids[0]).unwrap(), "_mnemo.merge_linked"), None);
    db.delete(&ids[1]).unwrap();
    db.close().unwrap();
    let tomb = records(&out)
        .into_iter()
        .find(|r| r["metadata"]["_mnemo.tombstone_of"] == ids[1].to_string())
        .expect("a tombstone");
    assert_eq!(
        tomb["metadata"]["_mnemo.tombstone_supersedes"],
        Value::String(base_k.to_string())
    );
}

#[test]
fn a_turn_saved_again_after_its_thread_was_forgotten_is_kept() {
    // a saves a turn and forgets the thread; d, copied from a, saves the
    // turn again; c saves it too, never having seen the forget, before d
    // or after it (the proptest's seed 6e5fb2a2: before, so c's copy has
    // the earlier id). The forget takes every copy saved before it: d's
    // turn stays, c's goes, in every grouping, and merging a with its own
    // later copy d changes nothing.
    for c_first in [false, true] {
        saved_again_case(c_first);
    }
}

fn saved_again_case(c_first: bool) {
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.mnemo");
    base(&base_path);
    let o = |n: &str| dir.path().join(n);
    let (a, c, d) = (o("a.mnemo"), o("c.mnemo"), o("d.mnemo"));
    let save = |p: &Path, text: &str, x: f32| {
        let mut db = open(p);
        db.ingest_thread_with_vectors(
            "t",
            &[ThreadTurn {
                turn_id: "u1".into(),
                role: "user".into(),
                content: text.into(),
                timestamp: None,
            }],
            &[v(x)],
        )
        .unwrap();
        db.close().unwrap();
    };
    copy(&base_path, &a);
    copy(&base_path, &c);
    save(&a, "the turn", 2.0);
    let mut db = open(&a);
    db.thread_forget("t").unwrap();
    db.close().unwrap();
    copy(&a, &d);
    if c_first {
        save(&c, "the turn, elsewhere", 4.0);
    }
    save(&d, "the turn, again", 3.0);
    if !c_first {
        save(&c, "the turn, elsewhere", 4.0);
    }
    let now = wall_ms() + 1_000;
    merge(&a, &d, &o("ad.mnemo"), now);
    assert_same_records(&o("ad.mnemo"), &d, "merge(a, d)");
    let out = assert_three_groupings(dir.path(), &a, &c, &d, now);
    let texts: Vec<String> = records(&out)
        .into_iter()
        .filter(|r| r["metadata"]["_mnemo.kind"] == "turn")
        .map(|r| r["content"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        texts,
        vec!["the turn, again".to_string()],
        "c first: {c_first}"
    );
}

/// Copies of `base` that each write the key once, at their own frozen
/// clock: (path, the version's id).
fn keyed_copies(
    dir: &Path,
    base_path: &Path,
    writes: &[(&str, u64, &str, f32)],
) -> Vec<(PathBuf, Ulid)> {
    writes
        .iter()
        .map(|&(name, at, text, x)| {
            let p = dir.join(format!("{name}.mnemo"));
            copy(base_path, &p);
            let mut db = open(&p);
            db.__freeze_hlc_clock_for_testing(Some(at));
            let (id, _) = db
                .remember_keyed(Memory::new(text, MemoryType::Semantic, v(x)), "k")
                .unwrap();
            db.close().unwrap();
            (p, id)
        })
        .collect()
}

#[test]
fn key_flags_come_from_what_was_written_two_alike_and_one_not() {
    // REV-017.1 (A), its first case: a and b write the same text over the
    // base version, c a different one, later. Every version nothing
    // follows but the last is flagged, in every grouping.
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.mnemo");
    base(&base_path);
    let mut db = open(&base_path);
    db.remember_keyed(Memory::new("k, base", MemoryType::Semantic, v(1.5)), "k")
        .unwrap();
    db.close().unwrap();
    let t = wall_ms() + 10_000;
    let w = keyed_copies(
        dir.path(),
        &base_path,
        &[
            ("a", t + 10, "X", 2.0),
            ("b", t + 20, "X", 3.0),
            ("c", t + 30, "Y", 4.0),
        ],
    );
    let out = assert_three_groupings(dir.path(), &w[0].0, &w[1].0, &w[2].0, t + 1_000);
    assert!(flagged(&out, &w[0].1));
    assert!(flagged(&out, &w[1].1));
    assert!(!flagged(&out, &w[2].1));
}

#[test]
fn key_flags_come_from_what_was_written_a_version_written_over_is_not_flagged() {
    // REV-017.1 (A), its second case: a writes X, c writes Y, and d, copied
    // from a, writes Z over a's. a's version was written over, so it isn't
    // flagged in any grouping; c's is, and d's, the last, isn't.
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.mnemo");
    base(&base_path);
    let t = wall_ms() + 10_000;
    let w = keyed_copies(
        dir.path(),
        &base_path,
        &[("a", t + 10, "X", 2.0), ("c", t + 20, "Y", 3.0)],
    );
    let d = dir.path().join("d.mnemo");
    copy(&w[0].0, &d);
    let mut db = open(&d);
    db.__freeze_hlc_clock_for_testing(Some(t + 30));
    let (kd, _) = db
        .remember_keyed(Memory::new("Z", MemoryType::Semantic, v(4.0)), "k")
        .unwrap();
    db.close().unwrap();
    let out = assert_three_groupings(dir.path(), &w[0].0, &w[1].0, &d, t + 1_000);
    assert!(!flagged(&out, &w[0].1));
    assert!(flagged(&out, &w[1].1));
    assert!(!flagged(&out, &kd));
}

#[test]
fn a_version_rewritten_nine_times_keeps_the_place_it_was_first_written_in() {
    // REV-017.1 (B): a writes the key, c a version after it, then a
    // rewrites its version in place nine times, past the eight earlier
    // stamps a record lists. The merge still orders a's version by the
    // stamp it was first written with, kept once in `_mnemo.first_hlc`.
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.mnemo");
    base(&base_path);
    let t = wall_ms() + 10_000;
    let w = keyed_copies(
        dir.path(),
        &base_path,
        &[("a", t, "k is a", 2.0), ("c", t + 5, "k is c", 3.0)],
    );
    let mut db = open(&w[0].0);
    db.__freeze_hlc_clock_for_testing(Some(t + 10));
    for i in 0..9 {
        edit(&mut db, &w[0].1, &format!("k is a, take {i}"));
    }
    let first = db.get(&w[0].1).unwrap();
    assert_eq!(
        meta(&first, "_mnemo.first_hlc").and_then(Value::as_u64),
        Some(t)
    );
    db.close().unwrap();
    let out = dir.path().join("ac.mnemo");
    merge(&w[0].0, &w[1].0, &out, t + 1_000);
    let mut db = Mnemo::open_read_only(&out, PW).unwrap();
    assert_eq!(
        meta(&db.get(&w[1].1).unwrap(), "_mnemo.supersedes").cloned(),
        Some(Value::String(w[0].1.to_string()))
    );
}

#[test]
fn a_merged_edit_keeps_the_later_update_time_when_the_other_text_wins() {
    // a's clock for stamps runs ahead of its wall clock, so its edit wins
    // on number; b's edit came a second later on the wall clock. The
    // merged record keeps the later update time, whichever text it took.
    let p = pair();
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    a.__freeze_hlc_clock_for_testing(Some(wall_ms() + 100_000));
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
    edit(&mut a, "edited in a, ahead");
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    edit(&mut b, "edited in b, a second later");
    let updated =
        |db: &mut Mnemo| db.get(&p.base_fact).unwrap().metadata["_mnemo.updated_at"].clone();
    let (ua, ub) = (updated(&mut a), updated(&mut b));
    assert!(ub.as_i64() > ua.as_i64());
    a.close().unwrap();
    b.close().unwrap();
    let (out, _) = p.check_laws(wall_ms() + 200_000);
    let mut db = Mnemo::open_read_only(&out, PW).unwrap();
    let m = db.get(&p.base_fact).unwrap();
    assert_eq!(m.content, "edited in a, ahead");
    assert_eq!(m.metadata["_mnemo.updated_at"], ub);
}

#[test]
fn a_renamed_task_edited_on_both_sides_is_reported_while_its_rename_stands() {
    // Two tasks under one slug: the merge renames the later. Two copies of
    // the output then add a note to the renamed one, the same text on both
    // sides. Its rename still stands, so the merge flags it and reports it.
    let p = pair();
    let t = wall_ms();
    let (mut a, mut b) = (open(&p.a), open(&p.b));
    b.__set_clock_offset_for_testing(60);
    for db in [&mut a, &mut b] {
        db.set_author(Some("planner-a".into()));
        let mut spec = TaskSpec::new("Write the notes");
        spec.role = Some("worker".into());
        db.task_add(&spec).unwrap();
    }
    a.close().unwrap();
    b.close().unwrap();
    let (out, _) = p.check_laws(t + 120_000);
    let qdir = tempfile::tempdir().unwrap();
    let q = Pair {
        a: qdir.path().join("x.mnemo"),
        b: qdir.path().join("y.mnemo"),
        _dir: qdir,
        base_fact: p.base_fact,
    };
    copy(&out, &q.a);
    copy(&out, &q.b);
    for (path, note) in [(&q.a, "a note from x"), (&q.b, "a note from y")] {
        let mut db = open(path);
        db.set_author(Some("planner-a".into()));
        db.task_update("write-the-notes-merge-2", None, Some(note), None)
            .unwrap();
        db.close().unwrap();
    }
    let (out2, r) = q.check_laws(t + 240_000);
    assert_eq!(r.conflicts.get("task"), Some(&1), "{:?}", r.conflicts);
    let renamed = {
        let mut db = Mnemo::open(&out2, PW).unwrap();
        db.tasks(None, None)
            .unwrap()
            .into_iter()
            .find(|t| t.slug == "write-the-notes-merge-2")
            .unwrap()
            .id
    };
    assert!(flagged(&out2, &renamed));
}

#[test]
fn a_chain_a_later_version_continues_stays_linked_past_a_version_written_alone() {
    // a writes G over the base's V1, and later M over G; a2, copied from a
    // before M, forgets G. c writes X over V1, forgets it, then N, on its
    // own. G is gone, but M was written after V1 through it, so V1's chain
    // didn't end: the merge links V1 to N (stamped between), then N to M.
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.mnemo");
    base(&base_path);
    let t = wall_ms() + 10_000;
    let write = |db: &mut Mnemo, at: u64, text: &str, x: f32| -> Ulid {
        db.__freeze_hlc_clock_for_testing(Some(at));
        db.remember_keyed(Memory::new(text, MemoryType::Semantic, v(x)), "k")
            .unwrap()
            .0
    };
    let mut db = open(&base_path);
    let v1 = write(&mut db, t, "k one", 2.0);
    db.close().unwrap();
    let o = |n: &str| dir.path().join(n);
    let (a, a2, c) = (o("a.mnemo"), o("a2.mnemo"), o("c.mnemo"));
    copy(&base_path, &a);
    copy(&base_path, &c);
    let mut db = open(&a);
    let g = write(&mut db, t + 10, "k two", 3.0);
    db.close().unwrap();
    copy(&a, &a2);
    let mut db = open(&a2);
    db.delete(&g).unwrap();
    db.close().unwrap();
    let mut db = open(&c);
    let x = write(&mut db, t + 20, "k three", 4.0);
    db.delete(&x).unwrap();
    let n = write(&mut db, t + 30, "k four", 5.0);
    assert_eq!(meta(&db.get(&n).unwrap(), "_mnemo.supersedes"), None);
    db.close().unwrap();
    let mut db = open(&a);
    let m = write(&mut db, t + 40, "k five", 6.0);
    db.close().unwrap();
    let out = assert_three_groupings(dir.path(), &a, &a2, &c, t + 1_000);
    let mut db = Mnemo::open_read_only(&out, PW).unwrap();
    let sb =
        |db: &mut Mnemo, id: &Ulid| meta(&db.get(id).unwrap(), "_mnemo.superseded_by").cloned();
    let s = |id: &Ulid| Some(Value::String(id.to_string()));
    assert_eq!(sb(&mut db, &v1), s(&n));
    assert_eq!(sb(&mut db, &n), s(&m));
    assert_eq!(sb(&mut db, &m), None);
}
