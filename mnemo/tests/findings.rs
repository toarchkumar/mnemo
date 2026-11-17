//! v0.18.0 (13a step 8b): review findings as records: the id rules, every
//! allowed transition and a refused one, the list's filters and order, and
//! kept out of recall.

use mnemo_engine::{
    finding_status, FindingFilter, FindingSpec, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig,
    RecallRequest, FINDING_TRANSITIONS,
};

const PW: &str = "findings-test-passphrase";

fn db(dir: &tempfile::TempDir) -> Mnemo {
    let cfg = MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    let mut db = Mnemo::create(dir.path().join("f.mnemo"), PW, cfg).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.remember(Memory::new(
        "the release order keeps the tag after the dry run",
        MemoryType::Semantic,
        vec![1.0, 0.0, 0.0, 0.0],
    ))
    .unwrap();
    db
}

fn spec(id: &str, category: &str, priority: &str, title: &str) -> FindingSpec {
    FindingSpec {
        id: id.into(),
        category: category.into(),
        priority: priority.into(),
        title: title.into(),
        body: "Where it is and why it matters.".into(),
        refs: vec!["src/store/coordination.rs:206".into()],
    }
}

#[test]
fn a_finding_is_added_once_with_its_fields() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir);
    db.set_author(Some("reviewer-b".into()));
    let f = db
        .finding_add(&spec("RB-014.1", "CORRECTNESS", "HIGH", "The release race"))
        .unwrap();
    assert_eq!(
        (
            f.id.as_str(),
            f.batch.as_str(),
            f.status.as_str(),
            f.author.as_deref()
        ),
        ("RB-014.1", "RB-014", "open", Some("reviewer-b"))
    );
    assert_eq!(f.title, "The release race");
    assert_eq!(f.body, "Where it is and why it matters.");
    assert_eq!(f.refs, ["src/store/coordination.rs:206"]);
    // The id is unique.
    let e = db
        .finding_add(&spec("RB-014.1", "PERF", "LOW", "Again"))
        .unwrap_err()
        .to_string();
    assert!(e.contains("RB-014.1 is taken"), "{e}");
    // The rules.
    for (s, want) in [
        (spec("rb-014.2", "PERF", "LOW", "t"), "isn't a finding's id"),
        (spec("RB-014", "PERF", "LOW", "t"), "isn't a finding's id"),
        (spec("RB-014.2", "SPEED", "LOW", "t"), "isn't a category"),
        (spec("RB-014.2", "PERF", "URGENT", "t"), "isn't a priority"),
        (spec("RB-014.2", "PERF", "LOW", ""), "title"),
        (spec("RB-014.2", "PERF", "LOW", "two\nlines"), "title"),
        (spec("RB-014.2", "PERF", "LOW", &"x".repeat(201)), "title"),
    ] {
        let e = db.finding_add(&s).unwrap_err().to_string();
        assert!(e.contains(want), "{s:?}: {e}");
    }
    let mut big = spec("RB-014.2", "PERF", "LOW", "t");
    big.body = "x".repeat(16 * 1024 + 1);
    assert!(db.finding_add(&big).is_err());
}

#[test]
fn every_allowed_transition_and_a_refused_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir);
    db.set_author(Some("reviewer-b".into()));
    // One finding per (from, to): walked from `open` to `from`, then to `to`.
    let path_to = |from: &str| -> Vec<&'static str> {
        match from {
            "open" => vec![],
            "accepted" => vec!["accepted"],
            "deferred" => vec!["deferred"],
            "needs-user" => vec!["needs-user"],
            other => panic!("{other}"),
        }
    };
    for (i, (from, to)) in FINDING_TRANSITIONS.iter().enumerate() {
        let id = format!("RB-020.{}", i + 1);
        db.set_author(Some("reviewer-b".into()));
        db.finding_add(&spec(&id, "PERF", "LOW", "t")).unwrap();
        db.set_author(Some("planner-a".into()));
        for step in path_to(from) {
            db.finding_triage(&id, step, "on the way").unwrap();
        }
        let f = if *to == finding_status::VERIFIED {
            db.set_author(Some("reviewer-b".into()));
            db.finding_verify(&id, "abc1234").unwrap()
        } else {
            db.finding_triage(&id, to, "because").unwrap()
        };
        assert_eq!(f.status, *to, "{from} -> {to}");
        let last = f.history.last().unwrap();
        assert_eq!(
            (last["from"].as_str(), last["to"].as_str()),
            (Some(*from), Some(*to))
        );
        if *to == finding_status::VERIFIED {
            assert_eq!(
                (f.verified_ref.as_deref(), f.verified_by.as_deref()),
                (Some("abc1234"), Some("reviewer-b"))
            );
        } else {
            assert_eq!(
                (f.triage.as_deref(), f.triaged_by.as_deref()),
                (Some("because"), Some("planner-a"))
            );
        }
    }
    // Refused: `open` to `verified`, `rejected` onward, `accepted` back to
    // `deferred`, an unknown status, no text, no ref.
    db.set_author(Some("reviewer-b".into()));
    db.finding_add(&spec("RB-021.1", "PERF", "LOW", "t"))
        .unwrap();
    let e = db
        .finding_verify("RB-021.1", "abc")
        .unwrap_err()
        .to_string();
    assert!(e.contains("can't go from open to verified"), "{e}");
    db.finding_triage("RB-021.1", "rejected", "not ours")
        .unwrap();
    for to in ["accepted", "deferred", "needs-user"] {
        let e = db
            .finding_triage("RB-021.1", to, "x")
            .unwrap_err()
            .to_string();
        assert!(e.contains("can't go from rejected"), "{to}: {e}");
    }
    db.finding_add(&spec("RB-021.2", "PERF", "LOW", "t"))
        .unwrap();
    db.finding_triage("RB-021.2", "accepted", "yes").unwrap();
    assert!(db.finding_triage("RB-021.2", "deferred", "x").is_err());
    assert!(db.finding_triage("RB-021.2", "nonsense", "x").is_err());
    assert!(db.finding_triage("RB-021.2", "verified", "x").is_err());
    db.finding_add(&spec("RB-021.3", "PERF", "LOW", "t"))
        .unwrap();
    assert!(db.finding_triage("RB-021.3", "accepted", "  ").is_err());
    db.finding_triage("RB-021.3", "accepted", "yes").unwrap();
    assert!(db.finding_verify("RB-021.3", "").is_err());
    assert!(db.finding_triage("RB-404.1", "accepted", "x").is_err());
}

#[test]
fn the_list_filters_and_orders_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir);
    db.set_author(Some("reviewer-b".into()));
    for (id, c, p) in [
        ("RB-10.1", "PERF", "LOW"),
        ("RB-9.2", "SECURITY", "HIGH"),
        ("RB-9.10", "PERF", "MEDIUM"),
        ("R-011.1", "PRODUCT", "IDEA"),
    ] {
        db.finding_add(&spec(id, c, p, id)).unwrap();
    }
    db.finding_triage("RB-9.2", "accepted", "yes").unwrap();
    let ids = |f: &FindingFilter, db: &mut Mnemo| -> Vec<String> {
        db.findings(f).unwrap().into_iter().map(|f| f.id).collect()
    };
    assert_eq!(
        ids(&FindingFilter::default(), &mut db),
        ["R-011.1", "RB-9.2", "RB-9.10", "RB-10.1"]
    );
    let f = FindingFilter {
        status: Some("open".into()),
        ..Default::default()
    };
    assert_eq!(ids(&f, &mut db), ["R-011.1", "RB-9.10", "RB-10.1"]);
    let f = FindingFilter {
        batch: Some("RB-9".into()),
        category: Some("PERF".into()),
        ..Default::default()
    };
    assert_eq!(ids(&f, &mut db), ["RB-9.10"]);
    let f = FindingFilter {
        priority: Some("IDEA".into()),
        ..Default::default()
    };
    assert_eq!(ids(&f, &mut db), ["R-011.1"]);
    assert_eq!(db.finding("RB-9.2").unwrap().unwrap().status, "accepted");
    assert!(db.finding("RB-1.1").unwrap().is_none());
}

#[test]
fn a_finding_is_never_a_recall_hit() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir);
    db.set_author(Some("reviewer-b".into()));
    db.finding_add(&spec(
        "RB-014.1",
        "CORRECTNESS",
        "HIGH",
        "the release order keeps the tag after the dry run",
    ))
    .unwrap();
    db.flush().unwrap();
    for req in [
        RecallRequest::new(vec![1.0, 0.0, 0.0, 0.0]).top_k(10),
        RecallRequest::new(vec![0.5, 0.5, 0.5, 0.5]).top_k(10),
        RecallRequest::keywords_only("release order dry run").top_k(10),
    ] {
        let hits = db.recall(&req.track_access(false)).unwrap();
        assert!(!hits.is_empty());
        assert!(
            hits.iter()
                .all(|h| !mnemo_engine::is_coordination(&h.memory)),
            "{hits:?}"
        );
    }
    assert!(db
        .about()
        .unwrap()
        .iter()
        .all(|m| !mnemo_engine::is_coordination(m)));
    assert_eq!(db.kind_counts().coordination, 1);
}

/// Review round 1 (RB-014): a finding needs its author, and its id is
/// unique by prefix and numbers (leading zeros stay allowed).
#[test]
fn a_finding_needs_an_author_and_an_id_free_by_its_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir);
    let e = db
        .finding_add(&spec("RB-014.1", "PERF", "LOW", "t"))
        .unwrap_err()
        .to_string();
    assert!(e.contains("needs its author"), "{e}");
    db.set_author(Some("  ".into()));
    assert!(db
        .finding_add(&spec("RB-014.1", "PERF", "LOW", "t"))
        .is_err());
    db.set_author(Some("reviewer-b".into()));
    db.finding_add(&spec("RB-900.1", "PERF", "LOW", "t"))
        .unwrap();
    let e = db
        .finding_add(&spec("RB-0900.1", "PERF", "LOW", "t"))
        .unwrap_err()
        .to_string();
    assert!(e.contains("RB-0900.1 is taken (as RB-900.1)"), "{e}");
    // Leading zeros are fine on a free id.
    db.finding_add(&spec("RB-014.1", "PERF", "LOW", "t"))
        .unwrap();
    assert!(db
        .finding_add(&spec("RB-14.1", "PERF", "LOW", "t"))
        .is_err());
    db.finding_add(&spec("RB-014.2", "PERF", "LOW", "t"))
        .unwrap();
}

/// Review round 1 (RB-014): a `SECURITY` finding is `private` until it's
/// verified, and it's verified only against a release tag. The finding
/// reads still see it; other categories keep any ref.
#[test]
fn a_security_finding_is_private_until_verified_against_a_tag() {
    use mnemo_engine::visibility_of;
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir);
    db.set_author(Some("reviewer-b".into()));
    let f = db
        .finding_add(&spec("RB-014.1", "SECURITY", "MEDIUM", "t"))
        .unwrap();
    let other = db
        .finding_add(&spec("RB-014.2", "CORRECTNESS", "LOW", "t"))
        .unwrap();
    assert_eq!(visibility_of(&db.get(&f.record_id).unwrap()), "private");
    assert_eq!(visibility_of(&db.get(&other.record_id).unwrap()), "team");
    // Every finding read sees it.
    assert_eq!(db.finding("RB-014.1").unwrap().unwrap().id, "RB-014.1");
    let f2 = mnemo_engine::FindingFilter {
        category: Some("SECURITY".into()),
        ..Default::default()
    };
    assert_eq!(db.findings(&f2).unwrap().len(), 1);
    db.set_author(Some("planner-a".into()));
    db.finding_triage("RB-014.1", "accepted", "fix it").unwrap();
    db.finding_triage("RB-014.2", "accepted", "fix it").unwrap();
    assert_eq!(visibility_of(&db.get(&f.record_id).unwrap()), "private");
    db.set_author(Some("reviewer-b".into()));
    for r in ["abc1234", "v0.18", "v0.18.0-rc1", "#128", "release-0.18.0"] {
        let e = db.finding_verify("RB-014.1", r).unwrap_err().to_string();
        assert!(e.contains("published release tag"), "{r}: {e}");
    }
    assert_eq!(db.finding("RB-014.1").unwrap().unwrap().status, "accepted");
    let v = db.finding_verify("RB-014.1", "v0.18.0").unwrap();
    assert_eq!(
        (v.status.as_str(), v.verified_ref.as_deref()),
        ("verified", Some("v0.18.0"))
    );
    assert_eq!(visibility_of(&db.get(&f.record_id).unwrap()), "team");
    // Another category takes any ref, and stays team.
    db.finding_verify("RB-014.2", "abc1234").unwrap();
    assert_eq!(visibility_of(&db.get(&other.record_id).unwrap()), "team");
    // It survives a reopen.
    db.close().unwrap();
    let mut db = Mnemo::open(dir.path().join("f.mnemo"), PW).unwrap();
    assert_eq!(visibility_of(&db.get(&f.record_id).unwrap()), "team");
}

/// Review round 1 (RB-014): `update` (which `update_text` calls), `delete`
/// and `essence_remove` refuse a finding's record; `essence_remove` refuses any
/// record that isn't an essence item.
#[test]
fn a_finding_changes_only_by_triage_and_verification() {
    use mnemo_engine::{EssenceScope, EssenceSection, MemoryPatch};
    let dir = tempfile::tempdir().unwrap();
    let mut db = db(&dir);
    db.set_author(Some("reviewer-b".into()));
    let f = db
        .finding_add(&spec("RB-014.1", "SECURITY", "MEDIUM", "t"))
        .unwrap();
    let id = f.record_id;
    let e = db
        .update(
            &id,
            MemoryPatch {
                content: Some("other".into()),
                vector: Some(vec![1.0, 0.0, 0.0, 0.0]),
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
    assert!(e.contains("can't be updated"), "{e}");
    let mut keys = serde_json::Map::new();
    keys.insert("_mnemo.visibility".into(), "team".into());
    assert!(db
        .update(
            &id,
            MemoryPatch {
                caller_keys: Some(keys),
                ..Default::default()
            },
        )
        .is_err());
    let e = db.delete(&id).unwrap_err().to_string();
    assert!(
        e.contains("can't be deleted") && e.contains("rejected"),
        "{e}"
    );
    let e = db.essence_remove(&id).unwrap_err().to_string();
    assert!(e.contains("not an essence record"), "{e}");
    // Unchanged.
    let after = db.finding("RB-014.1").unwrap().unwrap();
    assert_eq!((after.title.as_str(), after.status.as_str()), ("t", "open"));
    assert_eq!(
        mnemo_engine::visibility_of(&db.get(&id).unwrap()),
        "private"
    );
    // `essence_remove` refuses a plain fact too, and still removes an item.
    let fact = db
        .remember(Memory::new(
            "a plain fact",
            MemoryType::Semantic,
            vec![0.0, 1.0, 0.0, 0.0],
        ))
        .unwrap();
    assert!(db.essence_remove(&fact).is_err());
    assert!(db.get(&fact).is_ok());
    let item = db
        .essence_add(EssenceScope::Project, EssenceSection::Open, "an item", None)
        .unwrap();
    db.essence_remove(&item).unwrap();
    assert!(db.get(&item).is_err());
    // `delete` still deletes everything else.
    db.delete(&fact).unwrap();
    assert!(db.get(&fact).is_err());
}
