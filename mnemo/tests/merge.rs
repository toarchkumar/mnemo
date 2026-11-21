//! v0.21.0 step 2 (spec.v0210 rev 2, 13.7): `Mnemo::merge_files`. Two
//! copies of one brain diverge; the merge keeps both sides' writes, applies
//! each side's deletes by id, and resolves the same record written on both
//! sides by its kind. The same inputs the other way round give the same
//! records, and merging the output again with an input changes nothing.

use std::path::{Path, PathBuf};

use mnemo_engine::{
    EssenceScope, EssenceSection, KdfParams, KeyScope, Memory, MemoryPatch, MemoryType,
    MergeOptions, MergeReport, MergeSecret, MergeSide, Mnemo, MnemoConfig, TaskSpec, Ulid,
    MNEMO_META_MERGE_CONFLICT, MNEMO_META_MERGE_SOURCE,
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
