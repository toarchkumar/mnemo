//! FMT-9b.1: the v9 catalog row (tags and scoring inputs) and the fp16
//! vector column. Every test asks for v9 explicitly.

use mnemo_engine::{
    Embedder, EmbedderIdentity, Filter, KdfParams, Memory, MemoryType, Metric, Mnemo, MnemoConfig,
    MnemoError, RecallRequest, Scope, ThreadTurn, Ulid,
};
use serde_json::json;
use tempfile::tempdir;

const DIMS: usize = 16;

fn cfg(version: u16) -> MnemoConfig {
    MnemoConfig {
        dimensions: DIMS,
        kdf: KdfParams::fast(),
        format_version: version,
        ..Default::default()
    }
}

/// A deterministic, well-spread vector for memory `i`.
fn vector(i: usize) -> Vec<f32> {
    (0..DIMS)
        .map(|k| {
            let x = ((i * 31 + k * 17) % 97) as f32 / 97.0 - 0.5;
            x + if k == i % DIMS { 1.0 } else { 0.0 }
        })
        .collect()
}

fn mem(i: usize) -> Memory {
    Memory::new(format!("memory {i}"), MemoryType::Semantic, vector(i))
}

fn ids_of(hits: &[mnemo_engine::RecallResult]) -> Vec<Ulid> {
    hits.iter().map(|h| h.memory.id).collect()
}

#[test]
fn v9_vectors_are_fp16_rounded_unless_float_vectors_are_kept() {
    let dir = tempdir().unwrap();
    for keep_float in [false, true] {
        let path = dir.path().join(format!("f{keep_float}.mnemo"));
        let mut db = Mnemo::create(
            &path,
            "pw",
            MnemoConfig {
                store_float_vectors: keep_float,
                ..cfg(9)
            },
        )
        .unwrap();
        let v: Vec<f32> = (0..DIMS).map(|k| 0.1 + k as f32 * 0.0123457).collect();
        let id = db
            .remember(Memory::new("x", MemoryType::Semantic, v.clone()))
            .unwrap();
        db.close().unwrap();
        let mut db = Mnemo::open(&path, "pw").unwrap();
        let got = db.get(&id).unwrap().vector;
        assert_eq!(got.len(), DIMS);
        for (a, b) in v.iter().zip(&got) {
            if keep_float {
                assert_eq!(a, b, "float vectors are kept exactly");
            } else {
                assert!((a - b).abs() <= a.abs() * 1e-3, "{a} vs {b}");
            }
        }
        if !keep_float {
            assert!(
                v.iter().zip(&got).any(|(a, b)| a != b),
                "fp16 rounding is visible"
            );
        }
        assert_eq!(db.verify().unwrap(), 1);
    }
}

#[test]
fn v9_recall_matches_v8_ranking() {
    let dir = tempdir().unwrap();
    let mut dbs: Vec<Mnemo> = [8u16, 9]
        .iter()
        .map(|&v| {
            let mut db =
                Mnemo::create(dir.path().join(format!("v{v}.mnemo")), "pw", cfg(v)).unwrap();
            for i in 0..200 {
                let mut m = mem(i);
                m.importance = (i % 5) as f32 / 4.0;
                m.agent_id = if i % 3 == 0 { "a".into() } else { "b".into() };
                if i % 7 == 0 {
                    m.memory_type = MemoryType::Episodic;
                }
                db.remember(m).unwrap();
            }
            db.flush().unwrap();
            db
        })
        .collect();
    for q in [3usize, 50, 111, 199] {
        for metric in [Metric::Cosine, Metric::Dot, Metric::L2] {
            let req = RecallRequest::new(vector(q))
                .top_k(10)
                .metric(metric)
                .track_access(false);
            let a = dbs[0].recall(&req).unwrap();
            let b = dbs[1].recall(&req).unwrap();
            assert_eq!(
                a[0].memory.content, b[0].memory.content,
                "top hit, q {q} {metric:?}"
            );
            // Two files never share ids, so compare by content.
            let cb: Vec<&str> = b.iter().map(|h| h.memory.content.as_str()).collect();
            let common = a
                .iter()
                .filter(|h| cb.contains(&h.memory.content.as_str()))
                .count();
            assert!(common >= 9, "q {q} {metric:?}: only {common}/10 in common");
            // Agent + type filters take the row path on v9.
            let req = req.clone().agent("a").types(vec![MemoryType::Semantic]);
            let a = dbs[0].recall(&req).unwrap();
            let b = dbs[1].recall(&req).unwrap();
            assert_eq!(a.len(), b.len());
            assert!(b
                .iter()
                .all(|h| h.memory.agent_id == "a" && h.memory.memory_type == MemoryType::Semantic));
        }
    }
    assert!(dbs[1].__vector_column().0, "recall loaded the column");
}

#[test]
fn rows_skip_superseded_expired_and_thread_layers_without_decrypting() {
    let dir = tempdir().unwrap();
    let mut db = Mnemo::create(dir.path().join("t.mnemo"), "pw", cfg(9)).unwrap();
    let (old, _) = db.remember_keyed(mem(1), "k").unwrap();
    let (new, _) = db.remember_keyed(mem(1), "k").unwrap();
    // A second keyed write with the same content may be a no-op; force a
    // real supersession with different content.
    let mut changed = mem(1);
    changed.content = "memory 1, revised".into();
    let (newer, _) = db.remember_keyed(changed, "k").unwrap();
    let mut ttl = mem(2);
    ttl.ttl_secs = Some(0);
    let ttl_id = db.remember(ttl).unwrap();
    let mut turn = mem(3);
    turn.metadata.insert("_mnemo.kind".into(), json!("turn"));
    turn.metadata.insert("_mnemo.thread".into(), json!("t1"));
    let turn_id = db.remember(turn).unwrap();
    db.flush().unwrap();

    let hits = db
        .recall(&RecallRequest::new(vector(1)).top_k(10).track_access(false))
        .unwrap();
    let ids = ids_of(&hits);
    assert!(ids.contains(&newer));
    assert!(!ids.contains(&old) || old == newer);
    assert!(!ids.contains(&new) || new == newer);
    assert!(!ids.contains(&ttl_id), "expired");
    assert!(
        !ids.contains(&turn_id),
        "thread turns are out of default recall"
    );

    // A `_mnemo.*` filter opts the turn back in (general path).
    let hits = db
        .recall(
            &RecallRequest::new(vector(3))
                .top_k(5)
                .filter(Filter::default().metadata_eq("_mnemo.thread", json!("t1")))
                .track_access(false),
        )
        .unwrap();
    assert_eq!(ids_of(&hits), vec![turn_id]);
}

#[test]
fn column_survives_reopen_and_pages_when_over_budget() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    let ids: Vec<Ulid> = (0..300).map(|i| db.remember(mem(i)).unwrap()).collect();
    db.close().unwrap();

    let req = RecallRequest::new(vector(42)).top_k(5).track_access(false);
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert!(!db.__vector_column().0, "the column loads lazily");
    let resident = db.recall(&req).unwrap();
    assert!(db.__vector_column().0);
    assert_eq!(resident[0].memory.id, ids[42]);
    drop(db);

    let mut db = Mnemo::open(&path, "pw").unwrap();
    db.set_vector_cache_budget(0);
    let paged = db.recall(&req).unwrap();
    assert!(
        !db.__vector_column().0,
        "over budget: read through the page cache"
    );
    assert_eq!(ids_of(&paged), ids_of(&resident));
    // Both reads decode the same fp16 entries, so similarity matches
    // exactly. The score also weighs recency, which moves between the two
    // recalls, so it only matches closely.
    for (a, b) in paged.iter().zip(&resident) {
        assert_eq!(a.similarity, b.similarity);
        assert!(
            (a.score - b.score).abs() < 1e-4,
            "score {} vs {}",
            a.score,
            b.score
        );
    }
    assert_eq!(db.verify().unwrap(), 300);
}

#[test]
fn vector_pages_are_packed_vacuumed_and_freed() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    db.set_max_snapshots(1);
    // One memory per commit: the vacuum folds each partly filled page into
    // the next commit's page, so pages stay mostly full.
    let ids: Vec<Ulid> = (0..60)
        .map(|i| {
            let id = db.remember(mem(i)).unwrap();
            db.flush().unwrap();
            id
        })
        .collect();
    // 16 dims → 36-byte entries, 226 per page: 60 vectors fit one page.
    let (_, pages) = db.__vector_column();
    assert!(
        pages <= 2,
        "{pages} vector pages for 60 single-commit vectors"
    );
    // Delete everything: every vector page is freed.
    for id in &ids {
        db.delete(id).unwrap();
    }
    db.flush().unwrap();
    assert_eq!(db.__vector_column().1, 0);
    assert_eq!(db.verify().unwrap(), 0);
    // And the space is reused.
    let tail = db.__next_page();
    for i in 0..10 {
        db.remember(mem(100 + i)).unwrap();
    }
    db.flush().unwrap();
    assert!(db.__next_page() <= tail + 2);
    db.close().unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.verify().unwrap(), 10);
}

#[test]
fn restore_brings_back_vectors_from_quarantined_pages() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    let ids: Vec<Ulid> = (0..30).map(|i| db.remember(mem(i)).unwrap()).collect();
    db.flush().unwrap();
    let t1 = db.snapshots().last().unwrap().txn_id;
    for id in &ids {
        db.delete(id).unwrap();
    }
    db.flush().unwrap();
    db.restore_to(t1).unwrap();
    db.set_max_snapshots(1);
    for round in 0..3 {
        let tmp: Vec<Ulid> = (0..20)
            .map(|i| db.remember(mem(500 + round * 20 + i)).unwrap())
            .collect();
        db.flush().unwrap();
        for id in &tmp {
            db.delete(id).unwrap();
        }
        db.flush().unwrap();
    }
    for (i, id) in ids.iter().enumerate() {
        let got = db.get(id).unwrap().vector;
        for (a, b) in vector(i).iter().zip(&got) {
            assert!((a - b).abs() <= a.abs() * 1e-3 + 1e-7);
        }
    }
    let hits = db
        .recall(&RecallRequest::new(vector(7)).top_k(1).track_access(false))
        .unwrap();
    assert_eq!(hits[0].memory.id, ids[7]);
    assert_eq!(db.verify().unwrap(), 30);
}

#[test]
fn small_v9_files_have_no_ann_index() {
    // The dev override exists to run the index suites on v9; it disables
    // exactly what this test checks.
    if std::env::var_os("MNEMO_DEV_ANN_MIN_FACTS").is_some() {
        return;
    }
    let dir = tempdir().unwrap();
    let mut db = Mnemo::create(dir.path().join("t.mnemo"), "pw", cfg(9)).unwrap();
    for i in 0..500 {
        db.remember(mem(i)).unwrap();
    }
    let info = db.build_index().unwrap();
    assert_eq!(info.partitions, 0);
    assert_eq!(info.vectors, 500);
    assert!(!db.has_index());
    let resp = db
        .recall_budgeted(&RecallRequest::new(vector(9)).top_k(3).track_access(false))
        .unwrap();
    assert!(resp.used_exact_scan);
}

#[test]
fn shared_scope_and_metadata_filters_still_work_on_v9() {
    let dir = tempdir().unwrap();
    let mut db = Mnemo::create(dir.path().join("t.mnemo"), "pw", cfg(9)).unwrap();
    let mut shared = mem(5).with_agent("other");
    shared.scope = Scope::Shared;
    let shared_id = db.remember(shared).unwrap();
    let private_id = db.remember(mem(5).with_agent("other")).unwrap();
    let mut tagged = mem(6).with_agent("me");
    tagged.metadata.insert("topic".into(), json!("db"));
    let tagged_id = db.remember(tagged).unwrap();
    let hits = db
        .recall(
            &RecallRequest::new(vector(5))
                .top_k(10)
                .agent("me")
                .track_access(false),
        )
        .unwrap();
    let ids = ids_of(&hits);
    assert!(ids.contains(&shared_id));
    assert!(!ids.contains(&private_id));
    assert!(ids.contains(&tagged_id));
    let hits = db
        .recall(
            &RecallRequest::new(vector(5))
                .top_k(10)
                .filter(Filter::default().metadata_eq("topic", json!("db")))
                .track_access(false),
        )
        .unwrap();
    assert_eq!(ids_of(&hits), vec![tagged_id]);
}

#[test]
fn too_many_dimensions_for_a_v9_page_is_refused() {
    let dir = tempdir().unwrap();
    let res = Mnemo::create(
        dir.path().join("t.mnemo"),
        "pw",
        MnemoConfig {
            dimensions: 4096,
            ..cfg(9)
        },
    );
    assert!(matches!(res, Err(MnemoError::Invalid(_))));
    // v8 is unaffected.
    Mnemo::create(
        dir.path().join("t8.mnemo"),
        "pw",
        MnemoConfig {
            dimensions: 4096,
            ..cfg(8)
        },
    )
    .unwrap();
}

// --- FMT-9b.2: packed record pages -----------------------------------------

fn distinct_pages(db: &Mnemo, ids: &[Ulid]) -> usize {
    let mut pages: Vec<u64> = ids
        .iter()
        .filter_map(|id| db.__record_location(id))
        .flat_map(|(p, c, _)| p..p + c as u64)
        .collect();
    pages.sort_unstable();
    pages.dedup();
    pages.len()
}

#[test]
fn small_records_share_pages_and_survive_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    let ids: Vec<Ulid> = (0..1000).map(|i| db.remember(mem(i)).unwrap()).collect();
    db.flush().unwrap();
    let pages = distinct_pages(&db, &ids);
    // ~130-byte records, ~60 per page.
    assert!(pages <= 25, "1000 small records on {pages} pages");
    db.close().unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    for (i, id) in ids.iter().enumerate().step_by(37) {
        assert_eq!(db.get(id).unwrap().content, format!("memory {i}"));
    }
    assert_eq!(db.verify().unwrap(), 1000);
}

#[test]
fn single_commits_fill_pages_and_deletes_vacuum_them() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    db.set_max_snapshots(1);
    let ids: Vec<Ulid> = (0..200)
        .map(|i| {
            let id = db.remember(mem(i)).unwrap();
            db.flush().unwrap();
            id
        })
        .collect();
    let before = distinct_pages(&db, &ids);
    assert!(before <= 8, "200 single-commit records on {before} pages");
    // Delete most of them: the sparse pages are folded into later commits.
    for id in ids.iter().skip(20) {
        db.delete(id).unwrap();
    }
    for i in 0..10 {
        db.remember(mem(1000 + i)).unwrap();
        db.flush().unwrap();
    }
    let after = distinct_pages(&db, &ids[..20]);
    assert!(after <= 2, "20 survivors still spread over {after} pages");
    for (i, id) in ids[..20].iter().enumerate() {
        assert_eq!(db.get(id).unwrap().content, format!("memory {i}"));
    }
    assert_eq!(db.verify().unwrap(), 30);
    db.close().unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.verify().unwrap(), 30);
}

#[test]
fn big_records_span_whole_pages() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    let big = "y".repeat(20_000);
    let id = db
        .remember(Memory::new(big.clone(), MemoryType::Semantic, vector(1)))
        .unwrap();
    let small = db.remember(mem(2)).unwrap();
    db.flush_index().unwrap();
    let (_, pages, _) = db.__record_location(&id).unwrap();
    assert_eq!(pages, 3, "a 20 KB record owns three pages");
    assert_eq!(db.__record_location(&small).unwrap().1, 1);
    drop(db);
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.get(&id).unwrap().content, big);
    db.delete(&id).unwrap();
    db.flush().unwrap();
    assert_eq!(db.verify().unwrap(), 1);
}

struct AxisEmbedder;
impl Embedder for AxisEmbedder {
    fn identity(&self) -> EmbedderIdentity {
        EmbedderIdentity {
            name: "axis".into(),
            dimensions: DIMS,
            normalize: false,
        }
    }
    fn embed(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| vector(t.len())).collect())
    }
}

#[test]
fn long_turns_chunk_to_fit_one_packed_page() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    db.set_embedder(AxisEmbedder).unwrap();
    let long = "z".repeat(30_000);
    db.ingest_thread(
        "chat",
        &[ThreadTurn {
            turn_id: "t1".into(),
            role: "user".into(),
            content: long.clone(),
            timestamp: None,
        }],
    )
    .unwrap();
    db.flush_index().unwrap();
    let chunks = db.thread_export("chat").unwrap();
    assert_eq!(chunks.len(), 4, "30,000 bytes in 7,900-byte chunks");
    for m in &chunks {
        let (_, pages, _) = db.__record_location(&m.id).unwrap();
        assert_eq!(pages, 1, "a chunk and its metadata fit one page");
    }
    drop(db);
    let mut db = Mnemo::open(&path, "pw").unwrap();
    let turns = db.thread_export_turns("chat").unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].content, long);
}

#[test]
fn a_record_deleted_before_its_first_flush_reopens_cleanly() {
    // Regression: the deleted row never got a page, and its zero
    // generation was left out of the row encoding, shifting the v9 fields.
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    let keep = db.remember(mem(1)).unwrap();
    let gone = db.remember(mem(2)).unwrap();
    db.delete(&gone).unwrap();
    db.close().unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.len(), 1);
    db.get(&keep).unwrap();
    assert!(db.get(&gone).is_err());
    assert_eq!(db.verify().unwrap(), 1);
}
