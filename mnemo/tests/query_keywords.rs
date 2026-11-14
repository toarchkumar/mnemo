//! 12.3 step 5 (v0.15): `Embedder::query_keywords`, the hook the built-in
//! embedder's identifier-aware recall goes through. A test embedder chooses
//! the keywords, so this runs in every build (no model).

use std::sync::{Arc, Mutex};

use mnemo_engine::{
    Embedder, EmbedderIdentity, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, RecallRequest,
    SharedOptions,
};
use tempfile::tempdir;

const PASS: &str = "query-keywords-test-passphrase";

/// Queries all embed to the first axis; `keywords` is what the hook says.
struct Chooser {
    keywords: Arc<Mutex<Option<String>>>,
}

impl Embedder for Chooser {
    fn identity(&self) -> EmbedderIdentity {
        EmbedderIdentity {
            name: "chooser".into(),
            dimensions: 4,
            normalize: true,
            ..Default::default()
        }
    }
    fn embed(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![1.0, 0.0, 0.0, 0.0]).collect())
    }
    fn query_keywords(&self, _text: &str) -> Option<String> {
        self.keywords.lock().unwrap().clone()
    }
}

const QUESTION: &str = "what does snapshot_list return";

/// A brain where the vector favours `alpha` and the keyword
/// `snapshot_list` favours the other memory.
fn brain(dir: &std::path::Path) -> (std::path::PathBuf, Arc<Mutex<Option<String>>>) {
    let p = dir.join("k.mnemo");
    let mut db = Mnemo::create(
        &p,
        PASS,
        MnemoConfig {
            dimensions: 4,
            kdf: KdfParams::fast(),
            ..Default::default()
        },
    )
    .unwrap();
    db.remember(Memory::new(
        String::from("alpha: a note about lists"),
        MemoryType::Semantic,
        vec![1.0, 0.0, 0.0, 0.0],
    ))
    .unwrap();
    db.remember(Memory::new(
        String::from("the snapshot_list call returns every snapshot"),
        MemoryType::Semantic,
        vec![0.6, 0.8, 0.0, 0.0],
    ))
    .unwrap();
    db.close().unwrap();
    (p, Arc::new(Mutex::new(None)))
}

fn top(db: &mut Mnemo, req: &RecallRequest) -> String {
    db.recall(&req.clone().track_access(false)).unwrap()[0]
        .memory
        .content
        .clone()
}

#[test]
fn the_embedder_chooses_the_keywords_unless_the_caller_does() {
    let dir = tempdir().unwrap();
    let (p, kw) = brain(dir.path());
    let mut db = Mnemo::open(&p, PASS).unwrap();
    // Registered boxed, as the CLI does, so the Box impl must pass it on.
    let boxed: Box<dyn Embedder> = Box::new(Chooser {
        keywords: kw.clone(),
    });
    db.set_embedder(boxed).unwrap();
    let embedder = db.embedder_arc().unwrap();
    let req = RecallRequest::from_text(embedder.as_ref(), QUESTION).unwrap();

    // Some(identifiers): scored as explicit keywords, at 0.5.
    *kw.lock().unwrap() = Some("snapshot_list".into());
    assert!(top(&mut db, &req).contains("snapshot_list"));
    // Some(""): vector only, though the text names snapshot_list.
    *kw.lock().unwrap() = Some(String::new());
    assert!(top(&mut db, &req).starts_with("alpha"));
    let vector_only = db.recall(&req.clone().track_access(false)).unwrap();
    let explicit = db
        .recall(&req.clone().keywords("").track_access(false))
        .unwrap();
    assert_eq!(vector_only[1].score, explicit[1].score);

    // Keywords the caller gives win over the hook, both ways.
    *kw.lock().unwrap() = Some("snapshot_list".into());
    assert!(top(&mut db, &req.clone().keywords("")).starts_with("alpha"));
    *kw.lock().unwrap() = Some(String::new());
    assert!(top(&mut db, &req.clone().keywords("snapshot_list")).contains("snapshot_list"));

    // None: the text itself, at the text weight (as before v0.15).
    *kw.lock().unwrap() = None;
    let by_text = db.recall(&req.clone().track_access(false)).unwrap();
    let as_keywords = db
        .recall(
            &req.clone()
                .keywords(QUESTION)
                .keyword_weight(req.text_keyword_weight)
                .track_access(false),
        )
        .unwrap();
    let scores = |h: &[mnemo_engine::RecallResult]| h.iter().map(|r| r.score).collect::<Vec<_>>();
    assert_eq!(scores(&by_text), scores(&as_keywords));
}

#[test]
fn only_a_recall_by_text_asks() {
    let dir = tempdir().unwrap();
    let (p, kw) = brain(dir.path());
    let mut db = Mnemo::open(&p, PASS).unwrap();
    db.set_embedder(Chooser {
        keywords: kw.clone(),
    })
    .unwrap();
    *kw.lock().unwrap() = Some("snapshot_list".into());
    // A plain vector request has no text: the hook isn't asked.
    let plain = RecallRequest::new(vec![1.0, 0.0, 0.0, 0.0]);
    assert!(top(&mut db, &plain).starts_with("alpha"));
    // A keyword-only request keeps its own keywords.
    let kw_only = RecallRequest::keywords_only("alpha");
    assert!(top(&mut db, &kw_only).starts_with("alpha"));
}

#[test]
fn a_shared_handle_asks_its_embedder_too() {
    let dir = tempdir().unwrap();
    let (p, kw) = brain(dir.path());
    let mut s = Mnemo::open_shared(&p, PASS, SharedOptions::default()).unwrap();
    s.set_embedder(Chooser {
        keywords: kw.clone(),
    })
    .unwrap();
    *kw.lock().unwrap() = Some("snapshot_list".into());
    let req = RecallRequest::new(vec![1.0, 0.0, 0.0, 0.0])
        .text(QUESTION)
        .track_access(false);
    let hits = s.read(|db| db.recall(&req)).unwrap();
    assert!(hits[0].memory.content.contains("snapshot_list"));
}
