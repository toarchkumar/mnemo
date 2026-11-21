//! v0.20.2 (G8): stale knowledge. `Mnemo::supersede` retires an unkeyed
//! record in favour of a newer one, so recall and `about` leave it out as
//! they leave out an old keyed version; `Mnemo::stale_unkeyed` lists the
//! old unkeyed episodic facts to review.

use mnemo_engine::{
    EssenceScope, EssenceSection, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, RecallRequest,
    Ulid, WriteOutcome, MNEMO_META_INVALID_AT, MNEMO_META_SUPERSEDED_BY, UNKEYED_STALE_SECS,
};

const PW: &str = "stale-knowledge-test-passphrase";

fn create(dir: &tempfile::TempDir) -> Mnemo {
    let cfg = MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    Mnemo::create(dir.path().join("s.mnemo"), PW, cfg).unwrap()
}

fn fact(db: &mut Mnemo, text: &str, t: MemoryType, v: f32) -> Ulid {
    db.remember(Memory::new(text, t, vec![v, 1.0, 0.0, 0.0]))
        .unwrap()
}

fn recalled(db: &mut Mnemo) -> Vec<String> {
    let req = RecallRequest::new(vec![1.0, 1.0, 0.0, 0.0])
        .top_k(50)
        .track_access(false);
    db.recall(&req)
        .unwrap()
        .into_iter()
        .map(|h| h.memory.content)
        .collect()
}

#[test]
fn supersede_retires_an_unkeyed_record_and_refuses_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = create(&dir);
    let old = fact(&mut db, "roadmap: sync first", MemoryType::Episodic, 1.0);
    let new = fact(&mut db, "roadmap: perf first", MemoryType::Episodic, 2.0);
    let other = fact(&mut db, "an unrelated note", MemoryType::Semantic, 3.0);
    let (keyed, _) = db
        .remember_keyed(
            Memory::new(
                "order: perf, then sync",
                MemoryType::Episodic,
                vec![4.0, 1.0, 0.0, 0.0],
            ),
            "decision.order",
        )
        .unwrap();
    let item = db
        .essence_add(EssenceScope::Project, EssenceSection::Next, "ship", None)
        .unwrap();
    db.flush().unwrap();
    assert!(recalled(&mut db).contains(&"roadmap: sync first".to_string()));

    assert_eq!(db.supersede(&old, &new).unwrap(), WriteOutcome::Superseded);
    assert!(!recalled(&mut db).contains(&"roadmap: sync first".to_string()));
    let m = db.get(&old).unwrap();
    assert_eq!(m.metadata[MNEMO_META_SUPERSEDED_BY], new.to_string());
    assert!(m.metadata[MNEMO_META_INVALID_AT].is_i64());
    // The same again changes nothing; `by` is untouched.
    assert_eq!(db.supersede(&old, &new).unwrap(), WriteOutcome::Unchanged);
    assert!(!db
        .get(&new)
        .unwrap()
        .metadata
        .contains_key(MNEMO_META_SUPERSEDED_BY));

    let refused = |db: &mut Mnemo, a: &Ulid, b: &Ulid, says: &str| {
        let e = db.supersede(a, b).unwrap_err().to_string();
        assert!(e.contains(says), "{e}");
    };
    refused(&mut db, &old, &other, "already superseded by");
    refused(&mut db, &other, &old, "superseded itself");
    refused(&mut db, &keyed, &new, "it has a key (decision.order)");
    refused(&mut db, &item, &new, "engine record");
    refused(&mut db, &other, &item, "engine record");
    refused(&mut db, &other, &other, "can't supersede itself");
    refused(&mut db, &other, &Ulid::new(), "no such memory");
    // A keyed record may be the newer one.
    assert_eq!(
        db.supersede(&other, &keyed).unwrap(),
        WriteOutcome::Superseded
    );
    db.close().unwrap();

    // It lasts: a fresh open leaves both out of recall.
    let mut db = Mnemo::open(dir.path().join("s.mnemo"), PW).unwrap();
    let hits = recalled(&mut db);
    assert!(
        !hits.contains(&"roadmap: sync first".to_string()),
        "{hits:?}"
    );
    assert!(!hits.contains(&"an unrelated note".to_string()), "{hits:?}");
    assert!(
        hits.contains(&"roadmap: perf first".to_string()),
        "{hits:?}"
    );
    let read_only = {
        db.close().unwrap();
        let mut ro = Mnemo::open_read_only(dir.path().join("s.mnemo"), PW).unwrap();
        ro.supersede(&new, &keyed).unwrap_err()
    };
    assert!(
        matches!(read_only, mnemo_engine::MnemoError::ReadOnly),
        "{read_only}"
    );
}

#[test]
fn stale_unkeyed_lists_old_unkeyed_episodic_facts_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = create(&dir);
    let decision = fact(
        &mut db,
        "2026-05-26: chose page size 4096",
        MemoryType::Episodic,
        1.0,
    );
    let replaced = fact(
        &mut db,
        "2026-05-26: chose a 64-page WAL",
        MemoryType::Episodic,
        2.0,
    );
    let newer = fact(
        &mut db,
        "2026-06-01: the WAL starts at 8 pages",
        MemoryType::Episodic,
        3.0,
    );
    fact(
        &mut db,
        "the server binds to 8443",
        MemoryType::Semantic,
        4.0,
    );
    db.remember_keyed(
        Memory::new(
            "release order: tag, then publish",
            MemoryType::Episodic,
            vec![5.0, 1.0, 0.0, 0.0],
        ),
        "process.release-order",
    )
    .unwrap();
    db.essence_add(
        EssenceScope::Project,
        EssenceSection::Decisions,
        "use 4096",
        None,
    )
    .unwrap();
    db.supersede(&replaced, &newer).unwrap();
    // Nothing is old yet.
    assert!(db.stale_unkeyed().unwrap().is_empty());
    // A month and a day on: the two live unkeyed episodic facts, not the
    // superseded one, the semantic one, the keyed one or the essence item.
    db.__set_clock_offset_for_testing(UNKEYED_STALE_SECS + 86_400);
    let rows = db.stale_unkeyed().unwrap();
    let ids: Vec<Ulid> = rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![decision, newer], "{rows:?}");
    assert!(rows.iter().all(|r| r.what == "unkeyed episodic"));
    let m = db.get(&decision).unwrap();
    assert_eq!(rows[0].since, m.created_at + UNKEYED_STALE_SECS);
    assert_eq!(rows[0].text, "2026-05-26: chose page size 4096");
    // `stale()` (what `about` and the brief count) is unchanged.
    assert!(db
        .stale()
        .unwrap()
        .iter()
        .all(|r| r.what != "unkeyed episodic"));
}
