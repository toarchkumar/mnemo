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
//! - Records with no stamp (v0.20.2's own) fall back to the conflict rule.

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

#[test]
fn records_with_no_stamp_edited_on_one_side_fall_back_to_a_copy() {
    // v0.20.2 wrote `fact a` with no stamp: the merge can't tell that this
    // build's edit descends from it, so it keeps both (conservative).
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
    db.close().unwrap();
    let out = dir.path().join("out.mnemo");
    let r = merge(&p[0], &p[1], &out);
    assert_eq!(r.conflicts.get("fact"), Some(&1));
    let mut db = Mnemo::open_read_only(&out, PW).unwrap();
    assert_eq!(db.get(&id).unwrap().content, "fact a, edited by v0.21");
    let copy = db
        .memories()
        .unwrap()
        .into_iter()
        .find(|m| m.metadata.get(MNEMO_META_MERGE_SOURCE).is_some())
        .unwrap();
    assert_eq!(copy.content, "fact a");
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
