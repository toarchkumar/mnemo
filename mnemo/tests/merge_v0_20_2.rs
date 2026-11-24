//! v0.21.0 step 2 (spec.v0210 rev 2, RB-019.8 case 5): merges of files an
//! older build wrote, read from committed fixtures in
//! `tests/fixtures/v0_20_2/`. Their generator (that folder's `generator`)
//! runs the published v0.20.2 engine; the inputs it starts from are written
//! by this build with `cargo test --test merge_v0_20_2 -- --ignored`.
//!
//! - A delete v0.20.2 made leaves only a deleted row, which the merge reads
//!   as a delete of that id.
//! - A record v0.20.2 edited in place keeps the stamp it had: against the
//!   unedited version, equal stamps with different content are a conflict
//!   (decision.v0210-ancestry), never a silent loss.
//! - A tombstone that v0.20.2's compaction wrote back as a fact is still a
//!   tombstone to the merge (it reads the record's kind, not its row).
//! - A record with no stamp (v0.20.2's own) that this build edits names
//!   what it was edited from, so the edit wins over the copy that didn't
//!   change it (RB-020.5); edits by v0.20.2 in both still conflict.
//! - A key v0.20.2 wrote keeps this build's later version live through any
//!   merge (RB-020.1), and a delete v0.20.2 made stays deleted when the
//!   output is merged again (RB-020.2).

use std::path::{Path, PathBuf};

use mnemo_engine::{
    KdfParams, Memory, MemoryPatch, MemoryType, MergeOptions, MergeSecret, Mnemo, MnemoConfig,
    MNEMO_META_MERGE_SOURCE,
};

const PW: &str = "v0-20-2-fixture-passphrase";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/v0_20_2")
        .join(name)
}

fn v(x: f32) -> Vec<f32> {
    vec![x, 1.0, 0.5, 0.25]
}

fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Copies of committed fixtures in a temp folder (a merge never writes its
/// inputs, but an edit before it does).
fn copies(names: &[&str]) -> (tempfile::TempDir, Vec<PathBuf>) {
    let dir = tempfile::tempdir().unwrap();
    let paths = names
        .iter()
        .map(|n| {
            let p = dir.path().join(n);
            std::fs::copy(fixture(n), &p).unwrap();
            p
        })
        .collect();
    (dir, paths)
}

fn merge(a: &Path, b: &Path, out: &Path) -> mnemo_engine::MergeReport {
    Mnemo::merge_files(
        a,
        MergeSecret::Passphrase(PW),
        b,
        MergeSecret::Passphrase(PW),
        out,
        &MergeOptions::at(wall_ms()),
    )
    .unwrap()
}

fn contents(p: &Path) -> Vec<String> {
    let mut db = Mnemo::open_read_only(p, PW).unwrap();
    let mut c: Vec<String> = db
        .memories()
        .unwrap()
        .into_iter()
        .filter(|m| {
            m.metadata.get("_mnemo.kind").is_none()
                && m.metadata.get("topic").and_then(|t| t.as_str()) != Some("manifest")
        })
        .map(|m| m.content)
        .collect();
    c.sort();
    c
}

/// Writes the v0.21 inputs the generator starts from. Run it, then the
/// generator, then commit the folder's `.mnemo` files.
#[test]
#[ignore]
fn write_the_v0_21_inputs() {
    let cfg = || MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    for n in [
        "edited.in.mnemo",
        "compacted.base.mnemo",
        "compacted.in.mnemo",
    ] {
        let _ = std::fs::remove_file(fixture(n));
    }
    let mut db = Mnemo::create(fixture("edited.in.mnemo"), PW, cfg()).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.remember(Memory::new(
        "stamped by v0.21",
        MemoryType::Semantic,
        v(1.0),
    ))
    .unwrap();
    db.close().unwrap();

    let mut db = Mnemo::create(fixture("compacted.base.mnemo"), PW, cfg()).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.remember(Memory::new("kept", MemoryType::Semantic, v(1.0)))
        .unwrap();
    db.remember(Memory::new(
        "deleted by v0.21",
        MemoryType::Semantic,
        v(2.0),
    ))
    .unwrap();
    db.close().unwrap();
    std::fs::copy(
        fixture("compacted.base.mnemo"),
        fixture("compacted.in.mnemo"),
    )
    .unwrap();
    let mut db = Mnemo::open(fixture("compacted.in.mnemo"), PW).unwrap();
    let id = db
        .memories()
        .unwrap()
        .into_iter()
        .find(|m| m.content == "deleted by v0.21")
        .unwrap()
        .id;
    db.delete(&id).unwrap();
    db.close().unwrap();
}

#[test]
fn an_older_builds_delete_with_no_tombstone_is_a_delete() {
    let (dir, p) = copies(&["plain_base.mnemo", "plain_deleted.mnemo"]);
    let out = dir.path().join("out.mnemo");
    let r = merge(&p[1], &p[0], &out);
    assert_eq!(contents(&out), ["fact a", "fact c"]);
    assert_eq!(r.removed.len(), 1);
    // Neither file says when a v0.21 build first wrote in it.
    assert_eq!(r.hlc_since, [None, None]);
}

/// Every record of the file at `p` as JSON, by id (vectors left out: the
/// fp16 column reads them back alike).
fn records(p: &Path) -> Vec<serde_json::Value> {
    let mut db = Mnemo::open_read_only(p, PW).unwrap();
    let mut recs = db.memories().unwrap();
    recs.sort_by_key(|m| m.id);
    recs.iter()
        .map(|m| {
            let mut j = serde_json::to_value(m).unwrap();
            j.as_object_mut().unwrap().remove("vector");
            j
        })
        .collect()
}

fn live_text(p: &Path, key: &str) -> Vec<String> {
    let mut db = Mnemo::open_read_only(p, PW).unwrap();
    db.history(key, None)
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect()
}

#[test]
fn a_key_an_older_build_wrote_keeps_this_builds_version_live_through_a_merge() {
    // RB-020.1: v0.20.2 wrote `k` once and `k2` twice; this build writes
    // each again, which rewrites the old version in place as superseded.
    let (dir, p) = copies(&["keyed.mnemo"]);
    let o = |n: &str| dir.path().join(n);
    std::fs::copy(&p[0], o("untouched.mnemo")).unwrap();
    let mut db = Mnemo::open(&p[0], PW).unwrap();
    for (key, text, x) in [
        ("k", "k as v0.21 wrote it", 5.0),
        ("k2", "k2 third, by v0.21", 6.0),
    ] {
        db.remember_keyed(Memory::new(text, MemoryType::Semantic, v(x)), key)
            .unwrap();
    }
    db.close().unwrap();
    let before = (live_text(&p[0], "k"), live_text(&p[0], "k2"));
    assert_eq!(before.0, ["k as v0.21 wrote it", "k as v0.20.2 wrote it"]);
    assert_eq!(
        before.1,
        [
            "k2 third, by v0.21",
            "k2 second, by v0.20.2",
            "k2 first, by v0.20.2"
        ]
    );
    std::fs::copy(&p[0], o("x2.mnemo")).unwrap();
    // The copy with itself, and with the copy this build never wrote, in
    // both orders.
    for (a, b, out) in [
        (p[0].clone(), o("x2.mnemo"), "xx.mnemo"),
        (p[0].clone(), o("untouched.mnemo"), "xu.mnemo"),
        (o("untouched.mnemo"), p[0].clone(), "ux.mnemo"),
    ] {
        let r = merge(&a, &b, &o(out));
        assert!(r.conflicts.is_empty(), "{out}: {r:?}");
        assert_eq!(
            (live_text(&o(out), "k"), live_text(&o(out), "k2")),
            before,
            "{out}"
        );
    }
    assert_eq!(records(&o("xx.mnemo")), records(&p[0]), "merge(x, x) = x");
    assert_eq!(records(&o("xu.mnemo")), records(&o("ux.mnemo")));
    merge(&o("xu.mnemo"), &o("untouched.mnemo"), &o("xu_u.mnemo"));
    merge(&o("xu.mnemo"), &p[0], &o("xu_x.mnemo"));
    assert_eq!(records(&o("xu_u.mnemo")), records(&o("xu.mnemo")));
    assert_eq!(records(&o("xu_x.mnemo")), records(&o("xu.mnemo")));
}

#[test]
fn an_older_builds_delete_stays_deleted_when_the_output_is_merged_again() {
    // RB-020.2: the output keeps the delete as a tombstone.
    let (dir, p) = copies(&["plain_base.mnemo", "plain_deleted.mnemo"]);
    let o = |n: &str| dir.path().join(n);
    let r = merge(&p[1], &p[0], &o("out1.mnemo"));
    assert_eq!(r.older_deletes, 1);
    for (a, b, out) in [
        (o("out1.mnemo"), p[0].clone(), "again_base.mnemo"),
        (p[0].clone(), o("out1.mnemo"), "base_again.mnemo"),
        (o("out1.mnemo"), p[1].clone(), "again_deleted.mnemo"),
        (o("out1.mnemo"), o("out1.mnemo"), "again_out.mnemo"),
    ] {
        let r = merge(&a, &b, &o(out));
        assert_eq!(contents(&o(out)), ["fact a", "fact c"], "{out}");
        assert_eq!(r.older_deletes, 0, "{out}: the tombstone is already there");
        assert_eq!(records(&o(out)), records(&o("out1.mnemo")), "{out}");
    }
    // Either order gives the same tombstone.
    merge(&p[0], &p[1], &o("out2.mnemo"));
    assert_eq!(records(&o("out2.mnemo")), records(&o("out1.mnemo")));
}

#[test]
fn a_one_sided_first_edit_of_an_older_record_wins_with_no_copy() {
    // RB-020.5: v0.20.2 wrote `fact a` with no stamp; this build's edit
    // lists that version among the ones before it, so it descends from the
    // copy that wasn't edited.
    let (dir, p) = copies(&["plain_base.mnemo", "plain_deleted.mnemo"]);
    let mut db = Mnemo::open(&p[0], PW).unwrap();
    let id = db
        .memories()
        .unwrap()
        .into_iter()
        .find(|m| m.content == "fact a")
        .unwrap()
        .id;
    db.update(
        &id,
        MemoryPatch {
            content: Some("fact a, edited by v0.21".into()),
            vector: Some(v(5.0)),
            ..Default::default()
        },
    )
    .unwrap();
    let e = db.get(&id).unwrap();
    assert_eq!(
        e.metadata.get("_mnemo.first_hlc").and_then(|x| x.as_u64()),
        Some(e.id.timestamp_ms()),
        "its first stamp is when v0.20.2 created it (RB-020.1)"
    );
    db.close().unwrap();
    let out = dir.path().join("out.mnemo");
    let r = merge(&p[0], &p[1], &out);
    assert!(r.conflicts.is_empty(), "{r:?}");
    let mut db = Mnemo::open_read_only(&out, PW).unwrap();
    assert_eq!(db.get(&id).unwrap().content, "fact a, edited by v0.21");
    assert!(
        !db.memories()
            .unwrap()
            .iter()
            .any(|m| m.metadata.get(MNEMO_META_MERGE_SOURCE).is_some()),
        "no conflict copy"
    );
}

#[test]
fn an_older_builds_edit_of_a_stamped_record_is_a_conflict_not_a_loss() {
    let (dir, p) = copies(&["edited.in.mnemo", "edited.mnemo"]);
    let out = dir.path().join("out.mnemo");
    let r = merge(&p[0], &p[1], &out);
    assert_eq!(r.conflicts.get("fact"), Some(&1), "{r:?}");
    let c = contents(&out);
    assert!(
        c.contains(&"edited by v0.20.2".to_string()) && c.contains(&"stamped by v0.21".to_string()),
        "{c:?}"
    );
}

#[test]
fn a_tombstone_an_older_build_compacted_still_deletes() {
    let (dir, p) = copies(&["compacted.base.mnemo", "compacted.mnemo"]);
    {
        let mut db = Mnemo::open_read_only(&p[1], PW).unwrap();
        assert_eq!(
            db.mistagged_coordination().unwrap(),
            1,
            "v0.20.2 wrote it as a fact"
        );
    }
    let out = dir.path().join("out.mnemo");
    let r = merge(&p[0], &p[1], &out);
    assert_eq!(contents(&out), ["kept"]);
    assert_eq!(r.removed.len(), 1);
    let mut db = Mnemo::open_read_only(&out, PW).unwrap();
    assert_eq!(db.kind_counts().unwrap().tombstones, 1);
    assert_eq!(
        db.mistagged_coordination().unwrap(),
        0,
        "the output's rows are right"
    );
}
