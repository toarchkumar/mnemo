//! v0.13 step 2a: compaction (and so the rekeyed exports and the import)
//! carries a file that holds legacy all-zero vectors, instead of refusing
//! it. The fixture `tests/fixtures/v8/v8_zero_vectors.mnemo` (see
//! `examples/gen_v8_zero_vector_fixture.rs`) is a v8 file with a zero-vector
//! scaffold manifest, a zero-vector ordinary memory and three ordinary ones.

use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoError, RecallRequest};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const PASSPHRASE: &str = "v8-fixture-passphrase";

/// A copy of the fixture in a fresh directory (the tests never touch the
/// committed file).
fn fixture_copy() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v8/v8_zero_vectors.mnemo");
    let dst = dir.path().join("zero.mnemo");
    std::fs::copy(src, &dst).unwrap();
    (dir, dst)
}

fn is_zero(v: &[f32]) -> bool {
    v.iter().all(|x| *x == 0.0)
}

#[test]
fn verify_counts_the_zero_vector_memories() {
    let (_dir, path) = fixture_copy();
    let mut db = Mnemo::open_read_only(&path, PASSPHRASE).unwrap();
    let report = db.verify_report().unwrap();
    assert_eq!(report.records, 5);
    assert_eq!(report.zero_vector_memories, 2);
    assert_eq!(db.verify().unwrap(), 5);
}

#[test]
fn compaction_carries_zero_vectors_and_fixes_the_legacy_manifest() {
    let (_dir, path) = fixture_copy();
    let report = Mnemo::compact_file(path.to_str().unwrap(), PASSPHRASE).unwrap();
    assert_eq!((report.before, report.after), (5, 5));
    assert_eq!(report.zero_vector_manifests, 1);
    assert_eq!(report.zero_vector_memories, 1);

    let mut db = Mnemo::open_read_only(&path, PASSPHRASE).unwrap();
    assert_eq!(db.stats_summary().format_version, 9);
    assert_eq!(db.len(), 5);
    // The manifest now has the sentinel vector a current manifest has.
    let manifest = db
        .about()
        .unwrap()
        .into_iter()
        .next()
        .expect("the manifest");
    assert_eq!(manifest.vector, Memory::scaffold_manifest(8).vector);
    // The other zero-vector memory is carried unchanged.
    let all = db.memories().unwrap();
    let legacy = all
        .iter()
        .find(|m| m.content == "A note written with an all-zero vector")
        .expect("the zero-vector memory survives");
    assert!(is_zero(&legacy.vector), "{:?}", legacy.vector);
    for i in 0..3 {
        assert!(all
            .iter()
            .any(|m| m.content == format!("ordinary memory {i}")));
    }
    let after = db.verify_report().unwrap();
    assert_eq!(
        after.zero_vector_memories, 1,
        "only the ordinary one is left"
    );

    // Recall over a carried zero vector stays finite: it scores 0.0 on
    // similarity, as it did before compaction.
    // Axis 1: away from the sentinel spike on axis 0.
    let mut q = vec![0.05f32; 8];
    q[1] = 1.0;
    let hits = db
        .recall(&RecallRequest::new(q).top_k(10).track_access(false))
        .unwrap();
    assert_eq!(hits.len(), 5);
    for h in &hits {
        assert!(h.score.is_finite() && h.similarity.is_finite(), "{h:?}");
    }
    assert_eq!(hits[0].memory.content, "ordinary memory 1");
    let zero_hit = hits
        .iter()
        .find(|h| is_zero(&h.memory.vector))
        .expect("the zero-vector memory");
    assert_eq!(zero_hit.similarity, 0.0);
}

#[test]
fn the_rekeyed_exports_and_the_import_carry_them_too() {
    let (dir, path) = fixture_copy();

    // The passphrase export, from a read-only handle (a v8 file stays
    // readable after the v8 sunset).
    let exported = dir.path().join("export.mnemo");
    {
        let mut src = Mnemo::open_read_only(&path, PASSPHRASE).unwrap();
        let report = src
            .export_rekeyed(&exported, "move-pw", KdfParams::fast())
            .unwrap();
        assert_eq!(report.after, 5);
        assert_eq!(
            (report.zero_vector_manifests, report.zero_vector_memories),
            (1, 1)
        );
    }
    let copy = Mnemo::open_read_only(&exported, "move-pw").unwrap();
    assert_eq!(copy.len(), 5);
    assert_eq!(copy.stats_summary().format_version, 9);
    drop(copy);

    // The import: the export opened read-only, written under a device key.
    let imported = dir.path().join("imported.mnemo");
    let key = Mnemo::generate_key();
    {
        let mut src = Mnemo::open_read_only(&exported, "move-pw").unwrap();
        src.export_rekeyed_with_key(&imported, &key).unwrap();
    }
    let db = Mnemo::open_with_key(&imported, &key).unwrap();
    assert_eq!(db.len(), 5);
}

#[test]
fn a_new_zero_vector_is_still_refused() {
    let (_dir, path) = fixture_copy();
    Mnemo::compact_file(path.to_str().unwrap(), PASSPHRASE).unwrap();
    let mut db = Mnemo::open(&path, PASSPHRASE).unwrap();
    let zero = Memory::new("never written", MemoryType::Semantic, vec![0.0; 8]);
    match db.remember(zero) {
        Err(MnemoError::Invalid(msg)) => assert!(msg.contains("all-zero vector refused"), "{msg}"),
        other => panic!("expected the zero-vector refusal, got {other:?}"),
    }
    assert_eq!(db.len(), 5);
}
