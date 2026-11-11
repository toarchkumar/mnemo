//! FMT-9d.1: the v9 BM25 keyword index. Every test asks for its format
//! explicitly.

use mnemo_engine::{
    KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, MnemoError, RecallRequest, ThreadTurn, Ulid,
};
use tempfile::tempdir;

fn cfg(version: u16) -> MnemoConfig {
    MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        format_version: version,
        ..Default::default()
    }
}

fn mem(text: &str, i: usize) -> Memory {
    Memory::new(
        text,
        MemoryType::Semantic,
        vec![1.0 + i as f32, 0.5, 0.25, 0.125],
    )
}

fn top(db: &mut Mnemo, q: &str) -> Vec<Ulid> {
    db.__bm25(q)
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

#[test]
fn an_exact_identifier_beats_its_parts() {
    let dir = tempdir().unwrap();
    let mut db = Mnemo::create(dir.path().join("t.mnemo"), "pw", cfg(9)).unwrap();
    let exact = db
        .remember(mem("Shipped BP.1 with the new parser", 0))
        .unwrap();
    let parts = db
        .remember(mem("BP review: item 1 of the plan", 1))
        .unwrap();
    let other = db.remember(mem("the user prefers tea", 2)).unwrap();
    // Before any flush the documents are pending, and still searchable.
    let hits = top(&mut db, "BP.1");
    assert_eq!(hits[0], exact);
    assert!(hits.contains(&parts));
    assert!(!hits.contains(&other));
    db.flush().unwrap();
    assert_eq!(top(&mut db, "BP.1")[0], exact);
    assert_eq!(top(&mut db, "tea"), vec![other]);
    assert!(top(&mut db, "nothing-like-this").is_empty());
}

#[test]
fn the_index_survives_reopen_and_follows_deletes_and_rewrites() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    let a = db
        .remember(mem("release v0.9.0 shipped on friday", 0))
        .unwrap();
    let b = db.remember(mem("release v0.10.0 is planned", 1)).unwrap();
    db.close().unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(top(&mut db, "v0.9.0"), vec![a]);
    db.delete(&a).unwrap();
    assert!(
        top(&mut db, "v0.9.0").is_empty(),
        "a deleted doc is gone at once"
    );
    // An overwrite indexes the new text and drops the old.
    let mut m = db.get(&b).unwrap();
    m.content = "release v0.11.0 is planned".into();
    db.remember(m).unwrap();
    assert!(top(&mut db, "v0.10.0").is_empty());
    assert_eq!(top(&mut db, "v0.11.0"), vec![b]);
    db.close().unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(top(&mut db, "v0.11.0"), vec![b]);
    assert!(top(&mut db, "v0.9.0").is_empty());
    assert_eq!(db.verify().unwrap(), 1);
}

#[test]
fn segments_merge_and_nothing_is_lost() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    let ids: Vec<Ulid> = (0..30)
        .map(|i| {
            let id = db
                .remember(mem(&format!("note number{i} about topic{}", i % 3), i))
                .unwrap();
            db.flush().unwrap();
            id
        })
        .collect();
    assert!(
        db.__bm25_segments() <= 8,
        "{} segments",
        db.__bm25_segments()
    );
    db.close().unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(top(&mut db, &format!("number{i}")), vec![*id]);
    }
    assert_eq!(top(&mut db, "topic1").len(), 10);
}

#[test]
fn restore_brings_back_the_keyword_index_of_its_snapshot() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    // Keep the history explicitly: under the default policy, 20 commits of
    // churn on a tiny file reach the 1 MiB pin cap.
    db.set_max_snapshots(64);
    let kept = db.remember(mem("alpha decision recorded", 0)).unwrap();
    db.flush().unwrap();
    let mark = db.snapshots().last().unwrap().txn_id;
    for i in 0..20 {
        db.remember(mem(&format!("beta change {i}"), i + 1))
            .unwrap();
        db.flush().unwrap();
    }
    db.delete(&kept).unwrap();
    db.flush().unwrap();
    assert!(top(&mut db, "alpha").is_empty());
    db.restore_to(mark).unwrap();
    assert_eq!(top(&mut db, "alpha"), vec![kept]);
    assert!(top(&mut db, "beta").is_empty());
    db.close().unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(top(&mut db, "alpha"), vec![kept]);
    assert_eq!(db.verify().unwrap(), 1);
}

#[test]
fn thread_turns_are_not_keyword_indexed_and_v8_has_no_index() {
    let dir = tempdir().unwrap();
    let mut db = Mnemo::create(dir.path().join("t.mnemo"), "pw", cfg(9)).unwrap();
    // A real thread turn (since v0.12.2 a plain `remember` can't forge one).
    db.ingest_thread_with_vectors(
        "t1",
        &[ThreadTurn {
            turn_id: "1".into(),
            role: "user".into(),
            content: "gamma said in a thread".into(),
            timestamp: None,
        }],
        &[vec![1.0, 0.5, 0.25, 0.125]],
    )
    .unwrap();
    let fact = db.remember(mem("gamma is a fact", 1)).unwrap();
    assert_eq!(top(&mut db, "gamma"), vec![fact]);

    let mut v8 = Mnemo::create(dir.path().join("v8.mnemo"), "pw", cfg(8)).unwrap();
    v8.remember(mem("gamma on v8", 0)).unwrap();
    assert!(top(&mut v8, "gamma").is_empty());
}

// --- FMT-9d.2: keywords and hybrid recall --------------------------------

#[test]
fn keyword_only_recall_needs_no_vector() {
    let dir = tempdir().unwrap();
    let mut db = Mnemo::create(dir.path().join("t.mnemo"), "pw", cfg(9)).unwrap();
    let exact = db
        .remember(mem("Shipped BP.1 with the new parser", 0))
        .unwrap();
    db.remember(mem("BP review: item 1 of the plan", 1))
        .unwrap();
    db.remember(mem("the user prefers tea", 2)).unwrap();
    db.flush().unwrap();
    let hits = db
        .recall(
            &RecallRequest::keywords_only("BP.1")
                .top_k(5)
                .track_access(false),
        )
        .unwrap();
    assert_eq!(hits[0].memory.id, exact);
    assert_eq!(hits.len(), 2, "only keyword matches come back");
    assert!(hits[0].score > hits[1].score);
}

#[test]
fn hybrid_recall_lifts_an_exact_keyword_match() {
    let dir = tempdir().unwrap();
    let mut db = Mnemo::create(dir.path().join("t.mnemo"), "pw", cfg(9)).unwrap();
    // The query vector is closest to "tea"; the keyword says "BP.1".
    let tea = db
        .remember(Memory::new(
            "the user prefers tea",
            MemoryType::Semantic,
            vec![0.0, 0.0, 1.0, 0.0],
        ))
        .unwrap();
    let bp = db
        .remember(Memory::new(
            "shipped BP.1 with the new parser",
            MemoryType::Semantic,
            vec![0.6, 0.0, 0.8, 0.0],
        ))
        .unwrap();
    db.flush().unwrap();
    let q = vec![0.0, 0.0, 1.0, 0.05];
    let vec_only = db
        .recall(&RecallRequest::new(q.clone()).top_k(2).track_access(false))
        .unwrap();
    assert_eq!(vec_only[0].memory.id, tea);
    let hybrid = db
        .recall(
            &RecallRequest::new(q)
                .keywords("BP.1")
                .top_k(2)
                .track_access(false),
        )
        .unwrap();
    assert_eq!(hybrid[0].memory.id, bp, "the keyword match wins");
    assert!(hybrid[0].similarity < 1.0, "similarity stays the vector's");
}

#[test]
fn keyword_recall_on_v8_is_a_clear_error() {
    let dir = tempdir().unwrap();
    let mut db = Mnemo::create(dir.path().join("t.mnemo"), "pw", cfg(8)).unwrap();
    db.remember(mem("gamma", 0)).unwrap();
    match db.recall(&RecallRequest::keywords_only("gamma")) {
        Err(MnemoError::Invalid(msg)) => assert!(msg.contains("v9"), "{msg}"),
        other => panic!("expected Invalid, got {:?}", other.map(|h| h.len())),
    }
}

#[test]
fn cli_recall_text_without_an_embedder_is_keyword_recall_on_v9() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
        db.remember(mem("Shipped BP.1 with the new parser", 0))
            .unwrap();
        db.remember(mem("the user prefers tea", 1)).unwrap();
        db.close().unwrap();
    }
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mnemo"))
        .args([
            "recall",
            path.to_str().unwrap(),
            "--text",
            "BP.1",
            "--format",
            "json",
        ])
        .env("MNEMO_PASSPHRASE", "pw")
        .output()
        .expect("spawn mnemo recall");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("Shipped BP.1"), "{stdout}");
    assert!(!stdout.contains("tea"), "{stdout}");
}

#[test]
fn cli_keywords_flag_is_keyword_recall_and_text_is_not_mixed_in() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
        db.remember(mem("Shipped BP.1 with the new parser", 0))
            .unwrap();
        db.remember(mem("the user prefers tea", 1)).unwrap();
        db.close().unwrap();
    }
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mnemo"))
        .args([
            "recall",
            path.to_str().unwrap(),
            "--keywords",
            "BP.1",
            "--format",
            "json",
        ])
        .env("MNEMO_PASSPHRASE", "pw")
        .output()
        .expect("spawn mnemo recall");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("Shipped BP.1"), "{stdout}");
    assert!(!stdout.contains("tea"), "{stdout}");
}

#[test]
fn an_unknown_index_layout_is_treated_as_no_index() {
    // The dev override lets a small file build an index; no other test in
    // this binary builds one.
    std::env::set_var("MNEMO_DEV_ANN_MIN_FACTS", "0");
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let ids: Vec<Ulid> = {
        let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
        let ids = (0..300)
            .map(|i| db.remember(mem(&format!("note {i}"), i)).unwrap())
            .collect();
        db.__set_index_layout_for_testing(99);
        assert!(db.build_index().unwrap().partitions > 0);
        db.flush().unwrap();
        // FMT-9f: a few commits write index deltas on top of that base.
        for i in 0..3 {
            db.remember(mem(&format!("late {i}"), 400 + i)).unwrap();
            db.flush().unwrap();
        }
        assert_eq!(db.__index_shape().0, 3, "three delta runs on the chain");
        db.close().unwrap();
        ids
    };
    let mut db = Mnemo::open(&path, "pw").unwrap();
    let hits = db
        .recall(
            &RecallRequest::new(mem("", 42).vector)
                .metric(mnemo_engine::Metric::L2)
                .top_k(1)
                .track_access(false),
        )
        .unwrap();
    assert_eq!(hits[0].memory.id, ids[42], "exact recall still works");
    assert!(
        !db.has_index(),
        "a layout this build doesn't know is no index"
    );
    // FMT-9f (the 9d leak): the unknown base's delta runs are still
    // tracked, so the rebuild below frees them with the base.
    assert_eq!(db.__index_shape().0, 3, "the chain is tracked, not leaked");
    // Rebuilding writes a current one.
    db.build_index().unwrap();
    db.flush().unwrap();
    assert_eq!(db.__index_shape().0, 0, "the old chain was released");
    db.close().unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    db.recall(
        &RecallRequest::new(mem("", 7).vector)
            .top_k(1)
            .track_access(false),
    )
    .unwrap();
    assert!(db.has_index());
}
