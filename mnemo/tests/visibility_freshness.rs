//! 13a step 7 (v0.17): visibility (13.5) and freshness (13.6). Private
//! records are left out of recall (every path), `about`, the inbox and the
//! essence render unless asked; `stale()` and its marks; ABOUT-COUNT.

use mnemo_engine::{
    EssenceScope, EssenceSection, KdfParams, Memory, MemoryPatch, MemoryType, Mnemo, MnemoConfig,
    RecallRequest, VisibilitySet, MNEMO_META_AS_OF, MNEMO_META_REVIEW_BY, MNEMO_META_VISIBILITY,
};
use serde_json::{json, Map};

const PW: &str = "visibility-test-passphrase";

fn db(dir: &tempfile::TempDir) -> Mnemo {
    let cfg = MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    Mnemo::create(dir.path().join("v.mnemo"), PW, cfg).unwrap()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn set_keys(db: &mut Mnemo, id: &mnemo_engine::Ulid, keys: &[(&str, serde_json::Value)]) {
    let mut m = Map::new();
    for (k, v) in keys {
        m.insert((*k).into(), v.clone());
    }
    db.update(
        id,
        MemoryPatch {
            caller_keys: Some(m),
            ..Default::default()
        },
    )
    .unwrap();
}

#[test]
fn private_records_are_left_out_of_recall_unless_asked() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir);
    let v = [1.0, 0.0, 0.0, 0.0];
    let team = db
        .remember(Memory::new(
            "deploy target is staging",
            MemoryType::Semantic,
            v.to_vec(),
        ))
        .unwrap();
    let public = db
        .remember(
            Memory::new(
                "deploy target is staging (public)",
                MemoryType::Semantic,
                v.to_vec(),
            )
            .with_visibility("public"),
        )
        .unwrap();
    let private = db
        .remember(
            Memory::new(
                "deploy target is staging (private)",
                MemoryType::Semantic,
                v.to_vec(),
            )
            .with_visibility("private"),
        )
        .unwrap();
    // Many other facts, so recall has a real shortlist.
    for i in 0..60 {
        db.remember(Memory::new(
            format!("filler {i}"),
            MemoryType::Semantic,
            vec![0.0, 1.0, (i as f32) / 60.0, 0.0],
        ))
        .unwrap();
    }
    let ids = |db: &mut Mnemo, req: RecallRequest| -> Vec<mnemo_engine::Ulid> {
        db.recall(&req.track_access(false))
            .unwrap()
            .into_iter()
            .map(|h| h.memory.id)
            .collect()
    };
    for pass in 0..3 {
        // Vector, keywords, and (pass 2) through the ANN index.
        let hits = ids(&mut db, RecallRequest::new(v.to_vec()).top_k(3));
        assert!(
            hits.contains(&team) && hits.contains(&public) && !hits.contains(&private),
            "pass {pass}: {hits:?}"
        );
        let hits = ids(&mut db, RecallRequest::keywords_only("staging").top_k(5));
        assert!(!hits.contains(&private), "pass {pass}");
        let all = ids(
            &mut db,
            RecallRequest::new(v.to_vec())
                .top_k(3)
                .visibility(VisibilitySet::ALL),
        );
        assert!(all.contains(&private), "pass {pass}: {all:?}");
        let only = ids(
            &mut db,
            RecallRequest::new(v.to_vec())
                .top_k(3)
                .visibility(VisibilitySet::parse(&["private"]).unwrap()),
        );
        assert_eq!(only, [private], "pass {pass}");
        match pass {
            0 => {
                let p = dir.path().join("v.mnemo");
                db.close().unwrap();
                db = Mnemo::open(&p, PW).unwrap();
            }
            1 => {
                db.build_index().unwrap();
            }
            _ => {}
        }
    }
    // An edit to private takes it out; back to team brings it back.
    set_keys(&mut db, &team, &[(MNEMO_META_VISIBILITY, json!("private"))]);
    assert!(!ids(&mut db, RecallRequest::new(v.to_vec()).top_k(3)).contains(&team));
    set_keys(&mut db, &team, &[(MNEMO_META_VISIBILITY, json!("team"))]);
    assert!(ids(&mut db, RecallRequest::new(v.to_vec()).top_k(3)).contains(&team));
    assert!(VisibilitySet::parse(&["secret"]).is_err());
    assert_eq!(
        VisibilitySet::parse::<&str>(&[]).unwrap(),
        VisibilitySet::DEFAULT
    );
}

#[test]
fn about_the_inbox_and_the_essence_render_leave_private_out() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir);
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    let mut onb = Memory::new(
        "a private onboarding note",
        MemoryType::Semantic,
        vec![1.0, 0.0, 0.0, 0.0],
    )
    .with_visibility("private");
    onb.metadata.insert("area".into(), json!("onboarding"));
    db.remember(onb).unwrap();
    assert_eq!(db.about().unwrap().len(), 1);
    assert_eq!(db.about_in(VisibilitySet::ALL).unwrap().len(), 2);
    // A private manifest is still the manifest the engine uses.
    let manifest = db.about().unwrap()[0].id;
    set_keys(
        &mut db,
        &manifest,
        &[(MNEMO_META_VISIBILITY, json!("private"))],
    );
    db.set_default_agent(Some("cursor-agent")).unwrap();
    assert_eq!(db.default_agent().unwrap().as_deref(), Some("cursor-agent"));
    // The inbox.
    let m = db
        .post("worker-a", "hidden", "", &[], None, Some("planner-a"), None)
        .unwrap();
    set_keys(&mut db, &m.id, &[(MNEMO_META_VISIBILITY, json!("private"))]);
    db.post("worker-a", "shown", "", &[], None, Some("planner-a"), None)
        .unwrap();
    let subjects =
        |v: Vec<mnemo_engine::Message>| v.into_iter().map(|m| m.subject).collect::<Vec<_>>();
    assert_eq!(
        subjects(db.inbox(Some("worker-a"), None, true).unwrap()),
        ["shown"]
    );
    assert_eq!(
        subjects(
            db.inbox_in(Some("worker-a"), None, true, VisibilitySet::ALL)
                .unwrap()
        ),
        ["hidden", "shown"]
    );
    // The essence render.
    db.essence_add(
        EssenceScope::Project,
        EssenceSection::State,
        "a shown state",
        None,
    )
    .unwrap();
    let hid = db
        .essence_add(
            EssenceScope::Project,
            EssenceSection::Open,
            "a private open item",
            None,
        )
        .unwrap();
    set_keys(&mut db, &hid, &[(MNEMO_META_VISIBILITY, json!("private"))]);
    let r = db.essence(EssenceScope::Project, 0).unwrap();
    assert!(
        r.contains("a shown state") && !r.contains("a private open item"),
        "{r}"
    );
}

#[test]
fn stale_records_by_kind_and_review_date() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir);
    let n = now();
    let day = 86_400;
    let fact = |db: &mut Mnemo, text: &str| {
        db.remember(Memory::new(
            text,
            MemoryType::Semantic,
            vec![1.0, 0.0, 0.0, 0.0],
        ))
        .unwrap()
    };
    // A fact is never stale on its own; with a past review date it is; with
    // a future one it isn't.
    fact(&mut db, "plain fact");
    let reviewed = fact(&mut db, "reviewed fact");
    set_keys(
        &mut db,
        &reviewed,
        &[(MNEMO_META_REVIEW_BY, json!(n - day))],
    );
    let later = fact(&mut db, "later fact");
    set_keys(
        &mut db,
        &later,
        &[(MNEMO_META_REVIEW_BY, json!(n + 30 * day))],
    );
    // Essence: a state item true as of 20 days ago (stale after 14); an
    // open item as of 20 days ago (not yet: 30); a resolved one never.
    let st = db
        .essence_add(
            EssenceScope::Project,
            EssenceSection::State,
            "old state",
            None,
        )
        .unwrap();
    set_keys(&mut db, &st, &[(MNEMO_META_AS_OF, json!(n - 20 * day))]);
    let op = db
        .essence_add(
            EssenceScope::Project,
            EssenceSection::Open,
            "recent-ish open",
            None,
        )
        .unwrap();
    set_keys(&mut db, &op, &[(MNEMO_META_AS_OF, json!(n - 20 * day))]);
    let done = db
        .essence_add(
            EssenceScope::Project,
            EssenceSection::Next,
            "done next",
            None,
        )
        .unwrap();
    set_keys(&mut db, &done, &[(MNEMO_META_AS_OF, json!(n - 20 * day))]);
    db.essence_resolve(&done).unwrap();
    // The render marks the stale state item.
    let r = db.essence(EssenceScope::Project, 0).unwrap();
    assert!(r.contains("old state [stale since"), "{r}");
    assert!(!r.contains("recent-ish open [stale"), "{r}");
    // A task and a message, then four days on.
    db.set_author(Some("planner-a".into()));
    db.task_add(&mnemo_engine::TaskSpec {
        title: "Step 7".into(),
        role: Some("worker".into()),
        ..Default::default()
    })
    .unwrap();
    db.post("worker-a", "unanswered", "", &[], None, None, None)
        .unwrap();
    db.set_author(Some("worker-a".into()));
    db.task_next(Some("worker")).unwrap().unwrap();
    let what = |db: &mut Mnemo| {
        let mut v: Vec<(String, String)> = db
            .stale()
            .unwrap()
            .into_iter()
            .map(|s| (s.what, s.text))
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        what(&mut db),
        [
            ("essence state".to_string(), "old state".to_string()),
            ("fact".into(), "reviewed fact".into())
        ]
    );
    db.__set_clock_offset_for_testing(4 * day);
    let w = what(&mut db);
    assert!(
        w.contains(&("task active".into(), "Step 7".into())),
        "{w:?}"
    );
    assert!(
        w.contains(&("message unread".into(), "unanswered".into())),
        "{w:?}"
    );
    let mine: Vec<_> = db
        .stale()
        .unwrap()
        .into_iter()
        .filter(|s| s.whose.as_deref() == Some("worker-a"))
        .collect();
    assert_eq!(mine.len(), 2, "its task and its message: {mine:?}");
    // An update renews the task.
    db.task_update("step-7", None, Some("still going"), None)
        .unwrap();
    assert!(!what(&mut db).iter().any(|(k, _)| k == "task active"));
    // ABOUT-COUNT.
    let c = db.kind_counts();
    // Three facts; three essence items; the task, the message and the
    // task's claim (nothing here touched presence).
    assert_eq!((c.facts, c.essence, c.coordination), (3, 3, 3), "{c:?}");
}
