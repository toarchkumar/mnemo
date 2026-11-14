//! 12.3 step 4 (v0.15): retrieval. The query side of an embedder, hybrid
//! recall by default, tokenizer 2 (TOK-1) and its mixed state, and
//! `reembed_file`.

use std::path::Path;

use mnemo_engine::{
    CachePutOpts, Embedder, EmbedderIdentity, KdfParams, KeywordTokenizer, Memory, MemoryType,
    Mnemo, MnemoConfig, MnemoError, RecallRequest, SemanticCachePutOpts,
};
use tempfile::tempdir;

const PASS: &str = "retrieval-test-passphrase";

fn cfg(dimensions: usize) -> MnemoConfig {
    MnemoConfig {
        dimensions,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

/// A test embedder: documents and queries get fixed vectors, so a test
/// can tell which side a text went through.
struct Sides {
    id: EmbedderIdentity,
    doc: Vec<f32>,
    query: Vec<f32>,
}

impl Embedder for Sides {
    fn identity(&self) -> EmbedderIdentity {
        self.id.clone()
    }
    fn embed(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| self.doc.clone()).collect())
    }
    fn embed_query(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| self.query.clone()).collect())
    }
}

fn identity(name: &str, dimensions: usize, convention: &str) -> EmbedderIdentity {
    EmbedderIdentity {
        name: name.into(),
        dimensions,
        normalize: true,
        query_convention: convention.into(),
    }
}

/// A text-dependent embedder of any width, with no query side of its own.
struct Hashing {
    id: EmbedderIdentity,
    fail_after: Option<usize>,
    calls: std::sync::atomic::AtomicUsize,
}

fn hashing(name: &str, dimensions: usize) -> Hashing {
    Hashing {
        id: identity(name, dimensions, ""),
        fail_after: None,
        calls: Default::default(),
    }
}

impl Embedder for Hashing {
    fn identity(&self) -> EmbedderIdentity {
        self.id.clone()
    }
    fn embed(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
        let n = self
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if self.fail_after.is_some_and(|f| n >= f) {
            return Err(MnemoError::Invalid("the embedder went away".into()));
        }
        let d = self.id.dimensions;
        Ok(texts
            .iter()
            .map(|t| {
                let mut v = vec![0.1f32; d];
                v[t.len() % d] = 1.0;
                v
            })
            .collect())
    }
}

fn mem(text: &str, vector: Vec<f32>) -> Memory {
    Memory::new(text.to_string(), MemoryType::Semantic, vector)
}

fn contents(hits: Vec<mnemo_engine::RecallResult>) -> Vec<String> {
    hits.into_iter().map(|h| h.memory.content).collect()
}

fn keyword_hits(db: &mut Mnemo, k: &str) -> Vec<String> {
    contents(
        db.recall(&RecallRequest::keywords_only(k).track_access(false))
            .unwrap(),
    )
}

// --- the query side --------------------------------------------------------

#[test]
fn a_recall_by_text_embeds_with_the_query_side() {
    let dir = tempdir().unwrap();
    let mut db = Mnemo::create(dir.path().join("q.mnemo"), PASS, cfg(4)).unwrap();
    db.remember(mem("doc-side", vec![1.0, 0.0, 0.0, 0.0]))
        .unwrap();
    db.remember(mem("query-side", vec![0.0, 1.0, 0.0, 0.0]))
        .unwrap();
    let e = Sides {
        id: identity("sides", 4, "test-query"),
        doc: vec![1.0, 0.0, 0.0, 0.0],
        query: vec![0.0, 1.0, 0.0, 0.0],
    };
    let req = RecallRequest::from_text(&e, "anything")
        .unwrap()
        .keywords("");
    assert_eq!(req.text.as_deref(), Some("anything"));
    let hits = contents(db.recall(&req.top_k(1)).unwrap());
    assert_eq!(hits, vec!["query-side"]);
}

#[test]
fn the_query_convention_is_part_of_the_identity() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("id.mnemo");
    {
        let mut db = Mnemo::create(&p, PASS, cfg(4)).unwrap();
        db.remember(Memory::scaffold_manifest(4)).unwrap();
        db.set_embedder(hashing("model", 4)).unwrap();
        db.flush().unwrap();
    }
    let mut db = Mnemo::open(&p, PASS).unwrap();
    // A symmetric embedder records no convention, as before v0.15.
    let manifest = db.about().unwrap().remove(0);
    let recorded = manifest.metadata["embedder"].as_object().unwrap();
    assert!(!recorded.contains_key("query_convention"), "{recorded:?}");
    assert_eq!(
        db.embedder_identity().unwrap().unwrap().query_convention,
        ""
    );
    // The same model with a query convention is another embedder.
    let other = Sides {
        id: identity("model", 4, "bge-query-instruction"),
        doc: vec![1.0, 0.0, 0.0, 0.0],
        query: vec![0.0, 1.0, 0.0, 0.0],
    };
    match db.set_embedder(other) {
        Err(MnemoError::EmbedderMismatch { stored, given }) => {
            assert_eq!(stored.query_convention, "");
            assert_eq!(given.query_convention, "bge-query-instruction");
            let msg = MnemoError::EmbedderMismatch { stored, given }.to_string();
            assert!(
                msg.contains("query_convention=\"bge-query-instruction\""),
                "{msg}"
            );
            assert!(msg.contains("mnemo reembed"), "{msg}");
        }
        other => panic!("expected a mismatch, got {other:?}"),
    }
}

#[test]
fn a_convention_is_recorded_and_checked_on_the_next_open() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("conv.mnemo");
    let with = || Sides {
        id: identity("model", 4, "q1"),
        doc: vec![1.0, 0.0, 0.0, 0.0],
        query: vec![0.0, 1.0, 0.0, 0.0],
    };
    {
        let mut db = Mnemo::create(&p, PASS, cfg(4)).unwrap();
        db.remember(Memory::scaffold_manifest(4)).unwrap();
        db.set_embedder(with()).unwrap();
        db.flush().unwrap();
    }
    let mut db = Mnemo::open(&p, PASS).unwrap();
    let manifest = db.about().unwrap().remove(0);
    assert_eq!(manifest.metadata["embedder"]["query_convention"], "q1");
    assert!(matches!(
        db.set_embedder(hashing("model", 4)),
        Err(MnemoError::EmbedderMismatch { .. })
    ));
    db.set_embedder(with()).unwrap();
}

// --- hybrid by default -----------------------------------------------------

/// The vector alone ranks `alpha` first (similarity 1.0 against 0.85); the
/// text's keywords (weight 0.25) pull up the memory that names
/// `mnemo.nosync`.
fn hybrid_file(dir: &Path) -> Mnemo {
    let mut db = Mnemo::create(dir.join("h.mnemo"), PASS, cfg(4)).unwrap();
    db.remember(mem("alpha beta gamma", vec![1.0, 0.0, 0.0, 0.0]))
        .unwrap();
    db.remember(mem(
        "the deploy key is in mnemo.nosync",
        vec![0.85, 0.527, 0.0, 0.0],
    ))
    .unwrap();
    db
}

fn query_embedder() -> Sides {
    Sides {
        id: identity("q", 4, ""),
        doc: vec![1.0, 0.0, 0.0, 0.0],
        query: vec![1.0, 0.0, 0.0, 0.0],
    }
}

#[test]
fn a_recall_by_text_is_hybrid_by_default_and_empty_keywords_opt_out() {
    let dir = tempdir().unwrap();
    let mut db = hybrid_file(dir.path());
    let e = query_embedder();
    let hybrid = RecallRequest::from_text(&e, "where is mnemo.nosync").unwrap();
    assert_eq!(
        contents(db.recall(&hybrid.clone().top_k(1)).unwrap()),
        vec!["the deploy key is in mnemo.nosync"]
    );
    let vector_only = hybrid.clone().keywords("");
    assert_eq!(
        contents(db.recall(&vector_only.top_k(1)).unwrap()),
        vec!["alpha beta gamma"]
    );
    // Keywords given replace the text.
    let replaced = hybrid.keywords("gamma");
    assert_eq!(
        contents(db.recall(&replaced.top_k(1)).unwrap()),
        vec!["alpha beta gamma"]
    );
    // A request built from a vector has no text: plain vector recall.
    let plain = RecallRequest::new(vec![1.0, 0.0, 0.0, 0.0]).top_k(1);
    assert_eq!(
        contents(db.recall(&plain).unwrap()),
        vec!["alpha beta gamma"]
    );
}

#[test]
fn a_v8_file_recalls_by_text_without_the_keyword_index() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v8/v8_passphrase.mnemo");
    let dir = tempdir().unwrap();
    let p = dir.path().join("v8.mnemo");
    std::fs::copy(&fixture, &p).unwrap();
    let mut db = Mnemo::open_read_only(&p, "v8-fixture-passphrase").unwrap();
    let d = db.dimensions();
    let e = hashing("whatever", d);
    let req = RecallRequest::from_text(&e, "anything at all")
        .unwrap()
        .track_access(false);
    // The text is ignored on v8 (no keyword index), so this doesn't fail.
    assert!(!db.recall(&req).unwrap().is_empty());
    assert_eq!(db.stats().unwrap().keyword_tokenizer, None);
    // Explicit keywords still need v9, as before.
    let req = RecallRequest::from_text(&e, "x")
        .unwrap()
        .keywords("y")
        .track_access(false);
    assert!(db.recall(&req).is_err());
}

// --- TOK-1 -----------------------------------------------------------------

#[test]
fn a_new_file_uses_tokenizer_2() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("t2.mnemo");
    {
        let mut db = Mnemo::create(&p, PASS, cfg(4)).unwrap();
        assert_eq!(
            db.stats().unwrap().keyword_tokenizer,
            Some(KeywordTokenizer::V2)
        );
        db.remember(mem(
            "The repo lives at mnemo.nosync",
            vec![1.0, 0.0, 0.0, 0.0],
        ))
        .unwrap();
        db.remember(mem("out of iCloud's reach", vec![0.0, 1.0, 0.0, 0.0]))
            .unwrap();
        // Before the commit too: the pending documents are tokenizer 2.
        assert_eq!(
            keyword_hits(&mut db, "nosync"),
            vec!["The repo lives at mnemo.nosync"]
        );
        db.flush().unwrap();
    }
    let mut db = Mnemo::open(&p, PASS).unwrap();
    assert_eq!(
        db.stats().unwrap().keyword_tokenizer,
        Some(KeywordTokenizer::V2)
    );
    assert_eq!(
        db.verify_report().unwrap().keyword_tokenizer,
        Some(KeywordTokenizer::V2)
    );
    assert_eq!(
        keyword_hits(&mut db, "nosync"),
        vec!["The repo lives at mnemo.nosync"]
    );
    assert_eq!(
        keyword_hits(&mut db, "icloud"),
        vec!["out of iCloud's reach"]
    );
    // Whole tokens still match.
    assert_eq!(
        keyword_hits(&mut db, "mnemo.nosync"),
        vec!["The repo lives at mnemo.nosync"]
    );
}

#[test]
fn segment_merges_keep_the_mark() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("merge.mnemo");
    let mut db = Mnemo::create(&p, PASS, cfg(4)).unwrap();
    // Past BM25_MAX_SEGMENTS (8) commits, segments merge.
    for i in 0..20 {
        db.remember(mem(
            &format!("note {i} about part{i}.whole"),
            vec![1.0, i as f32, 0.0, 0.0],
        ))
        .unwrap();
        db.flush().unwrap();
    }
    assert_eq!(
        db.stats().unwrap().keyword_tokenizer,
        Some(KeywordTokenizer::V2)
    );
    assert_eq!(
        keyword_hits(&mut db, "part3"),
        vec!["note 3 about part3.whole"]
    );
    drop(db);
    let mut db = Mnemo::open(&p, PASS).unwrap();
    assert_eq!(
        db.stats().unwrap().keyword_tokenizer,
        Some(KeywordTokenizer::V2)
    );
    assert_eq!(
        keyword_hits(&mut db, "part17"),
        vec!["note 17 about part17.whole"]
    );
}

#[test]
fn an_older_builds_writes_make_the_index_mixed_until_a_compaction() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("mixed.mnemo");
    {
        let mut db = Mnemo::create(&p, PASS, cfg(4)).unwrap();
        db.remember(mem("new side: mnemo.nosync", vec![1.0, 0.0, 0.0, 0.0]))
            .unwrap();
        db.flush().unwrap();
    }
    {
        let mut db = Mnemo::open(&p, PASS).unwrap();
        db.__write_keywords_like_an_older_build_for_testing();
        db.remember(mem("old side: other.place", vec![0.0, 1.0, 0.0, 0.0]))
            .unwrap();
        db.flush().unwrap();
    }
    let mut db = Mnemo::open(&p, PASS).unwrap();
    assert_eq!(
        db.stats().unwrap().keyword_tokenizer,
        Some(KeywordTokenizer::Mixed)
    );
    assert_eq!(
        db.verify_report().unwrap().keyword_tokenizer,
        Some(KeywordTokenizer::Mixed)
    );
    // Parts match the tokenizer 2 documents only; whole words match all.
    assert_eq!(
        keyword_hits(&mut db, "nosync"),
        vec!["new side: mnemo.nosync"]
    );
    assert!(keyword_hits(&mut db, "place").is_empty());
    assert_eq!(
        keyword_hits(&mut db, "other.place"),
        vec!["old side: other.place"]
    );
    // v0.15's own writes to a mixed index are tokenizer 2, and its merges
    // keep the index mixed rather than calling it tokenizer 1.
    for i in 0..12 {
        db.remember(mem(
            &format!("later {i}: x{i}.y"),
            vec![1.0, 1.0, i as f32, 0.0],
        ))
        .unwrap();
        db.flush().unwrap();
    }
    assert_eq!(
        db.stats().unwrap().keyword_tokenizer,
        Some(KeywordTokenizer::Mixed)
    );
    assert_eq!(keyword_hits(&mut db, "x5"), vec!["later 5: x5.y"]);
    drop(db);
    Mnemo::compact_file(p.to_str().unwrap(), PASS).unwrap();
    let mut db = Mnemo::open(&p, PASS).unwrap();
    assert_eq!(
        db.stats().unwrap().keyword_tokenizer,
        Some(KeywordTokenizer::V2)
    );
    assert_eq!(
        keyword_hits(&mut db, "place"),
        vec!["old side: other.place"]
    );
}

#[test]
fn a_tokenizer_1_file_stays_tokenizer_1_until_compacted() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tok1/v0_14_0_tokenizer_1.mnemo");
    let dir = tempdir().unwrap();
    let p = dir.path().join("v1.mnemo");
    std::fs::copy(&fixture, &p).unwrap();
    let pass = "tok1-fixture-passphrase";
    {
        let mut db = Mnemo::open(&p, pass).unwrap();
        assert_eq!(
            db.stats().unwrap().keyword_tokenizer,
            Some(KeywordTokenizer::V1)
        );
        // Whole words only, as v0.14.0 wrote them.
        assert!(keyword_hits(&mut db, "nosync").is_empty());
        assert_eq!(
            keyword_hits(&mut db, "mnemo.nosync"),
            vec!["The repo lives at ~/Documents/GitHub/mnemo.nosync"]
        );
        // A v0.15 write keeps it pure tokenizer 1.
        db.remember(mem("added by v0.15: other.place", vec![1.0, 2.0, 3.0, 0.0]))
            .unwrap();
        db.flush().unwrap();
    }
    {
        let mut db = Mnemo::open_read_only(&p, pass).unwrap();
        assert_eq!(
            db.stats().unwrap().keyword_tokenizer,
            Some(KeywordTokenizer::V1)
        );
        assert!(keyword_hits(&mut db, "place").is_empty());
    }
    // The CLI says how to rebuild it.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mnemo"))
        .args(["info", p.to_str().unwrap()])
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_CHILD_SESSION")
        .env_remove("MNEMO_KEY")
        .env_remove("MNEMO_KEY_FILE")
        .env_remove("MNEMO_KEY_COMMAND")
        .env("MNEMO_PASSPHRASE", pass)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("keywords:    tokenizer 1"), "{text}");
    assert!(text.contains("mnemo compact"), "{text}");
    Mnemo::compact_file(p.to_str().unwrap(), pass).unwrap();
    let mut db = Mnemo::open_read_only(&p, pass).unwrap();
    assert_eq!(
        db.stats().unwrap().keyword_tokenizer,
        Some(KeywordTokenizer::V2)
    );
    assert_eq!(
        keyword_hits(&mut db, "nosync"),
        vec!["The repo lives at ~/Documents/GitHub/mnemo.nosync"]
    );
    assert_eq!(
        keyword_hits(&mut db, "icloud"),
        vec!["The old folder was one of iCloud's placeholders"]
    );
    assert_eq!(
        keyword_hits(&mut db, "place"),
        vec!["added by v0.15: other.place"]
    );
}

// --- reembed ---------------------------------------------------------------

fn reembed_source(dir: &Path, n: usize) -> std::path::PathBuf {
    let p = dir.join("r.mnemo");
    let mut db = Mnemo::create(&p, PASS, cfg(4)).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.set_embedder(hashing("old-model", 4)).unwrap();
    for i in 0..n {
        db.remember_text(&format!("fact number {i}: v0.{i}.0"), MemoryType::Semantic)
            .unwrap();
    }
    db.cache_put("tools", "k", b"value", CachePutOpts::default())
        .unwrap();
    db.cache_put_semantic(
        "prompts",
        "p",
        vec![1.0, 0.0, 0.0, 0.0],
        b"answer",
        SemanticCachePutOpts::new("old-model"),
    )
    .unwrap();
    db.flush().unwrap();
    p
}

#[test]
fn reembed_moves_a_file_to_a_new_embedder_and_width() {
    let dir = tempdir().unwrap();
    let p = reembed_source(dir.path(), 10);
    let new = hashing("new-model", 8);
    let report = Mnemo::reembed_file(p.to_str().unwrap(), PASS, &new).unwrap();
    assert_eq!(report.memories, 10);
    assert_eq!(report.manifests, 1);
    assert_eq!(report.cache_entries, 1);
    assert_eq!(report.cache_dropped, 1);
    assert_eq!(report.from.unwrap().name, "old-model");
    assert_eq!(report.to.name, "new-model");

    let mut db = Mnemo::open(&p, PASS).unwrap();
    assert_eq!(db.dimensions(), 8);
    assert_eq!(db.len(), 11);
    assert_eq!(db.embedder_identity().unwrap().unwrap().name, "new-model");
    let manifest = db.about().unwrap().remove(0);
    assert_eq!(manifest.metadata["embedder"]["name"], "new-model");
    assert_eq!(manifest.metadata["embedder"]["dimensions"], 8);
    assert!(matches!(
        db.set_embedder(hashing("old-model", 4)),
        Err(MnemoError::DimensionMismatch { .. })
    ));
    db.set_embedder(hashing("new-model", 8)).unwrap();
    let e = hashing("new-model", 8);
    let req = RecallRequest::from_text(&e, "v0.3.0").unwrap().top_k(1);
    assert_eq!(
        contents(db.recall(&req).unwrap()),
        vec!["fact number 3: v0.3.0"]
    );
    assert_eq!(db.cache_get("tools", "k").unwrap().unwrap().value, b"value");
    assert_eq!(
        db.stats().unwrap().keyword_tokenizer,
        Some(KeywordTokenizer::V2)
    );
    db.verify().unwrap();
}

#[test]
fn reembed_can_change_only_the_query_convention() {
    let dir = tempdir().unwrap();
    let p = reembed_source(dir.path(), 3);
    let new = Sides {
        id: identity("old-model", 4, "q1"),
        doc: vec![1.0, 0.5, 0.0, 0.0],
        query: vec![0.5, 1.0, 0.0, 0.0],
    };
    Mnemo::reembed_file(p.to_str().unwrap(), PASS, &new).unwrap();
    let mut db = Mnemo::open(&p, PASS).unwrap();
    assert_eq!(
        db.embedder_identity().unwrap().unwrap().query_convention,
        "q1"
    );
    assert!(db.set_embedder(hashing("old-model", 4)).is_err());
    db.set_embedder(new).unwrap();
}

#[test]
fn an_interrupted_reembed_leaves_the_file_as_it_was() {
    let dir = tempdir().unwrap();
    // 70 memories: two batches of 64, and the embedder fails on the second.
    let p = reembed_source(dir.path(), 70);
    let mut failing = hashing("new-model", 4);
    failing.fail_after = Some(1);
    let err = Mnemo::reembed_file(p.to_str().unwrap(), PASS, &failing).unwrap_err();
    assert!(err.to_string().contains("the embedder went away"), "{err}");
    let mut db = Mnemo::open(&p, PASS).unwrap();
    assert_eq!(db.len(), 71);
    assert_eq!(db.embedder_identity().unwrap().unwrap().name, "old-model");
    db.set_embedder(hashing("old-model", 4)).unwrap();
    db.verify().unwrap();
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains("reembed"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

/// An embedder that records every text it's sent.
struct Recording {
    inner: Hashing,
    seen: std::sync::Mutex<Vec<String>>,
}

impl Embedder for Recording {
    fn identity(&self) -> EmbedderIdentity {
        self.inner.identity()
    }
    fn embed(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
        self.seen
            .lock()
            .unwrap()
            .extend(texts.iter().map(|t| t.to_string()));
        self.inner.embed(texts)
    }
}

/// R-008.3: a thread link keeps its sentinel vector through a reembed at a
/// new width, isn't sent to the embedder, and the continuation it records
/// still works.
#[test]
fn reembed_keeps_a_thread_links_sentinel_and_its_continuation() {
    let dir = tempdir().unwrap();
    let p = reembed_source(dir.path(), 2);
    let link = {
        let mut db = Mnemo::open(&p, PASS).unwrap();
        db.set_embedder(hashing("old-model", 4)).unwrap();
        db.essence_add(
            mnemo_engine::EssenceScope::Thread("t1".into()),
            mnemo_engine::EssenceSection::Open,
            "fix the flaky valve test",
            None,
        )
        .unwrap();
        let link = db.thread_start("t2", "t1").unwrap();
        db.flush().unwrap();
        link
    };
    let new = Recording {
        inner: hashing("new-model", 8),
        seen: Default::default(),
    };
    let report = Mnemo::reembed_file(p.to_str().unwrap(), PASS, &new).unwrap();
    assert_eq!(report.thread_links, 1);
    assert_eq!(report.manifests, 1);
    // Two facts and the essence item were embedded; the link wasn't.
    assert_eq!(report.memories, 3);
    let seen = new.seen.lock().unwrap().clone();
    assert!(
        seen.iter().all(|t| !t.starts_with("thread link:")),
        "{seen:?}"
    );
    assert!(
        seen.iter().any(|t| t == "fix the flaky valve test"),
        "{seen:?}"
    );

    let mut db = Mnemo::open(&p, PASS).unwrap();
    let m = db.get(&link).unwrap();
    let s = 1.0 / (8f32).sqrt();
    assert_eq!(m.vector.len(), 8);
    assert!(
        m.vector.iter().all(|x| (x - s).abs() < 1e-3),
        "{:?}",
        m.vector
    );
    let essence = db
        .essence(mnemo_engine::EssenceScope::Thread("t2".into()), 4096)
        .unwrap();
    assert!(essence.contains("fix the flaky valve test"), "{essence}");
}

/// Check 3: the segment mark is score-neutral. The same documents, with
/// their segments marked and with the mark left out, give identical raw
/// BM25 scores for a query of whole words (N, avgdl and every document's
/// length are the rows', and no query term is the mark).
#[test]
fn the_segment_mark_changes_no_score() {
    let texts = [
        "tea, not coffee, before the release",
        "the release needs one approval",
        "coffee at the release party, mnemo.nosync",
        "nothing about drinks",
    ];
    let scores = |unmarked: bool| {
        let dir = tempdir().unwrap();
        let p = dir.path().join("m.mnemo");
        let mut db = Mnemo::create(&p, PASS, cfg(4)).unwrap();
        if unmarked {
            db.__write_keyword_segments_without_the_mark_for_testing();
        }
        // One commit, so every document is split with tokenizer 2 in both
        // files (an unmarked segment would make later ones tokenizer 1).
        let mut ids = Vec::new();
        for (i, t) in texts.iter().enumerate() {
            ids.push(db.remember(mem(t, vec![1.0, i as f32, 0.0, 0.0])).unwrap());
        }
        db.flush().unwrap();
        drop(db);
        let mut db = Mnemo::open(&p, PASS).unwrap();
        let want = if unmarked {
            KeywordTokenizer::V1
        } else {
            KeywordTokenizer::V2
        };
        assert_eq!(db.stats().unwrap().keyword_tokenizer, Some(want));
        let s = db.__bm25_scores_for_testing("release coffee tea").unwrap();
        // Report by position, since the two files' ids differ.
        s.into_iter()
            .map(|(id, x)| (ids.iter().position(|i| *i == id).unwrap(), x))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let marked = scores(false);
    let plain = scores(true);
    assert_eq!(marked.len(), 3, "{marked:?}");
    assert_eq!(marked, plain);
}
