// Shared body of `tests/embed_id1.rs`, compiled once per format: the
// including module defines `FORMAT` (8 or 9).
//
// EMBED-ID-1: every file records the identity of the first embedder
// attached to it, with or without a manifest, and refuses any other.

use mnemo_engine::{
    CachePutOpts, Embedder, EmbedderIdentity, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig,
    MnemoError, RecallRequest,
};
use tempfile::tempdir;

fn cfg() -> MnemoConfig {
    MnemoConfig {
        format_version: FORMAT,
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

struct Fake(EmbedderIdentity);

fn fake(name: &str) -> Fake {
    Fake(EmbedderIdentity {
        name: name.into(),
        dimensions: 4,
        normalize: true,
        ..Default::default()
    })
}

impl Embedder for Fake {
    fn identity(&self) -> EmbedderIdentity {
        self.0.clone()
    }
    fn embed(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|t| {
                let mut v = vec![0.0f32; 4];
                v[t.len() % 4] = 1.0;
                v
            })
            .collect())
    }
}

fn mem(i: usize) -> Memory {
    Memory::new(
        format!("memory {i}"),
        MemoryType::Semantic,
        vec![(i as f32) + 1.0, 1.0, 0.0, 0.0],
    )
}

fn assert_mismatch(r: mnemo_engine::Result<()>, stored: &str, given: &str) {
    match r {
        Err(MnemoError::EmbedderMismatch { stored: s, given: g }) => {
            assert_eq!(s.name, stored);
            assert_eq!(g.name, given);
        }
        other => panic!("expected EmbedderMismatch, got {other:?}"),
    }
}

/// A file with no manifest records the first identity and refuses a second
/// one of the same width, on this handle and after a reopen.
#[test]
fn a_file_without_a_manifest_refuses_a_second_identity() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("plain.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", cfg()).unwrap();
        assert!(db.about().unwrap().is_empty(), "no manifest");
        assert_eq!(db.embedder_identity().unwrap(), None);
        db.set_embedder(fake("model-r1")).unwrap();
        assert_eq!(db.embedder_identity().unwrap().unwrap().name, "model-r1");
        db.remember(mem(0)).unwrap();
        assert_mismatch(db.set_embedder(fake("model-r2")), "model-r1", "model-r2");
        // The slot keeps the first embedder.
        db.remember_text("still the first model", MemoryType::Semantic)
            .unwrap();
    }
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_mismatch(db.set_embedder(fake("model-r2")), "model-r1", "model-r2");
    // A different normalize flag is another identity too.
    let mut unnormalized = fake("model-r1");
    unnormalized.0.normalize = false;
    assert!(matches!(
        db.set_embedder(unnormalized),
        Err(MnemoError::EmbedderMismatch { .. })
    ));
    db.set_embedder(fake("model-r1")).unwrap();
}

/// The mismatch message starts with the old grep string and says what to do.
#[test]
fn the_mismatch_message_names_both_and_says_to_create_a_new_store() {
    let dir = tempdir().unwrap();
    let mut db = Mnemo::create(dir.path().join("m.mnemo"), "pw", cfg()).unwrap();
    db.set_embedder(fake("apple-nl-sentence-en-r1")).unwrap();
    let msg = db
        .set_embedder(fake("apple-nl-sentence-en-r2"))
        .unwrap_err()
        .to_string();
    assert!(msg.starts_with(EmbedderIdentity::MISMATCH_MSG), "{msg}");
    assert!(msg.contains("apple-nl-sentence-en-r1"), "{msg}");
    assert!(msg.contains("apple-nl-sentence-en-r2"), "{msg}");
    assert!(msg.contains("create a new store"), "{msg}");
}

/// A read-only handle checks the identity but never writes one.
#[test]
fn a_read_only_open_checks_but_never_writes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ro.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", cfg()).unwrap();
        db.remember(mem(0)).unwrap();
        db.flush().unwrap();
    }
    // No identity yet: a reader adopts its embedder, records nothing.
    {
        let mut ro = Mnemo::open_read_only(&path, "pw").unwrap();
        ro.set_embedder(fake("reader-model")).unwrap();
        assert_eq!(ro.embedder_identity().unwrap(), None);
    }
    let bytes_before = std::fs::read(&path).unwrap();
    {
        let mut ro = Mnemo::open_read_only(&path, "pw").unwrap();
        ro.set_embedder(fake("reader-model")).unwrap();
    }
    assert_eq!(std::fs::read(&path).unwrap(), bytes_before, "the reader wrote");

    // Recorded by a writer: a reader with another model is refused.
    {
        let mut db = Mnemo::open(&path, "pw").unwrap();
        db.set_embedder(fake("writer-model")).unwrap();
    }
    let mut ro = Mnemo::open_read_only(&path, "pw").unwrap();
    assert_mismatch(ro.set_embedder(fake("reader-model")), "writer-model", "reader-model");
    ro.set_embedder(fake("writer-model")).unwrap();
}

/// The record isn't a memory: counts, recall, `about`, `memories`, `get`
/// and the cache API don't see it.
#[test]
fn the_record_is_invisible_to_counts_reads_and_the_cache_api() {
    let dir = tempdir().unwrap();
    let mut db = Mnemo::create(dir.path().join("c.mnemo"), "pw", cfg()).unwrap();
    let ids: Vec<_> = (0..3).map(|i| db.remember(mem(i)).unwrap()).collect();
    db.cache_put("app", "k", b"v", CachePutOpts::default()).unwrap();
    db.flush().unwrap();
    let verified_before = db.verify().unwrap();

    db.set_embedder(fake("model")).unwrap();

    assert_eq!(db.len(), 3);
    assert_eq!(db.stats().unwrap().memories, 3);
    assert_eq!(db.stats_summary().memories, 3);
    assert_eq!(db.memories().unwrap().len(), 3);
    assert!(db.about().unwrap().is_empty());
    let hits = db
        .recall(&RecallRequest::new(vec![1.0, 1.0, 0.0, 0.0]).top_k(10))
        .unwrap();
    assert_eq!(hits.len(), 3);
    for id in &ids {
        db.get(id).unwrap();
    }
    assert_eq!(db.verify().unwrap(), verified_before);
    let cache = db.cache_stats(None);
    assert_eq!(cache.entries, 1, "only the app's own entry");

    // The reserved namespace is refused, and a full purge keeps the record.
    for r in [
        db.cache_put("_mnemo.embedder", "identity", b"x", CachePutOpts::default()),
        db.cache_delete("_mnemo.embedder", "identity"),
    ] {
        assert!(matches!(r, Err(MnemoError::Invalid(_))), "{r:?}");
    }
    assert!(db.cache_get("_mnemo.embedder", "identity").is_err());
    assert!(db.cache_purge(Some("_mnemo.embedder"), false).is_err());
    assert_eq!(db.cache_purge(None, false).unwrap(), 1);
    db.flush().unwrap();
    assert_mismatch(db.set_embedder(fake("other")), "model", "other");
}

/// Compaction carries the identity, and doesn't
/// count it as a cache entry.
#[test]
fn the_identity_survives_compaction() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("compact.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", cfg()).unwrap();
        db.set_embedder(fake("model")).unwrap();
        for i in 0..5 {
            db.remember(mem(i)).unwrap();
        }
        db.flush().unwrap();
    }
    let report = Mnemo::compact_file(path.to_str().unwrap(), "pw").unwrap();
    assert_eq!(report.after, 5);
    assert_eq!((report.cache_before, report.cache_after), (0, 0));
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.stats().unwrap().format_version, FORMAT);
    assert_eq!(db.len(), 5);
    assert_eq!(db.embedder_identity().unwrap().unwrap().name, "model");
    assert_mismatch(db.set_embedder(fake("other")), "model", "other");
}

/// The rekeyed exports (passphrase and raw key) and the binding's import
/// path (a read-only source exported under a new key) carry it.
#[test]
fn the_identity_survives_the_rekeyed_exports_and_the_import() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("src.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", cfg()).unwrap();
        db.set_embedder(fake("model")).unwrap();
        db.remember(mem(0)).unwrap();
        db.flush().unwrap();
    }

    let by_passphrase = dir.path().join("export.mnemo");
    {
        let mut db = Mnemo::open(&path, "pw").unwrap();
        db.export_rekeyed(&by_passphrase, "move-pw", KdfParams::fast())
            .unwrap();
    }
    let mut copy = Mnemo::open(&by_passphrase, "move-pw").unwrap();
    assert_eq!(copy.len(), 1);
    assert_mismatch(copy.set_embedder(fake("other")), "model", "other");
    drop(copy);

    // The import: open the export read-only, write it under a device key.
    let imported = dir.path().join("imported.mnemo");
    let key = Mnemo::generate_key();
    {
        let mut source = Mnemo::open_read_only(&by_passphrase, "move-pw").unwrap();
        source.export_rekeyed_with_key(&imported, &key).unwrap();
    }
    let mut db = Mnemo::open_with_key(&imported, &key).unwrap();
    assert_eq!(db.len(), 1);
    assert_eq!(db.embedder_identity().unwrap().unwrap().name, "model");
    assert_mismatch(db.set_embedder(fake("other")), "model", "other");
    db.set_embedder(fake("model")).unwrap();
}

/// A file with a manifest behaves as before: the first registration writes
/// the manifest's identity (and now the record too), a mismatch is refused,
/// and a manifest that already names the embedder is honoured.
#[test]
fn a_file_with_a_manifest_behaves_as_before() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("manifest.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", cfg()).unwrap();
        db.remember(Memory::scaffold_manifest(4)).unwrap();
        db.flush().unwrap();
        db.set_embedder(fake("model")).unwrap();
        let manifest = &db.about().unwrap()[0];
        assert_eq!(manifest.metadata["embedder"]["name"], "model");
        assert_eq!(db.len(), 1, "only the manifest");
    }
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_mismatch(db.set_embedder(fake("other")), "model", "other");
    db.set_embedder(fake("model")).unwrap();
}

/// A manifest identity from before EMBED-ID-1 (no record) is checked, and
/// the record is written to match on the first registration.
#[test]
fn a_manifest_identity_without_a_record_is_checked_then_recorded() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("old-manifest.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg()).unwrap();
    let mut m = Memory::scaffold_manifest(4);
    m.metadata.insert(
        "embedder".into(),
        serde_json::json!({ "name": "seeded", "dimensions": 4, "normalize": true }),
    );
    db.remember(m).unwrap();
    db.flush().unwrap();
    assert_eq!(db.embedder_identity().unwrap(), None);
    assert_mismatch(db.set_embedder(fake("other")), "seeded", "other");
    assert_eq!(db.embedder_identity().unwrap(), None, "a refused call records nothing");
    db.set_embedder(fake("seeded")).unwrap();
    assert_eq!(db.embedder_identity().unwrap().unwrap().name, "seeded");
}

/// A manifest and a record that disagree are reported with both names.
#[test]
fn a_manifest_and_record_that_disagree_name_both() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("conflict.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg()).unwrap();
    db.set_embedder(fake("recorded")).unwrap();
    let mut m = Memory::scaffold_manifest(4);
    m.metadata.insert(
        "embedder".into(),
        serde_json::json!({ "name": "on-manifest", "dimensions": 4, "normalize": true }),
    );
    db.remember(m).unwrap();
    db.flush().unwrap();
    drop(db);
    let mut db = Mnemo::open(&path, "pw").unwrap();
    match db.set_embedder(fake("recorded")) {
        Err(MnemoError::Invalid(msg)) => {
            assert!(msg.starts_with(EmbedderIdentity::MISMATCH_MSG), "{msg}");
            assert!(msg.contains("on-manifest") && msg.contains("recorded"), "{msg}");
        }
        other => panic!("expected Invalid naming both, got {other:?}"),
    }
}

/// The dimension check still comes first.
#[test]
fn a_wrong_width_is_still_a_dimension_mismatch() {
    let dir = tempdir().unwrap();
    let mut db = Mnemo::create(dir.path().join("w.mnemo"), "pw", cfg()).unwrap();
    db.set_embedder(fake("model")).unwrap();
    let mut wide = fake("model");
    wide.0.dimensions = 8;
    assert!(matches!(
        db.set_embedder(wide),
        Err(MnemoError::DimensionMismatch { expected: 4, got: 8 })
    ));
}
