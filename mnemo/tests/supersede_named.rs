//! 12.3 step 5b (v0.15): a keyed write that supersedes a named memory
//! (`Mnemo::remember_keyed_superseding`): the old, unkeyed record becomes the
//! key's first version and is superseded.

use mnemo_engine::{
    EssenceScope, EssenceSection, KdfParams, KeyScope, Memory, MemoryType, Mnemo, MnemoConfig,
    MnemoError, RecallRequest, Scope, Ulid, WriteOutcome, MNEMO_META_INVALID_AT, MNEMO_META_KEY,
    MNEMO_META_SUPERSEDED_BY, MNEMO_META_SUPERSEDES,
};
use tempfile::tempdir;

const PW: &str = "supersede-test-passphrase";

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn mem(text: &str, i: f32) -> Memory {
    Memory::new(
        text.to_string(),
        MemoryType::Semantic,
        vec![1.0, i, 0.5, 0.0],
    )
}

fn recall_texts(db: &mut Mnemo, keywords: &str) -> Vec<String> {
    db.recall(&RecallRequest::keywords_only(keywords).track_access(false))
        .unwrap()
        .into_iter()
        .map(|h| h.memory.content)
        .collect()
}

fn history_texts(db: &mut Mnemo, key: &str) -> Vec<String> {
    db.history(key, None)
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect()
}

fn invalid(r: mnemo_engine::Result<(Ulid, WriteOutcome)>, needle: &str) {
    match r {
        Err(MnemoError::Invalid(msg)) => assert!(msg.contains(needle), "{msg}"),
        other => panic!("expected Invalid ({needle}), got {other:?}"),
    }
}

#[test]
fn an_unkeyed_memory_becomes_the_keys_first_version() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("s.mnemo");
    let mut db = Mnemo::create(&p, PW, cfg()).unwrap();
    let old = db.remember(mem("Rust MSRV is 1.75", 1.0)).unwrap();
    let (new, outcome) = db
        .remember_keyed_superseding(mem("The MSRV is 1.92", 2.0), "fact.msrv", &old)
        .unwrap();
    assert_eq!(outcome, WriteOutcome::Superseded);
    assert_ne!(new, old);
    // The old one is the key's oldest version, out of recall.
    assert_eq!(
        history_texts(&mut db, "fact.msrv"),
        ["The MSRV is 1.92", "Rust MSRV is 1.75"]
    );
    assert_eq!(recall_texts(&mut db, "MSRV"), ["The MSRV is 1.92"]);
    let o = db.get(&old).unwrap();
    assert_eq!(o.metadata[MNEMO_META_KEY], "fact.msrv");
    assert_eq!(o.metadata[MNEMO_META_SUPERSEDED_BY], new.to_string());
    assert!(o.metadata.contains_key(MNEMO_META_INVALID_AT));
    assert_eq!(
        db.get(&new).unwrap().metadata[MNEMO_META_SUPERSEDES],
        old.to_string()
    );
    // A later keyed write supersedes the new record, not the adopted one.
    let (third, outcome) = db
        .remember_keyed(mem("The MSRV is 1.93", 3.0), "fact.msrv")
        .unwrap();
    assert_eq!(outcome, WriteOutcome::Superseded);
    assert_eq!(
        history_texts(&mut db, "fact.msrv"),
        ["The MSRV is 1.93", "The MSRV is 1.92", "Rust MSRV is 1.75"]
    );
    assert_eq!(
        db.get(&new).unwrap().metadata[MNEMO_META_SUPERSEDED_BY],
        third.to_string()
    );
}

#[test]
fn matching_content_adopts_the_old_record_as_is() {
    let dir = tempdir().unwrap();
    let mut db = Mnemo::create(dir.path().join("s.mnemo"), PW, cfg()).unwrap();
    let old = db.remember(mem("v0.14.0 released", 1.0)).unwrap();
    let (id, outcome) = db
        .remember_keyed_superseding(mem("v0.14.0 released", 1.0), "fact.release", &old)
        .unwrap();
    assert_eq!((id, outcome), (old, WriteOutcome::Updated));
    assert_eq!(history_texts(&mut db, "fact.release"), ["v0.14.0 released"]);
    assert_eq!(recall_texts(&mut db, "released"), ["v0.14.0 released"]);
    let o = db.get(&old).unwrap();
    assert_eq!(o.metadata[MNEMO_META_KEY], "fact.release");
    assert!(!o.metadata.contains_key(MNEMO_META_SUPERSEDED_BY));
    assert_eq!(db.len(), 1);
}

#[test]
fn the_refusals() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("s.mnemo");
    let mut db = Mnemo::create(&p, PW, cfg()).unwrap();
    let nobody = Ulid::new();
    invalid(
        db.remember_keyed_superseding(mem("x", 1.0), "k1", &nobody),
        "no such memory",
    );
    let gone = db.remember(mem("gone", 1.0)).unwrap();
    db.delete(&gone).unwrap();
    invalid(
        db.remember_keyed_superseding(mem("x", 1.0), "k1", &gone),
        "deleted",
    );
    // Already superseded.
    let a = db.remember(mem("a", 1.0)).unwrap();
    db.remember_keyed_superseding(mem("a2", 2.0), "k2", &a)
        .unwrap();
    invalid(
        db.remember_keyed_superseding(mem("a3", 3.0), "k3", &a),
        "already superseded",
    );
    // Already keyed.
    let (keyed, _) = db.remember_keyed(mem("keyed", 1.0), "k4").unwrap();
    invalid(
        db.remember_keyed_superseding(mem("x", 1.0), "k5", &keyed),
        "already has a key",
    );
    // The slot already has a live record.
    let b = db.remember(mem("b", 1.0)).unwrap();
    invalid(
        db.remember_keyed_superseding(mem("x", 1.0), "k4", &b),
        "already has a live record",
    );
    // Engine records: essence, and the manifest.
    let e = db
        .essence_add(
            EssenceScope::Project,
            EssenceSection::Decisions,
            "a decision",
            None,
        )
        .unwrap();
    invalid(
        db.remember_keyed_superseding(mem("x", 1.0), "k6", &e),
        "engine record",
    );
    let manifest = db.remember(Memory::scaffold_manifest(4)).unwrap();
    invalid(
        db.remember_keyed_superseding(mem("x", 1.0), "k6", &manifest),
        "engine record",
    );
    // Scopes: another agent's Private memory, and Shared against Private.
    let theirs = db.remember(mem("theirs", 1.0).with_agent("other")).unwrap();
    invalid(
        db.remember_keyed_superseding(mem("x", 1.0), "k7", &theirs),
        "scope",
    );
    let shared = db
        .remember(mem("shared", 1.0).with_scope(Scope::Shared))
        .unwrap();
    invalid(
        db.remember_keyed_superseding(mem("x", 1.0), "k7", &shared),
        "scope",
    );
    // ...but a Shared write takes a Shared memory.
    db.remember_keyed_superseding(mem("x", 1.0).with_scope(Scope::Shared), "k7", &shared)
        .unwrap();
    assert_eq!(
        db.history_in("k7", KeyScope::Shared)
            .unwrap()
            .into_iter()
            .map(|m| m.content)
            .collect::<Vec<_>>(),
        ["x", "shared"]
    );
    // An empty key, and engine metadata from the caller.
    invalid(
        db.remember_keyed_superseding(mem("x", 1.0), "", &b),
        "must not be empty",
    );
    let mut m = mem("x", 1.0);
    m.metadata.insert("_mnemo.key".into(), "k8".into());
    invalid(db.remember_keyed_superseding(m, "k8", &b), "reserved");
    db.flush().unwrap();
    drop(db);
    // Read-only.
    let mut ro = Mnemo::open_read_only(&p, PW).unwrap();
    assert!(matches!(
        ro.remember_keyed_superseding(mem("x", 1.0), "k9", &b),
        Err(MnemoError::ReadOnly)
    ));
}

#[test]
fn it_holds_across_a_reopen_and_a_compaction() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("s.mnemo");
    let (old, new) = {
        let mut db = Mnemo::create(&p, PW, cfg()).unwrap();
        let old = db.remember(mem("format v8", 1.0)).unwrap();
        let (new, _) = db
            .remember_keyed_superseding(mem("format v9", 2.0), "fact.format", &old)
            .unwrap();
        db.flush().unwrap();
        (old, new)
    };
    for round in ["reopen", "compaction"] {
        if round == "compaction" {
            Mnemo::compact_file(p.to_str().unwrap(), PW).unwrap();
        }
        let mut db = Mnemo::open(&p, PW).unwrap();
        // The cache rebuild sees the adopted record as superseded, not as
        // the slot's live record, though it carries the key.
        assert_eq!(
            history_texts(&mut db, "fact.format"),
            ["format v9", "format v8"],
            "{round}"
        );
        assert_eq!(recall_texts(&mut db, "format"), ["format v9"], "{round}");
        assert_eq!(
            db.get(&old).unwrap().metadata[MNEMO_META_SUPERSEDED_BY],
            new.to_string()
        );
        // The slot's live record is v9: an identical keyed write is a no-op.
        let (id, outcome) = db
            .remember_keyed(mem("format v9", 2.0), "fact.format")
            .unwrap();
        assert_eq!(outcome, WriteOutcome::Unchanged, "{round}");
        assert_eq!(id, new, "{round}");
        db.close().unwrap();
    }
}

#[cfg(feature = "failpoints")]
#[test]
fn a_shared_handle_writes_it_in_a_write_window() {
    if mnemo_engine::__shared::WINDOWS_OLD_RENAME {
        return;
    }
    let dir = tempdir().unwrap();
    let p = dir.path().join("s.mnemo");
    let old = {
        let mut db = Mnemo::create(&p, PW, cfg()).unwrap();
        let old = db
            .remember(mem("busy readers lock out a save", 1.0))
            .unwrap();
        db.close().unwrap();
        old
    };
    let mut s = Mnemo::open_shared(&p, PW, mnemo_engine::SharedOptions::default()).unwrap();
    let (_, outcome) = s
        .write(|db| {
            db.remember_keyed_superseding(
                mem("a writer past 500 ms gets the next turn", 2.0),
                "fact.busy",
                &old,
            )
        })
        .unwrap();
    assert_eq!(outcome, WriteOutcome::Superseded);
    drop(s);
    let mut db = Mnemo::open_read_only(&p, PW).unwrap();
    assert_eq!(
        history_texts(&mut db, "fact.busy"),
        [
            "a writer past 500 ms gets the next turn",
            "busy readers lock out a save"
        ]
    );
}
