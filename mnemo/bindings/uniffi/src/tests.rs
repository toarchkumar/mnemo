//! MOBILE-1a: Rust-level tests of the binding layer (the Swift and Kotlin
//! suites in 1b and 1c run through the generated code).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use tempfile::tempdir;

use crate::*;

const DIMS: u32 = 4;

fn config() -> MnemoConfig {
    MnemoConfig {
        dimensions: DIMS,
        format_version: 0,
        vector_cache_budget_bytes: MOBILE_VECTOR_CACHE_BUDGET,
        cache_pages: MOBILE_CACHE_PAGES,
    }
}

fn remember_opts() -> RememberOptions {
    RememberOptions {
        memory_type: None,
        importance: None,
        metadata_json: None,
        key: None,
        ttl_secs: None,
        agent_id: None,
    }
}

fn recall_opts() -> RecallOptions {
    RecallOptions {
        top_k: 10,
        types: None,
        keywords: None,
        keyword_weight: None,
        filter: Vec::new(),
        max_bytes: None,
        track_access: false,
    }
}

/// A deterministic embedder: a text's vector is a spike on the axis its
/// first byte picks (so "alpha …" and "apple …" land together), plus a
/// small non-zero tail.
struct Mock {
    calls: AtomicUsize,
}

fn mock_vector(t: &str) -> Vec<f32> {
    let axis = t.bytes().next().unwrap_or(0) as usize % DIMS as usize;
    let mut v = vec![0.01; DIMS as usize];
    v[axis] = 1.0;
    v
}

impl MnemoEmbedder for Mock {
    fn identity(&self) -> EmbedderIdentity {
        EmbedderIdentity {
            name: "mock-4".into(),
            dimensions: DIMS,
            normalize: false,
        }
    }
    fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, EmbedderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(texts.iter().map(|t| mock_vector(t)).collect())
    }
}

fn mock() -> Arc<Mock> {
    Arc::new(Mock {
        calls: AtomicUsize::new(0),
    })
}

fn new_db(dir: &tempfile::TempDir) -> (Arc<MnemoDb>, Vec<u8>) {
    let key = generate_key();
    assert_eq!(key.len(), 32);
    let path = dir.path().join("t.mnemo").display().to_string();
    let db = MnemoDb::create_with_key(path, key.clone(), config()).unwrap();
    (db, key)
}

#[test]
fn mock_embedder_round_trip_and_identity_checks() {
    let dir = tempdir().unwrap();
    let (db, _) = new_db(&dir);
    let e = mock();
    db.set_embedder(e.clone()).unwrap();
    let a = db
        .remember_text("alpha: the user prefers tea".into(), remember_opts())
        .unwrap();
    assert_eq!(a.outcome, WriteOutcome::Added);
    db.remember_text("beta: the deploy ran".into(), remember_opts())
        .unwrap();
    let hits = db.recall_text("apple".into(), recall_opts()).unwrap().hits;
    assert_eq!(hits[0].memory.id, a.id, "same axis as 'alpha'");
    assert_eq!(e.calls.load(Ordering::SeqCst), 3);

    // A wrong width is refused, as in the engine.
    struct Wide;
    impl MnemoEmbedder for Wide {
        fn identity(&self) -> EmbedderIdentity {
            EmbedderIdentity {
                name: "wide".into(),
                dimensions: 8,
                normalize: false,
            }
        }
        fn embed(&self, _: Vec<String>) -> Result<Vec<Vec<f32>>, EmbedderError> {
            unreachable!()
        }
    }
    assert!(matches!(
        db.set_embedder(Arc::new(Wide)),
        Err(MnemoError::DimensionMismatch {
            expected: 4,
            got: 8
        })
    ));
}

#[test]
fn a_throwing_embedder_is_an_error_and_writes_nothing() {
    struct Throws;
    impl MnemoEmbedder for Throws {
        fn identity(&self) -> EmbedderIdentity {
            EmbedderIdentity {
                name: "throws".into(),
                dimensions: DIMS,
                normalize: false,
            }
        }
        fn embed(&self, _: Vec<String>) -> Result<Vec<Vec<f32>>, EmbedderError> {
            Err(EmbedderError::Failed {
                message: "model not loaded".into(),
            })
        }
    }
    let dir = tempdir().unwrap();
    let (db, _) = new_db(&dir);
    db.set_embedder(Arc::new(Throws)).unwrap();
    match db.remember_text("x".into(), remember_opts()) {
        Err(MnemoError::Embedder { message }) => assert!(message.contains("model not loaded")),
        other => panic!("expected Embedder, got {other:?}"),
    }
    let turns = vec![ThreadTurn {
        turn_id: "t1".into(),
        role: "user".into(),
        content: "hello".into(),
        timestamp: None,
    }];
    assert!(matches!(
        db.ingest_thread("chat".into(), turns),
        Err(MnemoError::Embedder { .. })
    ));
    assert!(matches!(
        db.recall_text("q".into(), recall_opts()),
        Err(MnemoError::Embedder { .. })
    ));
    assert_eq!(db.len().unwrap(), 0, "nothing was written");
    assert!(db.thread_export("chat".into()).unwrap().is_empty());
}

#[test]
fn the_embedder_may_call_back_into_the_same_handle() {
    // `embed` runs with no lock held: a callback that reads the database it
    // embeds for must not deadlock. Run on a thread with a timeout, so a
    // deadlock fails the test instead of hanging it.
    struct Reentrant {
        db: Mutex<Option<Arc<MnemoDb>>>,
        seen: AtomicUsize,
    }
    impl MnemoEmbedder for Reentrant {
        fn identity(&self) -> EmbedderIdentity {
            EmbedderIdentity {
                name: "reentrant".into(),
                dimensions: DIMS,
                normalize: false,
            }
        }
        fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, EmbedderError> {
            let db = self.db.lock().unwrap().clone().unwrap();
            let n = db.len().map_err(|e| EmbedderError::Failed {
                message: e.to_string(),
            })?;
            let _ = db.stats().unwrap();
            self.seen.store(n as usize, Ordering::SeqCst);
            Ok(texts.iter().map(|t| mock_vector(t)).collect())
        }
    }
    let dir = tempdir().unwrap();
    let (db, _) = new_db(&dir);
    let e = Arc::new(Reentrant {
        db: Mutex::new(None),
        seen: AtomicUsize::new(usize::MAX),
    });
    *e.db.lock().unwrap() = Some(db.clone());
    db.set_embedder(e.clone()).unwrap();
    db.remember("seed".into(), vec![1.0, 0.0, 0.0, 0.0], remember_opts())
        .unwrap();

    let (tx, rx) = mpsc::channel();
    let worker = db.clone();
    std::thread::spawn(move || {
        let r1 = worker
            .remember_text("alpha".into(), remember_opts())
            .map(|_| ());
        let r2 = worker
            .recall_text("alpha".into(), recall_opts())
            .map(|_| ());
        let turns = vec![ThreadTurn {
            turn_id: "t1".into(),
            role: "user".into(),
            content: "hello there".into(),
            timestamp: None,
        }];
        let r3 = worker.ingest_thread("chat".into(), turns).map(|_| ());
        let _ = tx.send((r1, r2, r3));
    });
    let (r1, r2, r3) = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("deadlock: embed ran with the handle lock held");
    r1.unwrap();
    r2.unwrap();
    r3.unwrap();
    assert_eq!(e.seen.load(Ordering::SeqCst), 2, "the callback read the db");
    // Break the cycle (db -> embedder -> db).
    *e.db.lock().unwrap() = None;
}

#[test]
fn export_with_passphrase_opens_with_the_passphrase_not_the_key() {
    let dir = tempdir().unwrap();
    let (db, key) = new_db(&dir);
    let w = db
        .remember("kept".into(), vec![1.0, 0.0, 0.0, 0.0], remember_opts())
        .unwrap();
    let dest = dir.path().join("export.mnemo").display().to_string();
    db.export_with_passphrase(dest.clone(), "move-pw".into())
        .unwrap();
    // The live file still opens with its key (after close), unchanged.
    db.close().unwrap();
    let live = MnemoDb::open_with_key(
        dir.path().join("t.mnemo").display().to_string(),
        key.clone(),
    )
    .unwrap();
    assert_eq!(live.get(w.id.clone()).unwrap().content, "kept");
    live.close().unwrap();

    let exported = MnemoDb::open(dest.clone(), "move-pw".into()).unwrap();
    assert_eq!(exported.get(w.id.clone()).unwrap().content, "kept");
    exported.close().unwrap();
    assert!(matches!(
        MnemoDb::open_with_key(dest.clone(), key.clone()),
        Err(MnemoError::WrongPassphrase)
    ));
    // A fresh data key: the live file's header over the export doesn't
    // open it with the device key either.
    let live_path = dir.path().join("t.mnemo");
    let spliced = splice(&dest, &live_path, &dir, "export-with-live-header");
    assert!(MnemoDb::open_read_only_with_key(spliced, key).is_err());
    // A second export to the same path is refused.
    let again = MnemoDb::open(dest.clone(), "move-pw".into()).unwrap();
    assert!(matches!(
        again.export_with_passphrase(dest, "x".into()),
        Err(MnemoError::Invalid { .. })
    ));
}

#[test]
fn import_with_passphrase_opens_with_the_new_key_not_the_passphrase() {
    let dir = tempdir().unwrap();
    let src = dir
        .path()
        .join("from-old-phone.mnemo")
        .display()
        .to_string();
    let db = MnemoDb::create(src.clone(), "move-pw".into(), config()).unwrap();
    let w = db
        .remember("kept".into(), vec![0.0, 1.0, 0.0, 0.0], remember_opts())
        .unwrap();
    db.close().unwrap();

    let dest = dir.path().join("t.mnemo").display().to_string();
    let key = import_with_passphrase(src.clone(), "move-pw".into(), dest.clone()).unwrap();
    assert_eq!(key.len(), 32);
    let imported = MnemoDb::open_with_key(dest.clone(), key).unwrap();
    assert_eq!(imported.get(w.id.clone()).unwrap().content, "kept");
    imported.close().unwrap();
    assert!(matches!(
        MnemoDb::open(dest.clone(), "move-pw".into()),
        Err(MnemoError::WrongPassphrase)
    ));
    // A fresh data key: the transfer file's header over the import doesn't
    // open it with the transfer passphrase.
    let spliced = splice(
        &dest,
        std::path::Path::new(&src),
        &dir,
        "import-with-source-header",
    );
    assert!(MnemoDb::open_read_only(spliced, "move-pw".into()).is_err());
    // The source is untouched and still opens with the passphrase.
    MnemoDb::open(src.clone(), "move-pw".into())
        .unwrap()
        .close()
        .unwrap();
    // A wrong passphrase leaves no destination behind.
    let dest2 = dir.path().join("t2.mnemo").display().to_string();
    assert!(matches!(
        import_with_passphrase(src, "nope".into(), dest2.clone()),
        Err(MnemoError::WrongPassphrase)
    ));
    assert!(!std::path::Path::new(&dest2).exists());
}

#[test]
fn a_poisoned_handle_reports_corrupt_instead_of_panicking() {
    let dir = tempdir().unwrap();
    let (db, _) = new_db(&dir);
    db.remember("x".into(), vec![1.0, 0.0, 0.0, 0.0], remember_opts())
        .unwrap();
    let inner = db.clone();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _guard = inner.inner.lock().unwrap();
        panic!("a panic while the handle is in use");
    }));
    assert!(r.is_err());
    for res in [db.len().map(|_| ()), db.flush(), db.close()] {
        match res {
            Err(MnemoError::Corrupt { message }) => assert!(message.contains("poisoned")),
            other => panic!("expected Corrupt, got {other:?}"),
        }
    }
}

#[test]
fn close_then_closed_and_the_file_reopens() {
    let dir = tempdir().unwrap();
    let (db, key) = new_db(&dir);
    db.remember("x".into(), vec![1.0, 0.0, 0.0, 0.0], remember_opts())
        .unwrap();
    db.close().unwrap();
    db.close().unwrap();
    assert!(matches!(db.len(), Err(MnemoError::Closed)));
    assert!(matches!(db.flush(), Err(MnemoError::Closed)));
    let again = MnemoDb::open_with_key(db.path(), key.clone()).unwrap();
    assert_eq!(again.len().unwrap(), 1, "close flushed");
    // Read-only handles refuse writes.
    again.close().unwrap();
    let ro = MnemoDb::open_read_only_with_key(db.path(), key).unwrap();
    assert!(matches!(
        ro.remember("y".into(), vec![1.0, 0.0, 0.0, 0.0], remember_opts()),
        Err(MnemoError::ReadOnly)
    ));
    // A short key is refused before touching the file.
    assert!(matches!(
        MnemoDb::open_with_key(db.path(), vec![1, 2, 3]),
        Err(MnemoError::Invalid { .. })
    ));
}

#[test]
fn threads_keyed_writes_filters_keywords_cache_and_compaction() {
    let dir = tempdir().unwrap();
    let (db, key) = new_db(&dir);
    db.set_embedder(mock()).unwrap();

    // Threads: ingest (idempotent), export, forget.
    let turns: Vec<ThreadTurn> = (0..3)
        .map(|i| ThreadTurn {
            turn_id: format!("t{i}"),
            role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
            content: format!("turn {i} text"),
            timestamp: Some(1_700_000_000 + i),
        })
        .collect();
    let r = db.ingest_thread("chat".into(), turns.clone()).unwrap();
    assert_eq!(r.added_records, 3);
    let again = db.ingest_thread("chat".into(), turns.clone()).unwrap();
    assert_eq!(again.added_records, 0);
    assert_eq!(again.skipped_existing_turns, 3);
    let exported = db.thread_export("chat".into()).unwrap();
    assert_eq!(
        exported
            .iter()
            .map(|t| t.content.clone())
            .collect::<Vec<_>>(),
        turns.iter().map(|t| t.content.clone()).collect::<Vec<_>>()
    );

    // Keyed writes and history.
    let mut o = remember_opts();
    o.key = Some("drink".into());
    o.metadata_json = Some(r#"{"topic":"prefs"}"#.into());
    let first = db
        .remember("tea".into(), vec![0.0, 0.0, 1.0, 0.0], o.clone())
        .unwrap();
    assert_eq!(first.outcome, WriteOutcome::Added);
    let same = db
        .remember("tea".into(), vec![0.0, 0.0, 1.0, 0.0], o.clone())
        .unwrap();
    assert_eq!(same.outcome, WriteOutcome::Unchanged);
    let second = db
        .remember("coffee".into(), vec![0.0, 0.0, 1.0, 0.1], o.clone())
        .unwrap();
    assert_eq!(second.outcome, WriteOutcome::Superseded);
    let hist = db.history("drink".into()).unwrap();
    assert_eq!(hist.len(), 2);
    assert!(hist.iter().any(|m| m.content == "tea"));

    // Filters, keyword-only recall, update and delete.
    let mut ro = recall_opts();
    ro.filter = vec![FilterTerm::Eq {
        key: "topic".into(),
        value_json: r#""prefs""#.into(),
    }];
    let hits = db.recall(vec![0.0, 0.0, 1.0, 0.0], ro).unwrap().hits;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].memory.content, "coffee");
    let bp = db
        .remember(
            "shipped BP.1 parser".into(),
            vec![0.0, 1.0, 0.0, 0.0],
            remember_opts(),
        )
        .unwrap();
    let mut kw = recall_opts();
    kw.keywords = Some("BP.1".into());
    assert_eq!(db.recall(Vec::new(), kw).unwrap().hits[0].memory.id, bp.id);
    let patch = MemoryPatch {
        content: Some("shipped BP.1 parser (fixed)".into()),
        vector: None,
        importance: Some(0.9),
        memory_type: None,
        metadata_json: None,
        ttl_secs: None,
        clear_ttl: false,
    };
    assert_eq!(
        db.update(bp.id.clone(), patch).unwrap(),
        WriteOutcome::Updated
    );
    assert_eq!(db.get(bp.id.clone()).unwrap().importance, 0.9);
    assert!(matches!(
        db.update(
            "not-an-id".into(),
            MemoryPatch {
                content: None,
                vector: None,
                importance: None,
                memory_type: None,
                metadata_json: None,
                ttl_secs: None,
                clear_ttl: false
            }
        ),
        Err(MnemoError::Invalid { .. })
    ));
    assert!(matches!(
        db.remember(
            "bad".into(),
            vec![1.0, 0.0, 0.0, 0.0],
            RememberOptions {
                metadata_json: Some("[1]".into()),
                ..remember_opts()
            }
        ),
        Err(MnemoError::Invalid { .. })
    ));

    // The result cache.
    db.cache_put(
        "tools".into(),
        "weather:paris".into(),
        b"sunny".to_vec(),
        Some(60),
    )
    .unwrap();
    assert_eq!(
        db.cache_get("tools".into(), "weather:paris".into())
            .unwrap(),
        Some(b"sunny".to_vec())
    );
    assert_eq!(db.cache_get("tools".into(), "nope".into()).unwrap(), None);

    // Stats, the forget, the essence render, then compaction.
    assert_eq!(db.stats().unwrap().format_version, 9);
    assert_eq!(db.thread_forget("chat".into()).unwrap(), 3);
    let _ = db.essence(EssenceScope::Project, 4096).unwrap();
    db.delete(bp.id.clone()).unwrap();
    assert!(matches!(db.get(bp.id), Err(MnemoError::NotFound { .. })));
    db.close().unwrap();
    let report = compact_file_with_key(db.path(), key.clone()).unwrap();
    assert_eq!(report.after, report.before);
    let reopened = MnemoDb::open_with_key(db.path(), key).unwrap();
    assert_eq!(reopened.history("drink".into()).unwrap().len(), 2);
}

#[test]
fn essence_writes_round_trip_and_embed_with_no_lock_held() {
    // An embedder that reads the handle it embeds for: it deadlocks if an
    // essence write embeds with the lock held. Run with a timeout.
    struct Reentrant {
        db: Mutex<Option<Arc<MnemoDb>>>,
        calls: AtomicUsize,
    }
    impl MnemoEmbedder for Reentrant {
        fn identity(&self) -> EmbedderIdentity {
            EmbedderIdentity {
                name: "reentrant".into(),
                dimensions: DIMS,
                normalize: false,
            }
        }
        fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, EmbedderError> {
            let db = self.db.lock().unwrap().clone().unwrap();
            db.len().map_err(|e| EmbedderError::Failed {
                message: e.to_string(),
            })?;
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(texts.iter().map(|t| mock_vector(t)).collect())
        }
    }
    let dir = tempdir().unwrap();
    let (db, _) = new_db(&dir);
    let e = Arc::new(Reentrant {
        db: Mutex::new(Some(db.clone())),
        calls: AtomicUsize::new(0),
    });
    db.set_embedder(e.clone()).unwrap();
    let thread = || EssenceScope::Thread {
        thread_id: "chat".into(),
    };

    let (tx, rx) = mpsc::channel();
    let worker = db.clone();
    std::thread::spawn(move || {
        let scope = || EssenceScope::Thread {
            thread_id: "chat".into(),
        };
        let decision = worker.essence_add(
            scope(),
            EssenceSection::Decisions,
            "drink tea at standups".into(),
            None,
        );
        let timeline = worker.essence_append_timeline(scope(), "shipped the demo".into(), None);
        let _ = tx.send((decision, timeline));
    });
    let (decision, timeline) = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("deadlock: an essence write embedded with the handle lock held");
    let decision = decision.unwrap();
    timeline.unwrap();
    assert_eq!(
        e.calls.load(Ordering::SeqCst),
        2,
        "each write embedded once"
    );

    let render = db.essence(thread(), 4096).unwrap();
    assert!(render.contains("drink tea at standups"), "{render}");
    assert!(render.contains("shipped the demo"), "{render}");

    // Update in place, then the same text again is a no-op.
    let outcome = db
        .essence_update(decision.clone(), "drink coffee at standups".into(), None)
        .unwrap();
    assert!(matches!(outcome, WriteOutcome::Updated), "{outcome:?}");
    let again = db
        .essence_update(decision.clone(), "drink coffee at standups".into(), None)
        .unwrap();
    assert!(matches!(again, WriteOutcome::Unchanged), "{again:?}");
    let render = db.essence(thread(), 4096).unwrap();
    assert!(
        render.contains("drink coffee") && !render.contains("drink tea"),
        "{render}"
    );

    // Resolve, and remove another item from the render.
    db.essence_resolve(decision).unwrap();
    let open = db
        .essence_add(thread(), EssenceSection::Open, "which milk?".into(), None)
        .unwrap();
    assert!(db.essence(thread(), 4096).unwrap().contains("which milk?"));
    db.essence_remove(open).unwrap();
    assert!(!db.essence(thread(), 4096).unwrap().contains("which milk?"));

    // Errors: a bad id, an over-long item, editing the timeline.
    assert!(matches!(
        db.essence_resolve("not-an-id".into()),
        Err(MnemoError::Invalid { .. })
    ));
    assert!(matches!(
        db.essence_add(thread(), EssenceSection::Next, "x".repeat(281), None),
        Err(MnemoError::Invalid { .. })
    ));
    let entry = db
        .essence_append_timeline(thread(), "milestone".into(), None)
        .unwrap();
    assert!(db.essence_update(entry, "rewritten".into(), None).is_err());
    *e.db.lock().unwrap() = None;
}

#[test]
fn essence_writes_work_without_an_embedder() {
    let dir = tempdir().unwrap();
    let (db, _) = new_db(&dir);
    db.essence_add(
        EssenceScope::Project,
        EssenceSection::Conventions,
        "tabs, not spaces".into(),
        Some(vec!["style-guide".into()]),
    )
    .unwrap();
    db.essence_append_timeline(EssenceScope::Project, "first release".into(), None)
        .unwrap();
    let render = db.essence(EssenceScope::Project, 4096).unwrap();
    assert!(
        render.contains("tabs, not spaces") && render.contains("first release"),
        "{render}"
    );
}

fn fill(db: &MnemoDb, n: u32) {
    for i in 0..n {
        let x = i as f32 + 1.0;
        db.remember(
            format!("memory {i}"),
            vec![x.sin() + 1.5, x.cos(), (x * 0.37).sin(), 1.0],
            remember_opts(),
        )
        .unwrap();
    }
    db.flush().unwrap();
}

#[test]
fn build_index_and_has_index() {
    // v9 (the default) below V9_ANN_MIN_FACTS: an exact scan is faster, so
    // build_index builds nothing and says so.
    let dir = tempdir().unwrap();
    let (db, _) = new_db(&dir);
    assert!(!db.has_index().unwrap());
    fill(&db, 600);
    let info = db.build_index().unwrap();
    assert_eq!((info.vectors, info.partitions), (600, 0), "{info:?}");
    assert!(!db.has_index().unwrap());
    db.close().unwrap();

    // v8 builds an IVF+PQ index at any size, and recall uses it.
    let dir = tempdir().unwrap();
    let path = dir.path().join("v8.mnemo").display().to_string();
    let db = MnemoDb::create_with_key(
        path,
        generate_key(),
        MnemoConfig {
            format_version: 8,
            ..config()
        },
    )
    .unwrap();
    fill(&db, 600);
    let info = db.build_index().unwrap();
    assert_eq!(info.vectors, 600);
    assert!(info.partitions > 0 && info.subspaces > 0, "{info:?}");
    assert!(db.has_index().unwrap());
    assert!(db.stats().unwrap().has_index);
    let hits = db
        .recall(vec![2.0, 0.5, 0.1, 1.0], recall_opts())
        .unwrap()
        .hits;
    assert_eq!(hits.len(), 10);
    db.close().unwrap();
}

fn turn(id: &str, content: &str) -> ThreadTurn {
    ThreadTurn {
        turn_id: id.into(),
        role: "user".into(),
        content: content.into(),
        timestamp: None,
    }
}

#[test]
fn ingest_in_two_halves_plans_applies_and_refuses_a_stale_plan() {
    let dir = tempdir().unwrap();
    let (db, _) = new_db(&dir);
    let turns = vec![turn("t1", "alpha one"), turn("t2", "beta two")];
    let texts = db.ingest_thread_plan("chat".into(), turns.clone()).unwrap();
    assert_eq!(texts.len(), 2);
    let vectors: Vec<Vec<f32>> = texts.iter().map(|t| mock_vector(t)).collect();

    // Wrong shapes are refused before anything is written.
    assert!(matches!(
        db.ingest_thread_with_vectors(
            "chat".into(),
            turns.clone(),
            texts.clone(),
            vectors[..1].to_vec()
        ),
        Err(MnemoError::Invalid { .. })
    ));
    assert!(matches!(
        db.ingest_thread_with_vectors(
            "chat".into(),
            turns.clone(),
            texts.clone(),
            vec![vec![1.0; 3]; 2]
        ),
        Err(MnemoError::DimensionMismatch {
            expected: 4,
            got: 3
        })
    ));

    // The thread changed since the plan: None, nothing written.
    db.ingest_thread_with_vectors(
        "chat".into(),
        vec![turns[0].clone()],
        vec![texts[0].clone()],
        vec![vectors[0].clone()],
    )
    .unwrap()
    .expect("a fresh plan applies");
    assert!(db
        .ingest_thread_with_vectors("chat".into(), turns.clone(), texts.clone(), vectors.clone())
        .unwrap()
        .is_none());
    assert_eq!(db.thread_export("chat".into()).unwrap().len(), 1);

    // Plan again: only t2 is new.
    let texts = db.ingest_thread_plan("chat".into(), turns.clone()).unwrap();
    assert_eq!(texts.len(), 1);
    let vectors = texts.iter().map(|t| mock_vector(t)).collect();
    let report = db
        .ingest_thread_with_vectors("chat".into(), turns.clone(), texts, vectors)
        .unwrap()
        .unwrap();
    assert_eq!(report.skipped_existing_turns, 1);
    assert_eq!(db.thread_export("chat".into()).unwrap().len(), 2);
    assert!(db
        .ingest_thread_plan("chat".into(), turns)
        .unwrap()
        .is_empty());
}

#[test]
fn embed_texts_checks_like_the_text_calls() {
    let m = mock();
    let v = embed_texts(m.clone(), vec!["alpha".into(), "beta".into()], DIMS).unwrap();
    assert_eq!(v, vec![mock_vector("alpha"), mock_vector("beta")]);
    assert!(matches!(
        embed_texts(m.clone(), vec!["alpha".into()], 8),
        Err(MnemoError::DimensionMismatch {
            expected: 8,
            got: 4
        })
    ));
    struct Short;
    impl MnemoEmbedder for Short {
        fn identity(&self) -> EmbedderIdentity {
            EmbedderIdentity {
                name: "short".into(),
                dimensions: DIMS,
                normalize: false,
            }
        }
        fn embed(&self, _: Vec<String>) -> Result<Vec<Vec<f32>>, EmbedderError> {
            Err(EmbedderError::Failed {
                message: "model not loaded".into(),
            })
        }
    }
    match embed_texts(Arc::new(Short), vec!["x".into()], DIMS) {
        Err(MnemoError::Embedder { message }) => assert!(message.contains("model not loaded")),
        other => panic!("expected Embedder, got {other:?}"),
    }
}

#[test]
fn essence_writes_with_the_callers_vector_never_call_the_embedder() {
    let dir = tempdir().unwrap();
    let (db, _) = new_db(&dir);
    let m = mock();
    db.set_embedder(m.clone()).unwrap();
    let before = m.calls.load(Ordering::SeqCst);
    let id = db
        .essence_add_with_vector(
            EssenceScope::Project,
            EssenceSection::Decisions,
            "keep it small".into(),
            None,
            Some(mock_vector("keep")),
        )
        .unwrap();
    db.essence_append_timeline_with_vector(
        EssenceScope::Project,
        "shipped".into(),
        None,
        Some(mock_vector("s")),
    )
    .unwrap();
    assert_eq!(m.calls.load(Ordering::SeqCst), before, "no embed call");
    let text = db.essence(EssenceScope::Project, 4096).unwrap();
    assert!(
        text.contains("keep it small") && text.contains("shipped"),
        "{text}"
    );
    assert!(db.get(id).is_ok());
    assert!(matches!(
        db.essence_add_with_vector(
            EssenceScope::Project,
            EssenceSection::Decisions,
            "x".into(),
            None,
            Some(vec![1.0; 3])
        ),
        Err(MnemoError::DimensionMismatch { .. })
    ));
}

/// A copy of `file` with `header_from`'s page 0 (the header: salt, KDF costs,
/// wrapped data key) written over its own.
fn splice(
    file: &str,
    header_from: &std::path::Path,
    dir: &tempfile::TempDir,
    name: &str,
) -> String {
    let mut bytes = std::fs::read(file).unwrap();
    let other = std::fs::read(header_from).unwrap();
    bytes[..8192].copy_from_slice(&other[..8192]);
    let out = dir.path().join(format!("{name}.mnemo"));
    std::fs::write(&out, bytes).unwrap();
    out.display().to_string()
}
