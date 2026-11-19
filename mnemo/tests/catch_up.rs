//! v0.20.0 (v0200-3): incremental catch-up (`docs/design/catch-up.md`).
//!
//! A shared handle that finds another handle's commit applies only what
//! changed. After every catch-up its state must equal a fresh open of the
//! file: the catalog, the id index, the page maps, the small runs, the BM25
//! stats and dicts, the ANN index, the vector column's filled rows and
//! recall results. `__catch_ups` says which path ran: incremental where
//! the note says it can, the full reload everywhere else.
//!
//! Needs the `failpoints` feature (CI's `cargo test --features failpoints`).
#![cfg(feature = "failpoints")]

use mnemo_engine::__failpoints as fp;
use mnemo_engine::{
    CachePutOpts, KdfParams, Memory, MemoryPatch, MemoryType, Mnemo, MnemoConfig, MnemoError,
    RecallRequest, SharedMnemo, SharedOptions, Ulid,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tempfile::tempdir;

const DIMS: usize = 96;

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: DIMS,
        kdf: KdfParams::fast(),
        format_version: 9,
        ..Default::default()
    }
}

fn vector(i: usize) -> Vec<f32> {
    (0..DIMS)
        .map(|k| ((i * 7 + k * 3) % 11) as f32 + 0.5 + (i % 5) as f32 * 0.1)
        .collect()
}

/// About 300 bytes of text, like the bench corpus: a commit repacks its
/// record page's rows (FMT-9b.2), so the text size sets how many rows one
/// commit changes, and so how soon a delta chain reaches a quarter of the
/// catalog and a new base is written.
fn mem(i: usize) -> Memory {
    let words: String = (0..40)
        .map(|k| format!("w{} ", (i * 31 + k * 7) % 97))
        .collect();
    Memory::new(
        format!("memory {i} about topic {}: {words}", i % 13),
        MemoryType::Semantic,
        vector(i),
    )
}

/// A closed v9 file of `n` memories.
fn base(dir: &Path, n: usize) -> PathBuf {
    let p = dir.join("brain.mnemo");
    let mut db = Mnemo::create(&p, "pw", cfg()).unwrap();
    for i in 0..n {
        db.remember(mem(i)).unwrap();
    }
    db.close().unwrap();
    p
}

fn shared(p: &Path, pw: &str) -> SharedMnemo {
    Mnemo::open_shared(p, pw, SharedOptions::default()).unwrap()
}

/// The reader, with every lazy part built (the index, the column, BM25).
fn warm(r: &mut SharedMnemo) {
    r.read(|db| db.__warm_for_testing()).unwrap();
}

fn first_difference(a: &str, b: &str) -> String {
    let at = a
        .bytes()
        .zip(b.bytes())
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    let lo = at.saturating_sub(120);
    let cut = |s: &str| s.get(lo..(at + 120).min(s.len())).unwrap_or("").to_string();
    format!(
        "lengths {} and {}, first difference at {at}:\n  caught up: …{}…\n  fresh:     …{}…",
        a.len(),
        b.len(),
        cut(a),
        cut(b)
    )
}

/// The reader's state equals a fresh open of the file, part by part, and
/// both recall the same.
fn check(r: &mut SharedMnemo, p: &Path, pw: &str, tag: &str) {
    let db = r.__state().expect("loaded");
    let parts = db.__catch_up_parts();
    let filled = db.__column_filled();
    let mut fresh = Mnemo::open_read_only(p, pw).unwrap();
    fresh.__warm_for_testing().unwrap();
    let fparts: HashMap<String, String> = fresh.__catch_up_parts().into_iter().collect();
    for (name, v) in &parts {
        let f = fparts
            .get(name)
            .unwrap_or_else(|| panic!("{tag}: a fresh open has no {name}"));
        assert!(v == f, "{tag}: {name} differs: {}", first_difference(v, f));
    }
    let ffilled: HashMap<usize, (Vec<u16>, u32)> = fresh
        .__column_filled()
        .into_iter()
        .map(|(i, v, n)| (i, (v, n)))
        .collect();
    for (i, v, n) in filled {
        if let Some(f) = ffilled.get(&i) {
            assert!(*f == (v, n), "{tag}: column row {i} differs");
        }
    }
    for q in 0..5 {
        let req = RecallRequest::new(vector(1000 + q * 17))
            .top_k(8)
            .track_access(false);
        // Scores carry a recency boost, which moves between the two calls,
        // so they're compared within a tolerance.
        let mine = r.read(|db| db.recall(&req)).unwrap();
        let theirs = fresh.recall(&req).unwrap();
        let ids =
            |v: &[mnemo_engine::RecallResult]| v.iter().map(|h| h.memory.id).collect::<Vec<_>>();
        assert_eq!(ids(&mine), ids(&theirs), "{tag}: recall {q}");
        for (a, b) in mine.iter().zip(&theirs) {
            assert!((a.score - b.score).abs() < 1e-4, "{tag}: recall {q} scores");
        }
    }
}

/// Catch the reader up with one empty read window, and say how.
fn catch_up(r: &mut SharedMnemo) -> (u64, u64) {
    let before = r.__catch_ups();
    r.read(|_| Ok(())).unwrap();
    let after = r.__catch_ups();
    (after.0 - before.0, after.1 - before.1)
}

const INCREMENTAL: (u64, u64) = (1, 0);
const FULL: (u64, u64) = (0, 1);

fn chain_len(r: &mut SharedMnemo) -> usize {
    let parts = r.__state().unwrap().__catch_up_parts();
    let chain = &parts.iter().find(|(n, _)| n == "delta_chain").unwrap().1;
    chain.matches("RunPtr").count()
}

#[test]
fn each_kind_of_commit_is_caught_up_incrementally() {
    let dir = tempdir().unwrap();
    let p = base(dir.path(), 1000);
    let mut w = shared(&p, "pw");
    let mut r = shared(&p, "pw");
    warm(&mut r);
    let ids: Vec<Ulid> = r
        .read(|db| Ok(db.memories()?.into_iter().map(|m| m.id).collect()))
        .unwrap();

    w.write(|db| db.remember(mem(5000))).unwrap();
    assert_eq!(catch_up(&mut r), INCREMENTAL, "one remember");
    check(&mut r, &p, "pw", "one remember");

    w.write(|db| {
        for i in 0..10 {
            db.remember(mem(5100 + i))?;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(catch_up(&mut r), INCREMENTAL, "a batch");
    check(&mut r, &p, "pw", "a batch");

    w.write(|db| db.remember_keyed(mem(5200), "fact.one"))
        .unwrap();
    w.write(|db| db.remember_keyed(mem(5201), "fact.one"))
        .unwrap();
    assert_eq!(catch_up(&mut r), INCREMENTAL, "a keyed supersede");
    check(&mut r, &p, "pw", "a keyed supersede");

    let id = ids[3];
    w.write(move |db| {
        db.update(
            &id,
            MemoryPatch {
                importance: Some(0.9),
                ..Default::default()
            },
        )
    })
    .unwrap();
    assert_eq!(catch_up(&mut r), INCREMENTAL, "an update");
    check(&mut r, &p, "pw", "an update");

    // A new vector for a row the reader's column has filled.
    let id = ids[5];
    w.write(move |db| {
        db.update(
            &id,
            MemoryPatch {
                vector: Some(vector(77_777)),
                ..Default::default()
            },
        )
    })
    .unwrap();
    assert_eq!(catch_up(&mut r), INCREMENTAL, "a new vector");
    check(&mut r, &p, "pw", "a new vector");

    let id = ids[4];
    w.write(move |db| db.delete(&id)).unwrap();
    assert_eq!(catch_up(&mut r), INCREMENTAL, "a delete");
    check(&mut r, &p, "pw", "a delete");

    w.write(|db| db.cache_put("ns", "k", b"value", CachePutOpts::default()))
        .unwrap();
    assert_eq!(catch_up(&mut r), INCREMENTAL, "a cache put");
    check(&mut r, &p, "pw", "a cache put");

    // A rekey keeps the data key: the key slots and the header's wrap move.
    w.rekey("pw2", KdfParams::fast()).unwrap();
    assert_eq!(catch_up(&mut r), INCREMENTAL, "a rekey");
    check(&mut r, &p, "pw2", "a rekey");
}

/// Addition (a): the reader misses several commits from two writers (each
/// write window also takes a lease, a header-only transaction), then
/// catches up once; then it misses a base rewrite followed by more deltas.
#[test]
fn several_commits_behind_from_two_writers() {
    let dir = tempdir().unwrap();
    let p = base(dir.path(), 1000);
    let mut a = shared(&p, "pw");
    let mut b = shared(&p, "pw");
    let mut r = shared(&p, "pw");
    warm(&mut r);
    let before = chain_len(&mut r);
    a.write(|db| db.remember(mem(6000))).unwrap();
    b.write(|db| db.remember(mem(6001))).unwrap();
    a.write(|db| db.remember(mem(6002))).unwrap();
    b.write(|db| db.remember_keyed(mem(6003), "fact.k"))
        .unwrap();
    assert_eq!(catch_up(&mut r), INCREMENTAL);
    assert_eq!(chain_len(&mut r), before + 4, "four deltas in one catch-up");
    check(&mut r, &p, "pw", "four commits behind");

    // A batch past a quarter of the catalog writes a new base; then more
    // deltas on it.
    a.write(|db| {
        for i in 0..300 {
            db.remember(mem(6100 + i))?;
        }
        Ok(())
    })
    .unwrap();
    b.write(|db| db.remember(mem(6200))).unwrap();
    a.write(|db| db.remember(mem(6201))).unwrap();
    assert_eq!(catch_up(&mut r), FULL, "behind across a base rewrite");
    check(&mut r, &p, "pw", "across a base rewrite");
}

#[test]
fn a_base_rewrite_falls_back_and_the_rest_are_incremental() {
    let dir = tempdir().unwrap();
    let p = base(dir.path(), 1000);
    let mut w = shared(&p, "pw");
    let mut r = shared(&p, "pw");
    warm(&mut r);
    let (mut inc, mut full) = (0, 0);
    for i in 0..70 {
        w.write(move |db| db.remember(mem(7000 + i))).unwrap();
        let (a, b) = catch_up(&mut r);
        inc += a;
        full += b;
        if b == 1 || i % 10 == 0 {
            check(&mut r, &p, "pw", &format!("commit {i}"));
        }
    }
    assert!(
        full >= 1,
        "a base rewrite happened: {full} full, {inc} incremental"
    );
    assert!(inc >= 60, "{inc} incremental, {full} full");
}

/// Deleting more than a quarter of the rows makes the next base drop them,
/// so rows move: a full reload. A compaction makes a new file: the same.
#[test]
fn tombstones_and_compaction_fall_back() {
    let dir = tempdir().unwrap();
    let p = base(dir.path(), 1000);
    let mut w = shared(&p, "pw");
    let mut r = shared(&p, "pw");
    warm(&mut r);
    let ids: Vec<Ulid> = r
        .read(|db| Ok(db.memories()?.into_iter().map(|m| m.id).collect()))
        .unwrap();
    let doomed: Vec<Ulid> = ids[..300].to_vec();
    w.write(move |db| {
        for id in &doomed {
            db.delete(id)?;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(catch_up(&mut r), FULL, "tombstones dropped");
    check(&mut r, &p, "pw", "tombstones dropped");

    drop(w);
    Mnemo::compact_file(p.to_str().unwrap(), "pw").unwrap();
    assert_eq!(catch_up(&mut r), FULL, "a compaction");
    check(&mut r, &p, "pw", "a compaction");
}

/// The case the page cache's generation key exists for: the reader has a
/// record page and a vector page cached; another handle frees them and
/// writes new rows into them; the reader catches up and must see the new
/// content.
#[test]
fn a_reused_page_is_never_served_stale() {
    let dir = tempdir().unwrap();
    let p = base(dir.path(), 1000);
    let mut r = shared(&p, "pw");
    warm(&mut r);
    let rows = r.__state().unwrap().__row_pages();
    let doomed: Vec<(Ulid, u64, u64)> = rows[..20].to_vec();
    // Cache their record pages in the reader.
    for (id, _, _) in &doomed {
        let id = *id;
        r.read(move |db| db.get(&id)).unwrap();
    }
    let old_pages: HashSet<u64> = doomed
        .iter()
        .flat_map(|&(_, rec, vec)| [rec, vec])
        .filter(|&p| p != 0)
        .collect();

    // Another process, with one snapshot kept so freed pages come back
    // quickly.
    let mut w = Mnemo::open(&p, "pw").unwrap();
    w.set_max_snapshots(1);
    for (id, _, _) in &doomed {
        w.delete(id).unwrap();
    }
    w.flush().unwrap();
    // One more commit, so the retained snapshot no longer holds the freed
    // pages; then the new rows, in two commits.
    w.remember(mem(8000)).unwrap();
    w.flush().unwrap();
    let mut new_ids = Vec::new();
    for half in 0..2 {
        for i in 0..10 {
            new_ids.push(w.remember(mem(8100 + half * 10 + i)).unwrap());
        }
        w.flush().unwrap();
    }
    let reused: Vec<(Ulid, u64, u64)> = w
        .__row_pages()
        .into_iter()
        .filter(|(id, rec, vec)| {
            new_ids.contains(id) && (old_pages.contains(rec) || old_pages.contains(vec))
        })
        .collect();
    w.close().unwrap();
    assert!(!reused.is_empty(), "the new rows took some freed pages");

    assert_eq!(catch_up(&mut r), INCREMENTAL);
    for (k, id) in new_ids.iter().enumerate() {
        let id = *id;
        let m = r.read(move |db| db.get(&id)).unwrap();
        assert_eq!(m.content, mem(8100 + k).content, "new row {k}");
        // Stored as fp16 (FMT-9b.1).
        for (x, y) in m.vector.iter().zip(&mem(8100 + k).vector) {
            assert!((x - y).abs() < 0.01, "new row {k}: {x} against {y}");
        }
    }
    for (id, _, _) in &doomed {
        let id = *id;
        assert!(r.read(move |db| db.get(&id)).is_err(), "deleted");
    }
    check(&mut r, &p, "pw", "pages reused");
}

/// The ANN index: deltas onto the reader's loaded index (from a writer
/// without the index loaded, so the reader reconciles the new rows, and
/// from one with it loaded, so its ops are in the chain); a rebuild
/// unloads it; a long run of commits takes the index past a rebase.
#[test]
fn the_index_follows() {
    std::env::set_var("MNEMO_DEV_ANN_MIN_FACTS", "1");
    let dir = tempdir().unwrap();
    let p = base(dir.path(), 1000);
    let mut w = shared(&p, "pw");
    w.write(|db| db.build_index().map(|_| ())).unwrap();
    let mut r = shared(&p, "pw");
    warm(&mut r);
    catch_up(&mut r);
    assert!(
        r.__state()
            .unwrap()
            .__catch_up_parts()
            .iter()
            .any(|(n, _)| n == "ann"),
        "the reader has the index loaded"
    );

    w.write(|db| db.remember(mem(9000))).unwrap();
    assert_eq!(catch_up(&mut r), INCREMENTAL, "a writer without the index");
    check(&mut r, &p, "pw", "a writer without the index");

    w.read(|db| db.__warm_for_testing()).unwrap();
    w.write(|db| db.remember(mem(9001))).unwrap();
    assert_eq!(catch_up(&mut r), INCREMENTAL, "a writer with the index");
    check(&mut r, &p, "pw", "a writer with the index");

    w.write(|db| db.build_index().map(|_| ())).unwrap();
    assert_eq!(catch_up(&mut r), INCREMENTAL, "a rebuild");
    assert!(
        !r.__state()
            .unwrap()
            .__catch_up_parts()
            .iter()
            .any(|(n, _)| n == "ann"),
        "a new index base unloads the reader's index"
    );
    check(&mut r, &p, "pw", "a rebuild");

    w.read(|db| db.__warm_for_testing()).unwrap();
    warm(&mut r);
    for i in 0..80 {
        w.write(move |db| db.remember(mem(9100 + i))).unwrap();
        catch_up(&mut r);
        if i % 20 == 19 {
            check(&mut r, &p, "pw", &format!("index commit {i}"));
        }
    }

    // Writes from a writer without the index loaded, so the reader's
    // reconcile has to see them. First an old row's importance: its record
    // gets a new generation and its vector stays where it was. Its index
    // entry came from the index build, and a fresh open now re-adds it, so
    // the reader must too. Then new vectors, for a recent row (its record
    // page's other rows move with their vectors unchanged, and are skipped)
    // and an old one. Then a commit that only repacks.
    let mut w2 = shared(&p, "pw");
    let ids: Vec<Ulid> = r
        .read(|db| Ok(db.memories()?.into_iter().map(|m| m.id).collect()))
        .unwrap();
    let old = ids[3];
    w2.write(move |db| {
        db.update(
            &old,
            MemoryPatch {
                importance: Some(0.8),
                ..Default::default()
            },
        )
    })
    .unwrap();
    assert_eq!(catch_up(&mut r), INCREMENTAL, "an old row's importance");
    check(&mut r, &p, "pw", "an old row's importance");
    let fresh_vector = |k: usize| -> Vec<f32> {
        (0..DIMS)
            .map(|j| ((j * 13 + k * 5) % 17) as f32 * 0.25 + 0.1)
            .collect()
    };
    for (k, id) in [ids[ids.len() - 2], ids[7]].into_iter().enumerate() {
        let v = fresh_vector(k);
        w2.write(move |db| {
            db.update(
                &id,
                MemoryPatch {
                    vector: Some(v),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        assert_eq!(catch_up(&mut r), INCREMENTAL, "a new vector {k}");
        check(&mut r, &p, "pw", &format!("a new vector {k}"));
    }
    // (Either path: the chain may be due for a new base here.)
    w2.write(|db| db.remember(mem(9300))).unwrap();
    catch_up(&mut r);
    check(&mut r, &p, "pw", "a repack");
}

#[test]
fn a_refused_write_then_a_commit_reloads() {
    let dir = tempdir().unwrap();
    let p = base(dir.path(), 1000);
    let mut w = shared(&p, "pw");
    let mut r = shared(&p, "pw");
    warm(&mut r);
    let e = r.write(|db| {
        db.remember(mem(9500))?;
        Err::<(), _>(MnemoError::Invalid("refused".into()))
    });
    assert!(e.is_err());
    w.write(|db| db.remember(mem(9501))).unwrap();
    assert_eq!(
        catch_up(&mut r),
        FULL,
        "no trusted state after a refused write"
    );
    check(&mut r, &p, "pw", "after a refused write");
}

/// Addition (b): a write fails before its commit point and is retried on
/// the same handle, which rewrites the same pages under the same
/// generation (v0200-1's restore). The writer's cache must hold the
/// retry's bytes, and a reader catches up to the retried commit.
#[test]
fn a_retried_write_rewrites_the_same_page() {
    let dir = tempdir().unwrap();
    let p = base(dir.path(), 1000);
    let mut r = shared(&p, "pw");
    warm(&mut r);
    let mut w = Mnemo::open(&p, "pw").unwrap();
    w.remember(mem(9600)).unwrap();
    w.flush().unwrap();
    let id = w.remember(mem(9601)).unwrap();
    fp::set_writes_failing(&[0]);
    assert!(w.flush().is_err(), "the first data page write fails");
    fp::set_writes_failing(&[]);
    w.flush().unwrap();
    assert_eq!(w.get(&id).unwrap().content, mem(9601).content);
    w.close().unwrap();
    assert_eq!(catch_up(&mut r), INCREMENTAL);
    let m = r.read(move |db| db.get(&id)).unwrap();
    assert_eq!(m.content, mem(9601).content);
    check(&mut r, &p, "pw", "a retried write");
}

/// Crash injection: another handle's commit fails at each of its writes in
/// turn (the handle is then dropped, as a process that died). The reader's
/// next call must reach the file's state, whichever it is (the old one, or
/// the replayed commit), and never an earlier or half state.
#[test]
fn a_writer_that_dies_at_each_write_is_caught_up_soundly() {
    let dir = tempdir().unwrap();
    let p = base(dir.path(), 1000);
    let mut r = shared(&p, "pw");
    warm(&mut r);
    // How many writes one commit makes.
    let mut w = shared(&p, "pw");
    fp::set_writes_failing(&[]);
    w.write(|db| db.remember(mem(9700))).unwrap();
    let writes = fp::writes_seen();
    drop(w);
    assert!(writes > 2, "a commit makes {writes} writes");
    catch_up(&mut r);
    check(&mut r, &p, "pw", "before the crashes");
    // A write the engine retries (the I/O retry) can still commit.
    let (mut failed, mut replays) = (0, 0);
    for k in 0..writes {
        let mut w = shared(&p, "pw");
        fp::set_writes_until_fail(k as i64);
        let res = w.write(move |db| db.remember(mem(9800 + k as usize)));
        fp::set_writes_until_fail(-1);
        drop(w);
        failed += usize::from(res.is_err());
        let (inc, full) = catch_up(&mut r);
        assert!(inc + full <= 1, "write {k}: one catch-up at most");
        replays += full;
        check(&mut r, &p, "pw", &format!("a crash at write {k}"));
    }
    assert!(failed > 0, "{failed} of {writes} commits failed");
    assert!(replays > 0, "a crash after the commit point is replayed");
    // And the reader still catches up incrementally after them.
    let mut w = shared(&p, "pw");
    w.write(|db| db.remember(mem(9900))).unwrap();
    drop(w);
    catch_up(&mut r);
    assert_eq!(catch_up(&mut r), (0, 0));
    w = shared(&p, "pw");
    w.write(|db| db.remember(mem(9901))).unwrap();
    assert_eq!(catch_up(&mut r), INCREMENTAL, "after the crashes");
    check(&mut r, &p, "pw", "after the crashes");
}
