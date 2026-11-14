//! v0.14.0 step 2b: `Mnemo::open_shared` (12.4 design note, "The protocol"
//! steps 4 to 8, "Catching up", "Writes that only this handle makes").
//! Failpoints builds only (CI's test step on every OS), since several tests
//! fail chosen writes.
#![cfg(feature = "failpoints")]

use std::path::{Path, PathBuf};

use mnemo_engine::__failpoints::{
    after_next_shared_commit, before_next_shared_commit, path_check_failures_left,
    set_path_check_failures, set_writes_until_fail, set_writes_until_fail_n,
};

use mnemo_engine::{
    CachePutOpts, Embedder, EmbedderIdentity, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig,
    MnemoError, RecallRequest, Result, SharedMnemo, SharedOptions, Ulid,
};
use tempfile::tempdir;

const PW: &str = "shared-pw";

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 2,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

/// A v9 brain with `n` memories, closed.
fn brain(dir: &Path, n: usize) -> PathBuf {
    let p = dir.join("b.mnemo");
    let mut db = Mnemo::create(&p, PW, cfg()).unwrap();
    for i in 0..n {
        db.remember(mem(&format!("seed {i}"))).unwrap();
    }
    db.flush().unwrap();
    db.close().unwrap();
    p
}

fn mem(text: &str) -> Memory {
    Memory::new(text, MemoryType::Semantic, vec![1.0, 0.0])
}

/// A shared handle, or `None` (the test skips) on a build that refuses
/// shared mode (Windows, Rust before 1.85).
fn shared(p: &Path) -> Option<SharedMnemo> {
    if mnemo_engine::__shared::WINDOWS_OLD_RENAME {
        eprintln!("skipped: this build refuses shared mode");
        return None;
    }
    Some(Mnemo::open_shared(p, PW, SharedOptions::default()).unwrap())
}

/// What a fresh open of the file has: the live memories' texts, sorted.
/// A copy is opened read-write, so a pending WAL (which the next writer
/// heals) is replayed in the copy, as any open would.
fn on_disk(p: &Path) -> Vec<String> {
    let copy = p.with_extension("view");
    std::fs::copy(p, &copy).unwrap();
    let mut db = Mnemo::open(&copy, PW).unwrap();
    let mut v: Vec<String> = db
        .memories()
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    drop(db);
    std::fs::remove_file(&copy).unwrap();
    v.sort();
    v
}

fn in_handle(s: &mut SharedMnemo) -> Vec<String> {
    let mut v: Vec<String> = s
        .read(|db| db.memories())
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    v.sort();
    v
}

fn remember(s: &mut SharedMnemo, text: &str) -> Result<Ulid> {
    let m = mem(text);
    s.write(move |db| db.remember(m))
}

// --- The seal's three outcomes (protocol step 5) -------------------------

#[test]
fn an_unchanged_file_is_not_reloaded() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 2);
    let Some(mut a) = shared(&p) else { return };
    assert_eq!(a.__reloads(), 1, "the open loads once");
    for _ in 0..5 {
        a.read(|db| db.recall(&RecallRequest::new(vec![1.0, 0.0]).top_k(2)))
            .unwrap();
    }
    remember(&mut a, "mine").unwrap();
    a.read(|db| db.memories()).unwrap();
    assert_eq!(a.__reloads(), 1, "its own commit doesn't make it reload");
}

#[test]
fn another_processs_commit_is_seen() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let Some(mut a) = shared(&p) else { return };
    let mut b = shared(&p).unwrap();
    remember(&mut b, "from b").unwrap();
    let before = a.__reloads();
    assert_eq!(in_handle(&mut a), vec!["from b", "seed 0"]);
    assert_eq!(a.__reloads(), before + 1);
    // And a plain (lifetime-locked) writer's commit too, once it closes.
    let mut plain = Mnemo::open(&p, PW).unwrap();
    plain.remember(mem("from plain")).unwrap();
    plain.close().unwrap();
    assert_eq!(in_handle(&mut a), vec!["from b", "from plain", "seed 0"]);
}

#[test]
fn a_compactions_new_data_key_is_followed_with_the_kept_secret() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 2);
    let Some(mut a) = shared(&p) else { return };
    remember(&mut a, "before").unwrap();
    Mnemo::compact_file(p.to_str().unwrap(), PW).unwrap();
    assert_eq!(in_handle(&mut a), vec!["before", "seed 0", "seed 1"]);
    remember(&mut a, "after").unwrap();
    assert_eq!(on_disk(&p), vec!["after", "before", "seed 0", "seed 1"]);

    // A plain rekey keeps the data key: the handle keeps working.
    let mut plain = Mnemo::open(&p, PW).unwrap();
    plain.rekey("second-pw", KdfParams::fast()).unwrap();
    plain.close().unwrap();
    assert_eq!(in_handle(&mut a).len(), 4);

    // A compaction under the new secret: the kept secret no longer opens it.
    Mnemo::compact_file(p.to_str().unwrap(), "second-pw").unwrap();
    let e = a.read(|db| db.memories()).unwrap_err();
    assert!(matches!(e, MnemoError::WrongPassphrase), "{e}");
}

// --- The reload (catching up) ------------------------------------------

struct Fixed;

impl Embedder for Fixed {
    fn identity(&self) -> EmbedderIdentity {
        EmbedderIdentity {
            name: "fixed-test".into(),
            dimensions: 2,
            normalize: false,
        }
    }
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![0.0, 1.0]).collect())
    }
}

#[test]
fn a_reload_keeps_settings_and_the_embedder_and_clears_the_page_cache() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 20);
    let Some(mut a) = shared(&p) else { return };
    a.set_embedder(Fixed).unwrap();
    a.write(|db| {
        db.set_cache_capacity(77);
        db.set_vector_cache_budget(1 << 20);
        Ok(())
    })
    .unwrap();
    a.read(|db| db.memories()).unwrap();
    let (cached, cap) = a.__page_cache();
    assert!(cached > 0, "the read filled the cache");
    assert_eq!(cap, 77);

    let mut b = shared(&p).unwrap();
    remember(&mut b, "from b").unwrap();
    let reloads = a.__reloads();
    a.read(|_| Ok(())).unwrap();
    assert_eq!(a.__reloads(), reloads + 1);
    // A new page cache: what a fresh handle's load reads, at the kept size.
    let fresh = shared(&p).unwrap().__page_cache().0;
    assert!(fresh < cached, "the load reads less than the read did");
    assert_eq!(
        a.__page_cache(),
        (fresh, 77),
        "cleared, with the same capacity"
    );
    let id = a
        .read(|db| Ok(db.embedder().map(|e| e.identity().name)))
        .unwrap();
    assert_eq!(id.as_deref(), Some("fixed-test"));
    // The embedder works after the reload.
    a.write(|db| db.remember_text("by text", MemoryType::Semantic))
        .unwrap();
    assert!(on_disk(&p).contains(&"by text".to_string()));
}

// --- Mutations (protocol step 7) ---------------------------------------

/// For every write a commit makes, fail that write in a handle that lives
/// on, then check that the handle never shows a memory a fresh open of
/// the file doesn't have, and that an error that isn't `MaybeStored`
/// stored nothing.
#[test]
fn a_failed_commit_leaves_the_handle_no_ahead_of_the_file() {
    let mut plain_errors = 0;
    for n in 0..40 {
        let dir = tempdir().unwrap();
        let p = brain(dir.path(), 2);
        let Some(mut a) = shared(&p) else { return };
        let reloads = a.__reloads();
        set_writes_until_fail(n);
        let r = remember(&mut a, "failing");
        set_writes_until_fail(-1);
        match &r {
            Ok(_) => {}
            Err(MnemoError::MaybeStored(_)) => {}
            Err(_) => {
                plain_errors += 1;
                assert!(
                    !on_disk(&p).contains(&"failing".to_string()),
                    "n={n}: {r:?}"
                );
            }
        }
        let shown = in_handle(&mut a);
        assert_eq!(shown, on_disk(&p), "n={n}: {r:?}");
        if r.is_err() {
            assert!(
                a.__reloads() > reloads,
                "n={n}: the handle must reload after {r:?}"
            );
        }
        // It keeps working.
        remember(&mut a, "next").unwrap();
        assert!(on_disk(&p).contains(&"next".to_string()));
        if r.is_ok() && n > 30 {
            break;
        }
    }
    assert!(plain_errors > 0, "no write before the commit point failed");
}

/// A failure after the commit point: the handle finishes the commit and
/// returns `Ok`. With the finishing step failing too: `MaybeStored`, and
/// the write is in the file once anything opens it.
#[test]
fn after_the_commit_point_the_write_is_finished_or_maybe_stored() {
    let (mut finished, mut maybe) = (0, 0);
    for n in 0..40 {
        for count in [1, 2] {
            let dir = tempdir().unwrap();
            let p = brain(dir.path(), 1);
            let Some(mut a) = shared(&p) else { return };
            set_writes_until_fail_n(n, count);
            let r = remember(&mut a, "late");
            set_writes_until_fail(-1);
            let stored = on_disk(&p).contains(&"late".to_string());
            match r {
                Ok(_) => {
                    assert!(stored, "n={n} count={count}: Ok but not stored");
                    if count == 1 {
                        finished += 1;
                    }
                }
                Err(MnemoError::MaybeStored(msg)) => {
                    maybe += 1;
                    assert!(stored, "n={n}: MaybeStored ({msg}), and the WAL had it");
                }
                Err(e) => assert!(!stored, "n={n} count={count}: {e}, but it was stored"),
            }
            assert_eq!(in_handle(&mut a), on_disk(&p), "n={n} count={count}");
        }
    }
    assert!(finished > 0, "no failure was finished by the handle");
    assert!(maybe > 0, "no failure came back as MaybeStored");
}

/// Another writer dies after its commit point (a plain handle whose flush
/// fails there and is dropped): the shared handle replays the WAL under
/// the exclusive lock and reloads.
#[test]
fn a_handle_that_was_current_replays_a_pending_wal_and_reloads() {
    for n in 0..40 {
        let dir = tempdir().unwrap();
        let p = brain(dir.path(), 1);
        let Some(mut a) = shared(&p) else { return };
        assert_eq!(in_handle(&mut a), vec!["seed 0"]);
        let mut plain = Mnemo::open(&p, PW).unwrap();
        plain.remember(mem("dying")).unwrap();
        set_writes_until_fail(n);
        let r = plain.flush();
        set_writes_until_fail(-1);
        drop(plain);
        let pending = matches!(
            Mnemo::open_read_only(&p, PW).err(),
            Some(MnemoError::NeedsWriteOpen { .. })
        );
        if r.is_err() && pending {
            let reloads = a.__reloads();
            assert_eq!(in_handle(&mut a), vec!["dying", "seed 0"], "n={n}");
            assert!(a.__reloads() > reloads);
            assert!(
                Mnemo::open_read_only(&p, PW).is_ok(),
                "the WAL was replayed"
            );
            return;
        }
    }
    panic!("no write left a pending WAL");
}

#[test]
fn every_write_window_takes_a_fresh_generation() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let Some(mut a) = shared(&p) else { return };
    let mut b = shared(&p).unwrap();
    let mut gens = Vec::new();
    for i in 0..6 {
        let h = if i % 2 == 0 { &mut a } else { &mut b };
        let id = remember(h, &format!("w{i}")).unwrap();
        let g = h
            .read(|db| Ok(db.__record_location(&id)))
            .unwrap()
            .unwrap()
            .2;
        gens.push(g);
    }
    for w in gens.windows(2) {
        assert!(w[1] > w[0], "generations {gens:?}");
    }

    // A window that fails before its commit, after sealing: the next
    // window, in the other handle, uses a later generation.
    for n in 0..30 {
        set_writes_until_fail(n);
        let m = mem("sealed then lost");
        let r = a.write(move |db| {
            let id = db.remember(m)?;
            Ok(id)
        });
        set_writes_until_fail(-1);
        let Err(e) = r else { continue };
        if matches!(e, MnemoError::MaybeStored(_)) {
            continue;
        }
        let attempted = a
            .__state()
            .and_then(|db| {
                db.memories()
                    .ok()?
                    .into_iter()
                    .find(|m| m.content == "sealed then lost")
                    .map(|m| m.id)
            })
            .and_then(|id| a.__state()?.__record_location(&id));
        let id = remember(&mut b, "after the failure").unwrap();
        let g = b
            .read(|db| Ok(db.__record_location(&id)))
            .unwrap()
            .unwrap()
            .2;
        if let Some((_, _, lost)) = attempted {
            assert!(g > lost, "b sealed under {g}, a's lost window under {lost}");
        }
        assert!(g > *gens.last().unwrap());
        return;
    }
    panic!("no write failed before its commit point");
}

#[test]
fn the_path_is_checked_again_after_the_commit() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let other = dir.path().join("other.mnemo");
    {
        let mut db = Mnemo::create(&other, PW, cfg()).unwrap();
        db.remember(mem("the other file")).unwrap();
        db.close().unwrap();
    }
    let Some(mut a) = shared(&p) else { return };
    let (from, to) = (other.clone(), p.clone());
    after_next_shared_commit(move || std::fs::rename(&from, &to).unwrap());
    let e = remember(&mut a, "lost to the rename").unwrap_err();
    assert!(matches!(e, MnemoError::ReplacedDuringWrite { .. }), "{e}");
    assert!(e.to_string().contains("replaced during the write"), "{e}");
    // The handle follows the file now at the path.
    assert_eq!(in_handle(&mut a), vec!["the other file"]);
}

/// A rename over the path after the window's identity check and before its
/// commit (an older build's compaction renames without its lock): the
/// commit's own check refuses with `Replaced`, nothing is written, and the
/// message says the next call follows the new file, which it does.
#[test]
fn a_rename_before_the_commit_is_refused_and_the_next_call_follows() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let other = dir.path().join("other.mnemo");
    {
        let mut db = Mnemo::create(&other, PW, cfg()).unwrap();
        db.remember(mem("the other file")).unwrap();
        db.close().unwrap();
    }
    let Some(mut a) = shared(&p) else { return };
    let (from, to) = (other.clone(), p.clone());
    before_next_shared_commit(move || std::fs::rename(&from, &to).unwrap());
    let e = remember(&mut a, "never written").unwrap_err();
    assert!(matches!(e, MnemoError::Replaced { .. }), "{e}");
    let msg = e.to_string();
    assert!(
        msg.contains("nothing was written") && msg.contains("the next call follows the new file"),
        "{msg}"
    );
    assert!(
        !msg.contains("Reopen"),
        "a shared handle needn't reopen: {msg}"
    );
    assert_eq!(on_disk(&p), vec!["the other file"]);
    // The handle follows the file now at the path, and writes to it.
    assert_eq!(in_handle(&mut a), vec!["the other file"]);
    remember(&mut a, "after the rename").unwrap();
    assert_eq!(on_disk(&p), vec!["after the rename", "the other file"]);
}

/// RB-008.2: a brain moved away between the window's identity check and
/// its commit: the commit refuses, saying the file was moved or removed.
#[test]
fn a_move_before_the_commit_says_moved_or_removed() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let moved = dir.path().join("moved.mnemo");
    let Some(mut a) = shared(&p) else { return };
    let (from, to) = (p.clone(), moved.clone());
    before_next_shared_commit(move || std::fs::rename(&from, &to).unwrap());
    let e = remember(&mut a, "never written").unwrap_err();
    assert!(
        matches!(
            e,
            MnemoError::Replaced {
                shared: true,
                missing: true,
                ..
            }
        ),
        "{e}"
    );
    assert!(e.to_string().contains("was moved or removed"), "{e}");
    assert_eq!(on_disk(&moved).len(), 1, "nothing was written");
}

/// RB-008.2: when the check that the path still names the open file fails
/// itself (here a failpoint's permission error), a plain handle commits as
/// v0.13.2 did: it has held the lock since it opened, so nothing can have
/// compacted over it. A shared handle, which holds nothing between calls,
/// returns the error and writes nothing.
#[test]
fn a_failing_path_check_lets_a_plain_handle_commit_and_stops_a_shared_one() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);

    let mut db = Mnemo::open(&p, PW).unwrap();
    db.remember(mem("plain write")).unwrap();
    set_path_check_failures(1);
    let r = db.flush();
    let left = path_check_failures_left();
    set_path_check_failures(0);
    r.unwrap();
    assert_eq!(left, 0, "the flush checked the path");
    db.close().unwrap();
    assert_eq!(on_disk(&p).len(), 2);

    let Some(mut a) = shared(&p) else { return };
    set_path_check_failures(1);
    let r = remember(&mut a, "shared write");
    set_path_check_failures(0);
    let e = r.unwrap_err();
    assert!(
        matches!(&e, MnemoError::Io(io) if io.kind() == std::io::ErrorKind::PermissionDenied),
        "{e}"
    );
    assert_eq!(on_disk(&p).len(), 2, "nothing was written");
    remember(&mut a, "shared write").unwrap();
    assert_eq!(on_disk(&p).len(), 3);
}

// --- What only this handle writes --------------------------------------

#[test]
fn statistics_ride_the_next_commit_and_are_dropped_at_close() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 0);
    let Some(mut a) = shared(&p) else { return };
    let id = remember(&mut a, "looked up").unwrap();
    a.write(|db| db.cache_put("ns", "k", b"v", CachePutOpts::default()))
        .unwrap();
    let count = |p: &Path| {
        let mut db = Mnemo::open_read_only(p, PW).unwrap();
        let m = db.get(&id).unwrap().access_count;
        let c = db.cache_get("ns", "k").unwrap().unwrap().access_count;
        (m, c)
    };

    // Reads under the shared lock: tracked, not written.
    let hits = a
        .read(|db| db.recall(&RecallRequest::new(vec![1.0, 0.0]).top_k(1)))
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(a.read(|db| db.cache_get("ns", "k")).unwrap().is_some());
    assert_eq!(count(&p), (0, 0));

    // The next write window carries them.
    a.write(|_| Ok(())).unwrap();
    assert_eq!(count(&p), (1, 1));

    // Read again, then close: dropped.
    a.read(|db| db.recall(&RecallRequest::new(vec![1.0, 0.0]).top_k(1)))
        .unwrap();
    a.close().unwrap();
    assert_eq!(count(&p), (1, 1));
}

#[test]
fn an_explicit_setting_is_refused_and_nothing_is_written() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let Some(mut a) = shared(&p) else { return };
    let m = mem("with a setting");
    let e = a
        .write(move |db| {
            db.set_max_snapshots(5);
            db.remember(m)
        })
        .unwrap_err();
    assert!(matches!(e, MnemoError::Unsupported(_)), "{e}");
    assert!(e.to_string().contains("default settings"), "{e}");
    assert_eq!(on_disk(&p), vec!["seed 0"]);
    let e = a
        .read(|db| {
            db.set_turn_compress_after(10);
            Ok(())
        })
        .unwrap_err();
    assert!(matches!(e, MnemoError::Unsupported(_)), "{e}");
    // The handle is back to the defaults and keeps working.
    remember(&mut a, "fine").unwrap();
    assert_eq!(on_disk(&p), vec!["fine", "seed 0"]);
}

#[test]
fn a_read_window_cant_write() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let Some(mut a) = shared(&p) else { return };
    let m = mem("sneaky");
    let e = a.read(move |db| db.remember(m)).unwrap_err();
    assert!(matches!(e, MnemoError::ReadOnly), "{e}");
    assert_eq!(on_disk(&p), vec!["seed 0"]);
}

#[test]
fn shared_mode_opens_only_an_existing_v9_brain() {
    let dir = tempdir().unwrap();
    if mnemo_engine::__shared::WINDOWS_OLD_RENAME {
        return;
    }
    let missing = dir.path().join("missing.mnemo");
    assert!(Mnemo::open_shared(&missing, PW, SharedOptions::default()).is_err());
    assert!(!missing.exists());
    let p = brain(dir.path(), 1);
    let e = Mnemo::open_shared(&p, "wrong", SharedOptions::default()).unwrap_err();
    assert!(matches!(e, MnemoError::WrongPassphrase), "{e}");
}

/// Four handles in four threads, each writing and reading: every write is
/// in the file at the end, and each handle sees them all.
#[test]
fn four_handles_write_and_read_together() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 0);
    if shared(&p).is_none() {
        return;
    }
    let threads: Vec<_> = (0..4)
        .map(|t| {
            let p = p.clone();
            std::thread::spawn(move || {
                let mut s = Mnemo::open_shared(&p, PW, SharedOptions::default()).unwrap();
                for i in 0..15 {
                    remember(&mut s, &format!("t{t}-{i}")).unwrap();
                    s.read(|db| db.recall(&RecallRequest::new(vec![1.0, 0.0]).top_k(3)))
                        .unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(on_disk(&p).len(), 60);
    let mut s = shared(&p).unwrap();
    assert_eq!(in_handle(&mut s).len(), 60);
}

/// A closure that panics inside a write window, caught by the caller: the
/// handle doesn't keep the lock (another handle writes at once), and its
/// next call reloads, since the write window cleared its stamp first.
#[test]
fn a_panicking_closure_releases_the_lock() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let Some(mut a) = shared(&p) else { return };
    let m = mem("half done");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = a.write(move |db| -> Result<()> {
            db.remember(m)?;
            panic!("the closure panics after a change");
        });
    }));
    assert!(caught.is_err(), "the panic reaches the caller");

    // Another handle gets the exclusive lock at once (a short deadline).
    let mut o = SharedOptions::default();
    o.deadline = std::time::Duration::from_millis(500);
    let mut b = Mnemo::open_shared(&p, PW, o).unwrap();
    remember(&mut b, "from b").unwrap();

    // The panicking handle reloads, and never shows the half-done write.
    let reloads = a.__reloads();
    assert_eq!(in_handle(&mut a), vec!["from b", "seed 0"]);
    assert!(a.__reloads() > reloads);
    assert_eq!(on_disk(&p), vec!["from b", "seed 0"]);

    // A read window that panics releases its shared lock too.
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = a.read(|_| -> Result<()> { panic!("a read panics") });
    }));
    assert!(caught.is_err());
    remember(&mut b, "b again").unwrap();
    assert_eq!(in_handle(&mut a), vec!["b again", "from b", "seed 0"]);
}

/// The settings check compares the cache budgets themselves: a budget whose
/// value changes is refused, even though the number of budgets doesn't.
#[test]
fn a_changed_cache_budget_value_is_refused() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let Some(mut a) = shared(&p) else { return };
    // Shared mode refuses setting a budget at all, so put one in the
    // handle's state directly (a test hook), then change only its value.
    let budget = |n| mnemo_engine::CacheBudget {
        max_entries: n,
        max_bytes: 1 << 20,
    };
    a.__state().unwrap().set_cache_budget("ns", budget(10));
    let m = mem("with a changed budget");
    let e = a
        .write(move |db| {
            db.set_cache_budget("ns", budget(5));
            db.remember(m)
        })
        .unwrap_err();
    assert!(matches!(e, MnemoError::Unsupported(_)), "{e}");
    assert_eq!(on_disk(&p), vec!["seed 0"]);
}
