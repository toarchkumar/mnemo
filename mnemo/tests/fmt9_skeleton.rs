//! FMT-9a skeleton: the v9 header, the quarantined free list (a), the
//! key-slot run (b) and the page generation in the AAD.
//!
//! v9 is still created only through the hidden `MnemoConfig::format_version`
//! switch; every test here asks for it explicitly (and `v8_is_untouched`
//! asks for v8), so the file runs the same with or without
//! `MNEMO_DEV_FORMAT=9`.

use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, MnemoError, Ulid};
use tempfile::tempdir;

const PAGE: usize = 8192;

fn cfg(version: u16) -> MnemoConfig {
    MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        format_version: version,
        ..Default::default()
    }
}

/// FMT-9b.2 packs small records into shared pages. These tests are about
/// page reuse one record at a time, so each record here is bigger than a
/// packed page and owns its own run of pages (a span), as in v8.
fn content(i: usize) -> String {
    format!("memory number {i} {}", "x".repeat(8_200))
}

fn mem(i: usize) -> Memory {
    Memory::new(
        content(i),
        MemoryType::Semantic,
        vec![1.0 + i as f32, 1.0, 0.5, 0.25],
    )
}

fn read_page(path: &std::path::Path, page: u64) -> Vec<u8> {
    let b = std::fs::read(path).unwrap();
    b[page as usize * PAGE..(page as usize + 1) * PAGE].to_vec()
}

fn write_page(path: &std::path::Path, page: u64, bytes: &[u8]) {
    let mut b = std::fs::read(path).unwrap();
    b[page as usize * PAGE..(page as usize + 1) * PAGE].copy_from_slice(bytes);
    std::fs::write(path, b).unwrap();
}

/// Recompute the v9 header CRC (bytes 508..512 over 0..508) the way an
/// attacker with write access would, so a test reaches the seal.
fn recrc_v9(path: &std::path::Path) {
    let mut b = std::fs::read(path).unwrap();
    let crc = crc32(&b[0..508]);
    b[508..512].copy_from_slice(&crc.to_le_bytes());
    std::fs::write(path, b).unwrap();
}

/// CRC-32 (IEEE), bitwise; matches `wal::checksum`.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Pages of every live record, for "did the allocator reuse a page" checks.
fn record_pages(db: &Mnemo, ids: &[Ulid]) -> Vec<u64> {
    ids.iter()
        .filter_map(|id| db.__record_location(id))
        .flat_map(|(p, c, _)| p..p + c as u64)
        .collect()
}

#[test]
fn v9_round_trips_through_close_and_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    assert_eq!(db.__format_version(), 9);
    let ids: Vec<Ulid> = (0..12).map(|i| db.remember(mem(i)).unwrap()).collect();
    db.close().unwrap();
    drop(db);

    // The header is a v9 page: version 9, and nothing past byte 512.
    let page0 = read_page(&path, 0);
    assert_eq!(u16::from_le_bytes([page0[4], page0[5]]), 9);
    assert!(page0[512..].iter().all(|&b| b == 0));

    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.__format_version(), 9);
    assert_eq!(db.len(), 12);
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(db.get(id).unwrap().content, content(i));
        let (_, _, gen) = db.__record_location(id).unwrap();
        assert!(gen >= 1, "v9 records carry a generation");
    }
    assert_eq!(db.verify().unwrap(), 12);
    drop(db);
    let mut ro = Mnemo::open_read_only(&path, "pw").unwrap();
    assert_eq!(ro.verify().unwrap(), 12);
}

#[test]
fn v8_is_untouched_no_reuse_no_free_list() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(8)).unwrap();
    db.set_max_snapshots(1);
    let ids: Vec<Ulid> = (0..10).map(|i| db.remember(mem(i)).unwrap()).collect();
    db.flush().unwrap();
    for id in &ids[..5] {
        db.delete(id).unwrap();
    }
    db.flush().unwrap();
    let tail = db.__next_page();
    let fresh: Vec<Ulid> = (10..15).map(|i| db.remember(mem(i)).unwrap()).collect();
    db.flush().unwrap();
    assert_eq!(db.__format_version(), 8);
    assert!(db.__free_list().0.is_empty());
    assert!(record_pages(&db, &fresh).iter().all(|&p| p >= tail));
}

#[test]
fn freed_pages_are_reused_once_unpinned() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    // One retained snapshot: a run freed at txn F is reusable once the
    // snapshot at F is the oldest one left.
    db.set_max_snapshots(1);
    let ids: Vec<Ulid> = (0..20).map(|i| db.remember(mem(i)).unwrap()).collect();
    db.flush().unwrap();
    let old_pages = record_pages(&db, &ids[..10]);
    for id in &ids[..10] {
        db.delete(id).unwrap();
    }
    db.flush().unwrap();
    let (runs, pending) = db.__free_list();
    assert_eq!(pending, 0);
    assert!(!runs.is_empty(), "the deletes freed their pages");
    let tail = db.__next_page();

    let fresh: Vec<Ulid> = (20..30).map(|i| db.remember(mem(i)).unwrap()).collect();
    db.flush().unwrap();
    let reused: Vec<u64> = record_pages(&db, &fresh)
        .into_iter()
        .filter(|p| old_pages.contains(p))
        .collect();
    assert!(
        !reused.is_empty(),
        "new records reuse the deleted records' pages"
    );
    assert!(
        db.__next_page() < tail + 10,
        "the file grew less than 10 pages"
    );
    assert_eq!(db.verify().unwrap(), 20);
    db.close().unwrap();
    drop(db);

    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.len(), 20);
    for (i, id) in ids[10..].iter().chain(fresh.iter()).enumerate() {
        assert_eq!(db.get(id).unwrap().content, content(i + 10));
    }
    assert_eq!(db.verify().unwrap(), 20);
}

#[test]
fn retained_snapshots_pin_freed_pages_and_restore_stays_intact() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    // An explicit count keeps exactly that many snapshots (v9's default
    // policy would let its pin cap drop the pre-delete one).
    db.set_max_snapshots(256);
    // Default retention (many snapshots): nothing freed is reusable yet.
    let ids: Vec<Ulid> = (0..10).map(|i| db.remember(mem(i)).unwrap()).collect();
    db.flush().unwrap();
    let before_delete = db.snapshots().last().unwrap().txn_id;
    let old_pages = record_pages(&db, &ids);
    for id in &ids {
        db.delete(id).unwrap();
    }
    db.flush().unwrap();
    let fresh: Vec<Ulid> = (10..20).map(|i| db.remember(mem(i)).unwrap()).collect();
    db.flush().unwrap();
    assert!(
        record_pages(&db, &fresh)
            .iter()
            .all(|p| !old_pages.contains(p)),
        "pages a retained snapshot references are not reused"
    );

    // Restore to before the deletes: the old records come back intact, and
    // come off the free list.
    db.restore_to(before_delete).unwrap();
    assert_eq!(db.len(), 10);
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(db.get(id).unwrap().content, content(i));
    }
    let (runs, _) = db.__free_list();
    for (_, p, c) in runs {
        assert!(
            old_pages.iter().all(|&q| q < p || q >= p + c),
            "a restored record's page is still on the free list"
        );
    }
    assert_eq!(db.verify().unwrap(), 10);

    // Now drop the history and churn: the restored records must survive
    // every reuse that follows.
    db.set_max_snapshots(1);
    for round in 0..4 {
        let tmp: Vec<Ulid> = (0..8)
            .map(|i| db.remember(mem(100 + round * 10 + i)).unwrap())
            .collect();
        db.flush().unwrap();
        for id in &tmp {
            db.delete(id).unwrap();
        }
        db.flush().unwrap();
    }
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(db.get(id).unwrap().content, content(i));
    }
    assert_eq!(db.verify().unwrap(), 10);
    db.close().unwrap();
    drop(db);
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.verify().unwrap(), 10);
}

#[test]
fn rollback_of_a_reused_page_fails_authentication() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    db.set_max_snapshots(1);
    let victim = db.remember(mem(0)).unwrap();
    let keep = db.remember(mem(1)).unwrap();
    db.close().unwrap();
    let (vp, vc, vgen) = db.__record_location(&victim).unwrap();
    assert_eq!(vc, 2, "a span of two pages");
    drop(db);
    // The attacker's old copy of the victim's page.
    let stale = read_page(&path, vp);

    let mut db = Mnemo::open(&path, "pw").unwrap();
    db.set_max_snapshots(1);
    db.delete(&victim).unwrap();
    db.flush().unwrap();
    // Write until a new record lands on the victim's old page.
    let mut landed = None;
    for i in 2..40 {
        let id = db.remember(mem(i)).unwrap();
        db.flush().unwrap();
        if let Some((p, _, gen)) = db.__record_location(&id) {
            if p == vp {
                assert!(gen > vgen, "a reused page gets a newer generation");
                landed = Some(id);
                break;
            }
        }
    }
    let landed = landed.expect("the victim's page was reused");
    db.close().unwrap();
    drop(db);

    // Untampered, the new record reads fine.
    let mut db = Mnemo::open(&path, "pw").unwrap();
    db.get(&landed).unwrap();
    drop(db);

    // Put the page's previous ciphertext back. Its tag is valid for this
    // page number (that's all v8's AAD binds), but not for the generation
    // the catalog row now expects.
    write_page(&path, vp, &stale);
    let mut db = Mnemo::open(&path, "pw").unwrap();
    db.get(&keep).unwrap();
    match db.get(&landed) {
        Err(MnemoError::PageAuthFailed(p)) => assert_eq!(p, vp),
        other => panic!("expected PageAuthFailed({vp}), got {other:?}"),
    }
}

#[test]
fn v9_header_seal_covers_every_field() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    db.remember(mem(0)).unwrap();
    db.close().unwrap();
    drop(db);
    let pristine = std::fs::read(&path).unwrap();

    // dimensions (14), the KDF m_cost (74), a free-list pointer byte (326),
    // a key-slot pointer byte (358): none of them is covered by v8's seal.
    for off in [14usize, 74, 326, 358] {
        std::fs::write(&path, &pristine).unwrap();
        let mut b = std::fs::read(&path).unwrap();
        b[off] ^= 0x01;
        std::fs::write(&path, b).unwrap();
        recrc_v9(&path);
        match Mnemo::open(&path, "pw") {
            Err(MnemoError::HeaderTampered) => {}
            // m_cost feeds the KDF before the seal is checked, so a changed
            // cost can also surface as a wrong passphrase or a KDF error.
            Err(MnemoError::WrongPassphrase) | Err(MnemoError::Kdf(_)) if off == 74 => {}
            other => panic!("byte {off}: expected HeaderTampered, got {:?}", other.err()),
        }
    }
    std::fs::write(&path, &pristine).unwrap();
    Mnemo::open(&path, "pw").unwrap();
}

#[test]
fn key_slot_run_is_persisted_copy_on_write_and_authenticated() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    db.set_max_snapshots(1);
    db.remember(mem(0)).unwrap();
    db.__add_key_slot_for_testing("laptop").unwrap();
    db.close().unwrap();
    let (p1, n1, g1) = db.__key_slot_run();
    assert!(n1 > 0);
    drop(db);

    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.__key_slot_labels(), vec!["laptop".to_string()]);
    db.__add_key_slot_for_testing("recovery").unwrap();
    db.close().unwrap();
    let (p2, n2, g2) = db.__key_slot_run();
    assert!(g2 > g1, "the run is rewritten under a newer generation");
    assert!((p2, n2) != (p1, n1) || g2 != g1);
    // The superseded run went onto the free list.
    let (runs, _) = db.__free_list();
    assert!(runs.iter().any(|&(_, p, c)| p <= p1 && p1 < p + c));
    drop(db);

    let db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(
        db.__key_slot_labels(),
        vec!["laptop".to_string(), "recovery".to_string()]
    );
    drop(db);

    // A v8 file has no key-slot run.
    let path8 = dir.path().join("v8.mnemo");
    let mut db8 = Mnemo::create(&path8, "pw", cfg(8)).unwrap();
    assert!(db8.__add_key_slot_for_testing("x").is_err());

    // Tampering with the run's page fails the open.
    let mut page = read_page(&path, p2);
    page[40] ^= 1;
    write_page(&path, p2, &page);
    match Mnemo::open(&path, "pw") {
        Err(MnemoError::PageAuthFailed(p)) => assert_eq!(p, p2),
        other => panic!("expected PageAuthFailed, got {:?}", other.err()),
    }
}

#[test]
fn a_relocated_wal_region_is_freed() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(
        &path,
        "pw",
        MnemoConfig {
            wal_pages_initial: 2,
            ..cfg(9)
        },
    )
    .unwrap();
    // Enough records that the catalog outgrows a 2-page WAL.
    for i in 0..600 {
        db.remember(mem(i)).unwrap();
    }
    db.flush().unwrap();
    let (runs, _) = db.__free_list();
    assert!(
        runs.iter().any(|&(_, p, c)| p <= 1 && 1 < p + c),
        "the original WAL region (page 1) is on the free list: {runs:?}"
    );
    assert_eq!(db.verify().unwrap(), 600);
}

#[test]
fn compacting_a_v9_file_keeps_v9() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg(9)).unwrap();
    let ids: Vec<Ulid> = (0..6).map(|i| db.remember(mem(i)).unwrap()).collect();
    db.delete(&ids[0]).unwrap();
    db.close().unwrap();
    drop(db);
    let report = Mnemo::compact_file(path.to_str().unwrap(), "pw").unwrap();
    assert_eq!(report.after, 5);
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.__format_version(), 9);
    assert_eq!(db.verify().unwrap(), 5);
}

#[test]
fn unsupported_format_version_is_refused_at_create() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    assert!(matches!(
        Mnemo::create(&path, "pw", cfg(7)),
        Err(MnemoError::Invalid(_))
    ));
}

/// FMT-9 (a) crash tests: these need the failpoint hooks.
#[cfg(feature = "failpoints")]
mod crash {
    use super::*;

    /// A file with a committed free list whose runs are reusable: 20
    /// records, 10 deleted, one retained snapshot. Returns the survivors.
    fn setup(path: &std::path::Path) -> (Vec<Ulid>, Vec<u64>) {
        let mut db = Mnemo::create(path, "pw", cfg(9)).unwrap();
        db.set_max_snapshots(1);
        let ids: Vec<Ulid> = (0..20).map(|i| db.remember(mem(i)).unwrap()).collect();
        db.flush().unwrap();
        let freed = record_pages(&db, &ids[..10]);
        for id in &ids[..10] {
            db.delete(id).unwrap();
        }
        db.close().unwrap();
        (ids[10..].to_vec(), freed)
    }

    /// The crash between the data-page writes and `wal::commit`, after the
    /// new records were written into freed (reused) pages: reopening lands
    /// on the previous state, and every page it references authenticates.
    #[test]
    fn crash_after_reused_data_pages_before_commit_lands_on_previous_state() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.mnemo");
        let (survivors, freed) = setup(&path);

        let mut db = Mnemo::open(&path, "pw").unwrap();
        db.set_max_snapshots(1);
        let fresh: Vec<Ulid> = (20..25).map(|i| db.remember(mem(i)).unwrap()).collect();
        let into_freed = record_pages(&db, &fresh)
            .iter()
            .filter(|p| freed.contains(p))
            .count();
        assert!(into_freed > 0, "the new records were placed in freed pages");
        // 5 single-page records: let those 5 data writes through, fail the
        // first WAL write.
        mnemo_engine::__failpoints::set_writes_until_fail(5);
        let res = db.flush();
        mnemo_engine::__failpoints::set_writes_until_fail(-1);
        assert!(res.is_err(), "the injected failure aborts the commit");
        drop(db);

        let mut db = Mnemo::open(&path, "pw").unwrap();
        assert_eq!(db.len(), 10, "the uncommitted records are gone");
        for id in &survivors {
            db.get(id).unwrap();
        }
        for id in &fresh {
            assert!(db.get(id).is_err());
        }
        assert_eq!(db.verify().unwrap(), 10);
        // And the file is still writable, reusing the same pages.
        db.set_max_snapshots(1);
        for i in 30..35 {
            db.remember(mem(i)).unwrap();
        }
        db.close().unwrap();
        drop(db);
        let mut db = Mnemo::open(&path, "pw").unwrap();
        assert_eq!(db.verify().unwrap(), 15);
    }

    /// Fail every write of one commit in turn: whatever survives is the
    /// pre- or the post-commit state, its free list is exactly that
    /// state's, and nothing on the list overlaps a live page. Covers the
    /// crash after the commit and before the checkpoint, where the free
    /// list comes back by WAL replay.
    #[test]
    fn free_list_is_pre_or_post_at_every_crash_point() {
        let dir = tempdir().unwrap();
        let base = dir.path().join("base.mnemo");
        setup(&base);

        // Every step can hit the failpoint (the session's first write
        // takes the generation lease, a small commit of its own).
        let ops = |db: &mut Mnemo| -> mnemo_engine::Result<()> {
            db.set_max_snapshots(1);
            let ids = (40..46)
                .map(|i| db.remember(mem(i)))
                .collect::<mnemo_engine::Result<Vec<Ulid>>>()?;
            db.delete(&ids[0])?;
            db.flush()
        };
        let free_of = |p: &std::path::Path| {
            let mut db = Mnemo::open(p, "pw").unwrap();
            let n = db.verify().unwrap();
            (db.len(), n, db.__free_list().0)
        };
        let pre = free_of(&base);
        let post = {
            let p = dir.path().join("post.mnemo");
            std::fs::copy(&base, &p).unwrap();
            let mut db = Mnemo::open(&p, "pw").unwrap();
            ops(&mut db).unwrap();
            drop(db);
            free_of(&p)
        };
        assert_ne!(pre.0, post.0);

        let mut saw_post_by_replay = false;
        for fail_at in 0..60 {
            let p = dir.path().join(format!("crash-{fail_at}.mnemo"));
            std::fs::copy(&base, &p).unwrap();
            let mut db = Mnemo::open(&p, "pw").unwrap();
            mnemo_engine::__failpoints::set_writes_until_fail(fail_at);
            let res = ops(&mut db);
            mnemo_engine::__failpoints::set_writes_until_fail(-1);
            drop(db);
            let got = free_of(&p);
            assert!(
                got == pre || got == post,
                "fail_at {fail_at}: state is neither pre nor post: {got:?}"
            );
            if res.is_err() && got == post {
                saw_post_by_replay = true;
            }
            if res.is_ok() {
                break;
            }
        }
        assert!(
            saw_post_by_replay,
            "some crash point fell after the commit and was replayed"
        );
    }
}
