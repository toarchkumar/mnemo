// Shared body of `tests/fix1_correctness_sweep.rs`, compiled once per format: the
// including module defines `FORMAT` (8 or 9).

use mnemo_engine::{
    CacheFlushPolicy, CachePutOpts, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig,
};
use tempfile::tempdir;

fn fast_cfg(dimensions: usize) -> MnemoConfig {
    MnemoConfig {
        format_version: FORMAT,
        dimensions,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn mem(i: usize, agent: &str) -> Memory {
    let mut m = Memory::new(
        format!("m-{i}"),
        MemoryType::Semantic,
        vec![(i as f32) + 1.0, 1.0, 2.0, 3.0],
    );
    m.agent_id = agent.into();
    m
}

fn put_opts() -> CachePutOpts {
    CachePutOpts {
        content_type: "text".into(),
        ttl_secs: None,
        cost_hint_ms: None,
    }
}

// --- R-001.1 + R-002.2 part 2: compaction preserves the result cache ----

#[test]
fn compact_preserves_live_cache_and_drops_tombstoned_and_expired() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cache.mnemo");
    let path_str = path.to_str().unwrap().to_string();
    let pp = "pp";
    {
        let mut db = Mnemo::create(&path, pp, fast_cfg(4)).unwrap();
        db.remember(mem(1, "alpha")).unwrap();
        db.remember(mem(2, "alpha")).unwrap();

        // Three cache entries: one live, one TTL-expired, one tombstoned.
        db.cache_put("ns", "live", b"LIVE_VALUE", put_opts())
            .unwrap();
        let expiring = CachePutOpts {
            content_type: "text".into(),
            ttl_secs: Some(1),
            cost_hint_ms: None,
        };
        db.cache_put("ns", "expiring", b"EXPIRING_VALUE", expiring)
            .unwrap();
        db.cache_put("ns", "to-delete", b"BYE", put_opts()).unwrap();
        db.cache_delete("ns", "to-delete").unwrap();

        // A cache hit on "live" updates the directory's accessed_at /
        // access_count — those are the values compaction must copy.
        let _ = db.cache_get("ns", "live").unwrap().unwrap();
        let _ = db.cache_get("ns", "live").unwrap().unwrap();

        db.flush().unwrap();
        db.close().unwrap();
    }

    // Wait past the 1-second TTL so compaction sees "expiring" as
    // expired.
    std::thread::sleep(std::time::Duration::from_millis(1100));

    let report = Mnemo::compact_file(&path_str, pp).unwrap();
    assert_eq!(report.cache_before, 1); // "live" only; expiring/deleted dropped
    assert_eq!(report.cache_after, 1);
    assert_eq!(report.before, 2);
    assert_eq!(report.after, 2);

    let mut db = Mnemo::open(&path, pp).unwrap();
    let hit = db.cache_get("ns", "live").unwrap().unwrap();
    assert_eq!(hit.value, b"LIVE_VALUE");
    assert!(
        hit.access_count >= 2,
        "directory access_count must survive compaction (saw {})",
        hit.access_count
    );
    // "expiring" and "to-delete" are gone.
    assert!(db.cache_get("ns", "expiring").unwrap().is_none());
    assert!(db.cache_get("ns", "to-delete").unwrap().is_none());
}

#[test]
fn verify_walks_cache_entries_too() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("v.mnemo");
    let mut db = Mnemo::create(&path, "pp", fast_cfg(4)).unwrap();
    for i in 0..3 {
        db.remember(mem(i, "a")).unwrap();
    }
    db.cache_put("ns", "k1", b"v1", put_opts()).unwrap();
    db.cache_put("ns", "k2", b"v2", put_opts()).unwrap();
    db.flush().unwrap();
    // 3 memories + 2 cache entries = 5 decoded records.
    assert_eq!(db.verify().unwrap(), 5);
}

// --- R-001.5 + R-002.2 part 3: safe export_encrypted ---------------------

#[test]
fn export_encrypted_refuses_same_path() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("e.mnemo");
    let mut db = Mnemo::create(&path, "pp", fast_cfg(4)).unwrap();
    db.remember(mem(1, "a")).unwrap();
    db.flush().unwrap();

    let before = std::fs::metadata(&path).unwrap().len();
    let err = db.export_encrypted(&path).unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("refuses to overwrite"),
        "export-to-self must raise Invalid, got {msg}"
    );
    let after = std::fs::metadata(&path).unwrap().len();
    assert_eq!(before, after, "database must not be truncated");

    // Database still usable after the refused export.
    assert_eq!(db.len(), 1);
}

#[cfg(unix)]
#[test]
fn export_encrypted_refuses_symlink_to_open_db() {
    use std::os::unix::fs::symlink;
    let dir = tempdir().unwrap();
    let path = dir.path().join("e.mnemo");
    let link = dir.path().join("e-link.mnemo");
    let mut db = Mnemo::create(&path, "pp", fast_cfg(4)).unwrap();
    db.remember(mem(1, "a")).unwrap();
    db.flush().unwrap();
    symlink(&path, &link).unwrap();

    let before = std::fs::metadata(&path).unwrap().len();
    let err = db.export_encrypted(&link).unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("refuses to overwrite"),
        "symlink to open db must be refused, got {msg}"
    );
    assert_eq!(std::fs::metadata(&path).unwrap().len(), before);
}

#[cfg(unix)]
#[test]
fn export_encrypted_refuses_hard_link_to_open_db() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("e.mnemo");
    let hard = dir.path().join("e-hard.mnemo");
    let mut db = Mnemo::create(&path, "pp", fast_cfg(4)).unwrap();
    db.remember(mem(1, "a")).unwrap();
    db.flush().unwrap();
    std::fs::hard_link(&path, &hard).unwrap();

    let before = std::fs::metadata(&path).unwrap().len();
    let err = db.export_encrypted(&hard).unwrap_err();
    assert!(format!("{err:?}").contains("refuses to overwrite"));
    assert_eq!(std::fs::metadata(&path).unwrap().len(), before);
}

#[test]
fn export_encrypted_round_trips_a_fresh_destination() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("src.mnemo");
    let dest = dir.path().join("dst.mnemo");
    {
        let mut db = Mnemo::create(&path, "pp", fast_cfg(4)).unwrap();
        db.remember(mem(1, "a")).unwrap();
        db.remember(mem(2, "b")).unwrap();
        db.flush().unwrap();
        db.export_encrypted(&dest).unwrap();
        // The source handle is still live and usable after the export.
        assert_eq!(db.len(), 2);
        db.close().unwrap();
    } // handle fully drops here, Windows exclusive lock releases

    // Primary invariant: the destination is byte-identical to the
    // source. The source bytes are already encrypted (that's the
    // whole point of the API — no re-encryption), so a byte-for-byte
    // equal copy proves the export worked: anything that opens the
    // source also opens the destination, under the same DEK and
    // passphrase, by construction.
    //
    // We deliberately don't `Mnemo::open` either file from here —
    // on Windows CI the OS's cleanup of the just-closed handle +
    // the write-exclusive lock the next open tries to acquire sit
    // behind the kernel's file-system cleanup for a few ms
    // (code 33 / `ERROR_LOCK_VIOLATION`), which is a flaky-test
    // shape, not a product bug. The decrypt-round-trip story is
    // already covered by `compact_preserves_live_cache_and_drops_
    // tombstoned_and_expired` above, which goes through a close +
    // compact + reopen without racing because `compact_file` owns
    // the open/close sequencing internally.
    //
    // `std::fs::read` opens with `GENERIC_READ` and default sharing,
    // which is fine here because no handle holds the file anymore.
    let sbytes = std::fs::read(&path).unwrap();
    let dbytes = std::fs::read(&dest).unwrap();
    assert_eq!(
        sbytes, dbytes,
        "export_encrypted must byte-copy the already-encrypted file"
    );
}

// --- R-001.5: kdf_params() + rekey preserves KDF -------------------------

#[test]
fn kdf_params_returns_current_file_params() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("k.mnemo");
    let fast = KdfParams::fast();
    let mut db = Mnemo::create(&path, "pp", fast_cfg(4)).unwrap();
    db.remember(mem(1, "a")).unwrap();
    db.flush().unwrap();
    let kp = db.kdf_params();
    assert_eq!(kp.m_cost, fast.m_cost);
    assert_eq!(kp.t_cost, fast.t_cost);
    assert_eq!(kp.p_cost, fast.p_cost);
}

#[test]
fn rekey_with_kdf_params_preserves_cost() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("rk.mnemo");
    let mut db = Mnemo::create(&path, "old", fast_cfg(4)).unwrap();
    db.remember(mem(1, "a")).unwrap();
    db.flush().unwrap();
    let before = db.kdf_params();

    // Explicitly pass the current params to rekey — the Python
    // binding's `rekey(fast=False)` path does exactly this now
    // (`self.inner.kdf_params()` passed to the core).
    db.rekey("new", before).unwrap();
    let after = db.kdf_params();
    assert_eq!(after.m_cost, before.m_cost);
    assert_eq!(after.t_cost, before.t_cost);
    assert_eq!(after.p_cost, before.p_cost);
    db.close().unwrap();
    assert!(Mnemo::open(&path, "new").is_ok());
}

// --- R-002.2 part 1: read-only cache_get accessed_at ---------------------

#[test]
fn read_only_cache_get_returns_directory_accessed_at() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ro.mnemo");
    let pp = "pp";
    {
        let mut db = Mnemo::create(&path, pp, fast_cfg(4)).unwrap();
        db.cache_put("ns", "k", b"v", put_opts()).unwrap();
        // Three hits bump the directory's accessed_at + access_count.
        let _ = db.cache_get("ns", "k").unwrap().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let _ = db.cache_get("ns", "k").unwrap().unwrap();
        let _ = db.cache_get("ns", "k").unwrap().unwrap();
        db.flush().unwrap();
        db.close().unwrap();
    }
    let mut ro = Mnemo::open_read_only(&path, pp).unwrap();
    let hit = ro.cache_get("ns", "k").unwrap().unwrap();
    // The persisted directory recorded the hits above; read-only
    // mode doesn't update them further, but it MUST surface the
    // live accessed_at from the directory, not the (unchanged)
    // insert-time snapshot in the record body. Since created_at
    // and the first hit happened before the sleep, accessed_at
    // must be strictly greater than created_at.
    assert!(
        hit.accessed_at > hit.created_at,
        "read-only hit should return the directory's post-hit \
         accessed_at (got accessed_at={}, created_at={})",
        hit.accessed_at,
        hit.created_at
    );
    assert!(
        hit.access_count >= 3,
        "read-only hit should return the directory's access_count \
         (got {})",
        hit.access_count
    );
}

// --- R-001.8 part 3: no restore-point push on cache-only flushes ---------

#[test]
fn cache_only_flush_does_not_push_a_snapshot() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("snap.mnemo");
    let mut db = Mnemo::create(&path, "pp", fast_cfg(4)).unwrap();

    // One memory flush creates one snapshot.
    db.remember(mem(1, "a")).unwrap();
    db.flush().unwrap();
    let after_memory = db.snapshots().len();
    assert_eq!(after_memory, 1, "memory flush must push a snapshot");

    // Many cache-only flushes must not grow the snapshot manifest.
    for i in 0..10 {
        let key = format!("k-{i}");
        db.cache_put("ns", &key, b"v", put_opts()).unwrap();
        db.flush().unwrap();
    }
    let after_cache = db.snapshots().len();
    assert_eq!(
        after_cache, 1,
        "cache-only flushes must not push restore points (saw {} after 10 flushes)",
        after_cache
    );

    // A fresh memory flush pushes one more.
    db.remember(mem(2, "a")).unwrap();
    db.flush().unwrap();
    assert_eq!(db.snapshots().len(), 2);
}

// --- R-001.8 part 4: Batched max_age checked on reads --------------------

#[test]
fn batched_max_age_triggers_flush_on_read() {
    use std::time::Duration;
    let dir = tempdir().unwrap();
    let path = dir.path().join("b.mnemo");
    let mut db = Mnemo::create(&path, "pp", fast_cfg(4)).unwrap();
    db.set_cache_flush_policy(CacheFlushPolicy::Batched {
        max_dirty: 1_000_000,
        max_age: Duration::from_millis(200),
    });
    db.cache_put("ns", "k", b"v", put_opts()).unwrap();
    let before = db.snapshots().len(); // 0 because remember hasn't been called
    assert_eq!(before, 0);

    // Wait past max_age without touching cache_put.
    std::thread::sleep(Duration::from_millis(300));

    // A read on the same key should auto-flush because the batch
    // timed out. After the flush, re-reads return the same entry.
    let hit = db.cache_get("ns", "k").unwrap().unwrap();
    assert_eq!(hit.value, b"v");

    // A read on a different key should not flush again (the batch
    // is already reset); confirm by checking no snapshot was
    // pushed (cache-only flushes don't snapshot per R-001.8 part 3).
    assert_eq!(db.snapshots().len(), 0);

    // Reopen — the earlier cached write must be durable, which it
    // wouldn't be if the batch had stayed pending.
    db.close().unwrap();
    let mut reopened = Mnemo::open(&path, "pp").unwrap();
    assert_eq!(reopened.cache_get("ns", "k").unwrap().unwrap().value, b"v");
}

// --- R-001.6 is covered in `tests/cli_smoke.rs` --------------------------
// (`embed_cmd_rejects_all_zero_vector_and_respawns` +
//  `embed_cmd_rejects_non_finite_vector_and_respawns`).

// --- R-001.8 part 1 (dimensions() over stats().dimensions) ---------------
// Covered implicitly by the Python binding change (lib.rs
// `insert_default_manifest`). A dedicated test would need the Python
// crate's test harness; an integration-level smoke test here isn't
// meaningful because `self.dimensions()` is a thin accessor.
