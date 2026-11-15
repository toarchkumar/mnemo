//! 13a step 1 (v0.17): the plumbing under messages, tasks, claims and
//! presence (`docs/design/13a-coordination.md`): the four kinds are never
//! recall, keyword, ANN or `about` hits; visibility and freshness are the
//! caller's to set, checked; a handle's author is stamped on new records;
//! and the manifest's default agent.

use mnemo_engine::{
    KdfParams, Memory, MemoryPatch, MemoryType, Mnemo, MnemoConfig, MnemoError, RecallRequest,
    SharedOptions, WriteOutcome, COORDINATION_KINDS, MNEMO_META_AS_OF, MNEMO_META_AUTHOR,
    MNEMO_META_KIND, MNEMO_META_REVIEW_BY, MNEMO_META_VISIBILITY,
};
use serde_json::{json, Map, Value};

const PW: &str = "coordination-plumbing-test-passphrase";

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn fact(text: &str, v: [f32; 4]) -> Memory {
    Memory::new(text, MemoryType::Semantic, v.to_vec())
}

#[test]
fn coordination_kinds_are_never_recall_keyword_ann_or_about_hits() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Mnemo::create(dir.path().join("c.mnemo"), PW, cfg()).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    let fact_id = db
        .remember(fact("deploy target is staging seven", [1.0, 0.0, 0.0, 0.0]))
        .unwrap();
    let mut onboarding = Map::new();
    onboarding.insert("area".into(), json!("onboarding"));
    for kind in COORDINATION_KINDS {
        db.__put_coordination_for_testing(
            kind,
            "deploy target is staging seven",
            onboarding.clone(),
        )
        .unwrap();
    }
    db.flush().unwrap();
    for _ in 0..2 {
        // By vector (the sentinel is all-equal, so this query is its own),
        // by keywords, and by a kind filter that names each.
        let q = vec![0.5, 0.5, 0.5, 0.5];
        // (The scaffold manifest is a fact with the sentinel vector too, so
        // it may be a hit; no coordination record is.)
        let hits = db
            .recall(&RecallRequest::new(q.clone()).top_k(20).track_access(false))
            .unwrap();
        assert!(hits.iter().any(|h| h.memory.id == fact_id));
        assert!(
            hits.iter()
                .all(|h| !mnemo_engine::is_coordination(&h.memory)),
            "{hits:?}"
        );
        let hits = db
            .recall(
                &RecallRequest::keywords_only("staging seven")
                    .top_k(20)
                    .track_access(false),
            )
            .unwrap();
        assert_eq!(
            hits.iter().map(|h| h.memory.id).collect::<Vec<_>>(),
            [fact_id]
        );
        for kind in COORDINATION_KINDS {
            let f = mnemo_engine::Filter::default().metadata_eq(MNEMO_META_KIND, json!(kind));
            let hits = db
                .recall(
                    &RecallRequest::new(q.clone())
                        .top_k(20)
                        .track_access(false)
                        .filter(f),
                )
                .unwrap();
            assert!(hits.is_empty(), "{kind}");
        }
        assert!(db
            .about()
            .unwrap()
            .iter()
            .all(|m| !mnemo_engine::is_coordination(m)));
        assert!(db
            .recent(20)
            .unwrap()
            .iter()
            .all(|m| !mnemo_engine::is_coordination(m)));
        // The ANN too, once there's an index.
        db.build_index().unwrap();
        let hits = db
            .recall(&RecallRequest::new(q).top_k(20).track_access(false))
            .unwrap();
        assert!(hits
            .iter()
            .all(|h| !mnemo_engine::is_coordination(&h.memory)));
        // And after a reopen, from the stored rows.
        let p = dir.path().join("c.mnemo");
        db.close().unwrap();
        db = Mnemo::open(&p, PW).unwrap();
    }
    // They are records all the same: in `memories()`, with the sentinel
    // vector, in the shared scope.
    let all = db.memories().unwrap();
    let coord: Vec<&Memory> = all
        .iter()
        .filter(|m| mnemo_engine::is_coordination(m))
        .collect();
    assert_eq!(coord.len(), 4);
    assert!(coord.iter().all(|m| m.scope == mnemo_engine::Scope::Shared));
}

#[test]
fn visibility_and_freshness_are_the_callers_checked_and_nothing_else_is() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Mnemo::create(dir.path().join("v.mnemo"), PW, cfg()).unwrap();
    let id = db
        .remember(
            fact("a reviewed fact", [1.0, 0.0, 0.0, 0.0])
                .with_visibility("private")
                .with_review_by(1_900_000_000)
                .with_as_of(1_700_000_000),
        )
        .unwrap();
    let m = db.get(&id).unwrap();
    assert_eq!(m.metadata[MNEMO_META_VISIBILITY], json!("private"));
    assert_eq!(m.metadata[MNEMO_META_REVIEW_BY], json!(1_900_000_000));
    assert_eq!(m.metadata[MNEMO_META_AS_OF], json!(1_700_000_000));
    // Bad values, and any other reserved key, are refused.
    for bad in [
        fact("x", [1.0, 0.0, 0.0, 0.0]).with_visibility("secret"),
        fact("x", [1.0, 0.0, 0.0, 0.0]).with_review_by(-5),
    ] {
        assert!(matches!(db.remember(bad), Err(MnemoError::Invalid(_))));
    }
    let mut m = fact("x", [1.0, 0.0, 0.0, 0.0]);
    m.metadata
        .insert(MNEMO_META_AUTHOR.into(), json!("someone"));
    assert!(matches!(db.remember(m), Err(MnemoError::Invalid(e)) if e.contains("reserved")));
    let mut m = fact("x", [1.0, 0.0, 0.0, 0.0]);
    m.metadata.insert(MNEMO_META_KIND.into(), json!("task"));
    assert!(matches!(db.remember(m), Err(MnemoError::Invalid(e)) if e.contains("reserved")));
    // `update` sets and removes them, and keeps the user metadata.
    let mut keys = Map::new();
    keys.insert(MNEMO_META_VISIBILITY.into(), json!("public"));
    keys.insert(MNEMO_META_REVIEW_BY.into(), Value::Null);
    let out = db
        .update(
            &id,
            MemoryPatch {
                caller_keys: Some(keys),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(out, WriteOutcome::Updated);
    let m = db.get(&id).unwrap();
    assert_eq!(m.metadata[MNEMO_META_VISIBILITY], json!("public"));
    assert!(!m.metadata.contains_key(MNEMO_META_REVIEW_BY));
    assert_eq!(m.metadata[MNEMO_META_AS_OF], json!(1_700_000_000));
    let mut keys = Map::new();
    keys.insert(MNEMO_META_KIND.into(), json!("task"));
    assert!(db
        .update(
            &id,
            MemoryPatch {
                caller_keys: Some(keys),
                ..Default::default()
            }
        )
        .is_err());
    // A keyed re-write that changes only the visibility is a new version.
    let v = [0.25, 0.5, 0.75, 1.0];
    let (_, o) = db.remember_keyed(fact("same text", v), "k").unwrap();
    assert_eq!(o, WriteOutcome::Added);
    let (_, o) = db.remember_keyed(fact("same text", v), "k").unwrap();
    assert_eq!(o, WriteOutcome::Unchanged);
    let (_, o) = db
        .remember_keyed(fact("same text", v).with_visibility("private"), "k")
        .unwrap();
    assert_eq!(o, WriteOutcome::Superseded);
}

#[test]
fn the_author_is_stamped_on_new_records_only_and_kept_across_shared_reloads() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.mnemo");
    let mut db = Mnemo::create(&p, PW, cfg()).unwrap();
    let before = db.remember(fact("before", [1.0, 0.0, 0.0, 0.0])).unwrap();
    db.set_author(Some("worker-a".into()));
    assert_eq!(db.author(), Some("worker-a"));
    let after = db.remember(fact("after", [0.0, 1.0, 0.0, 0.0])).unwrap();
    assert_eq!(
        db.get(&after).unwrap().metadata[MNEMO_META_AUTHOR],
        json!("worker-a")
    );
    // An edit in place keeps the record's own (none here).
    db.update(
        &before,
        MemoryPatch {
            importance: Some(0.9),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!db
        .get(&before)
        .unwrap()
        .metadata
        .contains_key(MNEMO_META_AUTHOR));
    db.close().unwrap();

    let mut s = Mnemo::open_shared(&p, PW, SharedOptions::default()).unwrap();
    s.set_author(Some("planner-a".into()));
    // Another process's commit makes the shared handle reload.
    let mut other = Mnemo::open(&p, PW).unwrap();
    other.remember(fact("other", [0.0, 0.0, 1.0, 0.0])).unwrap();
    other.close().unwrap();
    let id = s
        .write(|db| db.remember(fact("shared", [0.0, 0.0, 0.0, 1.0])))
        .unwrap();
    let m = s.read(|db| db.get(&id)).unwrap();
    assert_eq!(m.metadata[MNEMO_META_AUTHOR], json!("planner-a"));
}

#[test]
fn the_default_agent_is_set_read_and_cleared_on_the_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("d.mnemo");
    let mut db = Mnemo::create(&p, PW, cfg()).unwrap();
    // No manifest: refused.
    assert!(matches!(
        db.set_default_agent(Some("cursor-agent")),
        Err(MnemoError::Invalid(_))
    ));
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    assert_eq!(db.default_agent().unwrap(), None);
    let manifest = db.about().unwrap()[0].clone();
    assert_eq!(
        db.set_default_agent(Some("cursor-agent")).unwrap(),
        WriteOutcome::Updated
    );
    assert_eq!(
        db.set_default_agent(Some("cursor-agent")).unwrap(),
        WriteOutcome::Unchanged
    );
    assert!(matches!(
        db.set_default_agent(Some("no spaces")),
        Err(MnemoError::Invalid(_))
    ));
    db.close().unwrap();
    let mut db = Mnemo::open(&p, PW).unwrap();
    assert_eq!(db.default_agent().unwrap().as_deref(), Some("cursor-agent"));
    // The manifest kept its id, content and the rest of its metadata.
    let now = db.about().unwrap()[0].clone();
    assert_eq!(now.id, manifest.id);
    assert_eq!(now.content, manifest.content);
    assert_eq!(now.metadata["topic"], manifest.metadata["topic"]);
    assert_eq!(db.set_default_agent(None).unwrap(), WriteOutcome::Updated);
    assert_eq!(db.default_agent().unwrap(), None);
}
