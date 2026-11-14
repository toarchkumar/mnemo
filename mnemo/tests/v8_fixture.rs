//! The v8 sunset (v0.13): v8 coverage as reads of the committed v8
//! fixtures in `tests/fixtures/v8/`, which the published v0.12.3 engine
//! wrote (`tests/fixtures/v8/generator`; what they hold is in
//! `expected.json`).
//!
//! - A read-only open reads every kind of record: memories, a keyed
//!   history, a thread and its essence, the project essence, the manifest,
//!   the result cache, the index and the snapshots.
//! - A read-write open is `FormatTooOld`, and nothing new can be v8.
//! - `compact_file` converts the file to v9 with all of it, after which it's
//!   writable.

use mnemo_engine::{
    Embedder, EmbedderIdentity, EssenceScope, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig,
    MnemoError, RecallRequest,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v8")
}

fn expected() -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixtures().join("expected.json")).unwrap())
        .unwrap()
}

/// A copy of a fixture in a fresh directory: the tests never touch the
/// committed files.
fn copy(name: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join(name);
    std::fs::copy(fixtures().join(name), &dst).unwrap();
    (dir, dst)
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

/// Everything `expected.json` records, read through `db`.
fn check_contents(db: &mut Mnemo, e: &Value) {
    assert_eq!(db.len() as u64, e["memories"].as_u64().unwrap());
    let contents: Vec<String> = db
        .memories()
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    for c in strings(&e["plain_contents"]) {
        assert!(contents.contains(&c), "missing {c:?}");
    }
    let history: Vec<String> = db
        .history("fact:drink", None)
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    assert_eq!(history, strings(&e["keyed_history_newest_first"]));
    let turns: Vec<String> = db
        .thread_export_turns("offsite")
        .unwrap()
        .into_iter()
        .map(|t| t.content)
        .collect();
    assert_eq!(turns, strings(&e["thread_turns"]));
    assert_eq!(
        db.essence(EssenceScope::Thread("offsite".into()), 4096)
            .unwrap(),
        e["thread_essence"].as_str().unwrap()
    );
    assert_eq!(
        db.essence(EssenceScope::Project, 4096).unwrap(),
        e["project_essence"].as_str().unwrap()
    );
    let manifest = db
        .about()
        .unwrap()
        .into_iter()
        .next()
        .expect("the manifest");
    assert_eq!(
        manifest.metadata["embedder"]["name"],
        e["manifest_embedder"]
    );
    let hits = db
        .recall(&RecallRequest::keywords_only("Meridian").track_access(false))
        .ok();
    // Keyword recall needs v9; on v8 it's a clear error (checked below).
    if let Some(hits) = hits {
        assert!(hits.iter().any(|h| h.memory.content.contains("Meridian")));
    }
}

#[test]
fn a_v8_file_is_read_only_and_reads_everything() {
    let e = expected();
    let e = &e["passphrase_file"];
    let (_dir, path) = copy("v8_passphrase.mnemo");
    let pass = e["passphrase"].as_str().unwrap();
    let before = std::fs::read(&path).unwrap();

    match Mnemo::open(&path, pass) {
        Err(err @ MnemoError::FormatTooOld { version: 8 }) => {
            let msg = err.to_string();
            assert!(
                msg.contains("opens read-only") && msg.contains("mnemo compact"),
                "{msg}"
            );
        }
        other => panic!("expected FormatTooOld, got {:?}", other.err()),
    }

    let mut db = Mnemo::open_read_only(&path, pass).unwrap();
    assert_eq!(db.stats().unwrap().format_version, 8);
    assert_eq!(db.has_index(), e["has_index"].as_bool().unwrap());
    assert_eq!(
        db.snapshots().len() as u64,
        e["snapshots"].as_u64().unwrap()
    );
    check_contents(&mut db, e);
    assert!(matches!(
        db.recall(&RecallRequest::keywords_only("Meridian").track_access(false)),
        Err(MnemoError::Invalid(_))
    ));
    let cached = db
        .cache_get("app", "weather:lisbon")
        .unwrap()
        .expect("the cache entry");
    assert_eq!(
        cached.value,
        e["cache"]["value"].as_str().unwrap().as_bytes()
    );
    // Vector recall works (through the index).
    let q = db.memories().unwrap()[3].vector.clone();
    assert_eq!(
        db.recall(&RecallRequest::new(q).top_k(3).track_access(false))
            .unwrap()
            .len(),
        3
    );
    // Writes on the read-only handle are refused, as on any read-only one.
    assert!(matches!(
        db.remember(Memory::new("x", MemoryType::Semantic, vec![1.0; 8])),
        Err(MnemoError::ReadOnly)
    ));
    assert_eq!(
        db.verify().unwrap() as u64,
        e["memories"].as_u64().unwrap() + 1
    );
    drop(db);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "reading changed the file"
    );
}

#[test]
fn compaction_converts_it_to_v9_with_everything() {
    let e = expected();
    let e = &e["passphrase_file"];
    let (_dir, path) = copy("v8_passphrase.mnemo");
    let pass = e["passphrase"].as_str().unwrap();

    let report = Mnemo::compact_file(path.to_str().unwrap(), pass).unwrap();
    let n = e["memories"].as_u64().unwrap() as usize;
    assert_eq!((report.before, report.after), (n, n));
    assert_eq!((report.cache_before, report.cache_after), (1, 1));

    let mut db = Mnemo::open(&path, pass).unwrap();
    assert_eq!(db.stats().unwrap().format_version, 9);
    check_contents(&mut db, e);
    assert_eq!(
        db.cache_get("app", "weather:lisbon")
            .unwrap()
            .unwrap()
            .value,
        b"sunny, 21C"
    );
    // The manifest's embedder is enforced, and now recorded (EMBED-ID-1).
    struct Named(&'static str);
    impl Embedder for Named {
        fn identity(&self) -> EmbedderIdentity {
            EmbedderIdentity {
                name: self.0.into(),
                dimensions: 8,
                normalize: true,
                ..Default::default()
            }
        }
        fn embed(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![1.0; 8]).collect())
        }
    }
    assert!(matches!(
        db.set_embedder(Named("another")),
        Err(MnemoError::EmbedderMismatch { .. })
    ));
    db.set_embedder(Named("fixture-embedder-r1")).unwrap();
    // Writable again.
    db.remember(Memory::new(
        "written on the v9 copy",
        MemoryType::Semantic,
        vec![1.0; 8],
    ))
    .unwrap();
    db.flush().unwrap();
    assert_eq!(db.len(), n + 1);
}

#[test]
fn the_raw_key_fixture_opens_read_only_and_compacts() {
    let e = expected();
    let hex = std::fs::read_to_string(fixtures().join("v8_key.key")).unwrap();
    let key: [u8; 32] =
        std::array::from_fn(|i| u8::from_str_radix(&hex.trim()[2 * i..2 * i + 2], 16).unwrap());
    let (_dir, path) = copy("v8_key.mnemo");
    assert!(matches!(
        Mnemo::open_with_key(&path, &key),
        Err(MnemoError::FormatTooOld { version: 8 })
    ));
    let db = Mnemo::open_read_only_with_key(&path, &key).unwrap();
    assert_eq!(db.len() as u64, e["key_file"]["memories"].as_u64().unwrap());
    drop(db);
    Mnemo::compact_file_with_key(path.to_str().unwrap(), &key).unwrap();
    let db = Mnemo::open_with_key(&path, &key).unwrap();
    assert_eq!(db.len(), 3);
}

#[test]
fn no_new_file_can_be_v8() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = MnemoConfig {
        format_version: 8,
        dimensions: 8,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    match Mnemo::create(dir.path().join("new.mnemo"), "pw", cfg) {
        Err(MnemoError::Invalid(msg)) => assert!(msg.contains("new files are v9"), "{msg}"),
        other => panic!("expected Invalid, got {:?}", other.err()),
    }
    assert!(!dir.path().join("new.mnemo").exists());
}

/// RB-003.3: a v8 file left mid-commit (`v8_pending_wal.mnemo`: 10
/// memories checkpointed, 5 committed in the WAL but not checkpointed, as
/// a kill under v0.12 leaves it). The read-only open can't recover it and
/// no build opens it read-write, so the message points at `mnemo compact`,
/// which recovers the WAL and converts the file.
#[test]
fn a_v8_file_with_a_pending_wal_points_at_compact() {
    let (_dir, path) = copy("v8_pending_wal.mnemo");
    let pass = "v8-fixture-passphrase";
    match Mnemo::open_read_only(&path, pass) {
        Err(err @ MnemoError::NeedsWriteOpen { .. }) => {
            let msg = err.to_string();
            assert!(msg.contains("run `mnemo compact <file>`"), "{msg}");
            assert!(!msg.contains("open read-write once"), "{msg}");
        }
        other => panic!("expected NeedsWriteOpen, got {:?}", other.err()),
    }

    // The CLI's read commands give the same advice.
    let mnemo = env!("CARGO_BIN_EXE_mnemo");
    let p = path.to_str().unwrap();
    for args in [
        vec!["about", p],
        vec!["list", p],
        vec!["info", p],
        vec!["verify", p],
        vec!["export", p, "--git"],
    ] {
        let out = std::process::Command::new(mnemo)
            .args(&args)
            .env("MNEMO_PASSPHRASE", pass)
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?}");
        assert!(
            err.contains("run `mnemo compact <file>`"),
            "{args:?}: {err}"
        );
    }

    // And compaction recovers all of it.
    let out = std::process::Command::new(mnemo)
        .args(["compact", p])
        .env("MNEMO_PASSPHRASE", pass)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("(v9): 15 -> 15 live memories"), "{stdout}");
    let mut db = Mnemo::open(&path, pass).unwrap();
    assert_eq!(db.stats().unwrap().format_version, 9);
    let contents: Vec<String> = db
        .memories()
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    for i in 0..5 {
        assert!(
            contents.contains(&format!("note in the WAL {i}")),
            "{contents:?}"
        );
    }
    assert_eq!(contents.len(), 15);
}

/// RB-003.3 (v0.13.1): a read-write open of the v8 pending-WAL fixture
/// writes nothing, whether the passphrase is wrong (`WrongPassphrase`) or
/// right (`FormatTooOld`, refused before the WAL is replayed); compaction
/// still recovers it (above).
#[test]
fn a_refused_open_of_a_v8_pending_wal_leaves_it_byte_identical() {
    let (_dir, path) = copy("v8_pending_wal.mnemo");
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        Mnemo::open(&path, "not the passphrase"),
        Err(MnemoError::WrongPassphrase)
    ));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "a wrong passphrase wrote"
    );
    assert!(matches!(
        Mnemo::open(&path, "v8-fixture-passphrase"),
        Err(MnemoError::FormatTooOld { version: 8 })
    ));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "a refused v8 open wrote"
    );
}

/// RB-004.5 item 2 (v0.13.2): a v8 file with a torn header page. The
/// read-only open can't heal it and a read-write open refuses a v8 file, so
/// the error names `mnemo compact`, which does heal it; the file is left
/// byte-identical until then.
#[test]
fn a_torn_v8_header_names_compact_and_compact_heals_it() {
    let e = expected();
    let e = &e["passphrase_file"];
    let (_dir, path) = copy("v8_passphrase.mnemo");
    let pass = e["passphrase"].as_str().unwrap();
    let live = Mnemo::open_read_only(&path, pass).unwrap().len();

    let mut bytes = std::fs::read(&path).unwrap();
    bytes[..16].fill(0);
    std::fs::write(&path, &bytes).unwrap();

    let err = Mnemo::open_read_only(&path, pass)
        .err()
        .expect("a torn header");
    let msg = err.to_string();
    assert!(
        matches!(err, MnemoError::NeedsWriteOpen { .. })
            && msg.contains("pre-v9")
            && msg.contains("mnemo compact")
            && !msg.contains("open read-write once"),
        "{msg}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "the refusal wrote");

    let report = Mnemo::compact_file(path.to_str().unwrap(), pass).unwrap();
    assert_eq!(report.after, live);
    let mut db = Mnemo::open_read_only(&path, pass).unwrap();
    assert_eq!(db.len(), live);
    assert_eq!(db.stats().unwrap().format_version, 9);
}
