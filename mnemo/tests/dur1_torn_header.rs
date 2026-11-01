//! DUR-1 — torn-header recovery after WAL-logged header commits.
//!
//! Every post-`create` page-0 write (WAL relocation, rekey) now goes
//! through the WAL first: commit a one-frame transaction `(0, header)`
//! into the current WAL region, checkpoint page 0, and only then
//! invalidate the predecessor region. On reopen, a page-0 that fails to
//! **parse** (`BadMagic`, `UnsupportedVersion`, `HeaderChecksum`)
//! triggers a file-wide streaming scan for valid committed header
//! frames; the highest-`txn_id` candidate that unwraps the DEK and
//! validates the seal under the passphrase replaces page 0.
//!
//! Per the Planner's 2026-11-01 Option B decision, a `validate_seal`
//! failure is **always** reported as `HeaderTampered` and never
//! auto-healed from the WAL (healing would erase tamper-evidence; a
//! real crash cannot produce "CRC valid + seal invalid" because the
//! full 294-byte header lies inside one 512-byte sector).
//!
//! The tests here cover the sector-level crash + tamper shapes the
//! spec calls out (R-002.1 part 4):
//!
//! * sector 0 zeroed
//! * sector 0 random garbage
//! * bytes 0..6 corrupted (MAGIC / version)
//! * bytes 242..294 stale with CRC intact → `HeaderTampered`
//! * sealed field altered + CRC recomputed → `HeaderTampered`
//! * torn page 0 **during** a WAL relocation
//! * torn page 0 **after** a WAL relocation (zeroed sector, >64-page
//!   relocation)
//! * torn page 0 during `rekey`, including on a never-flushed file
//!
//! The failpoint-driven tests arm the pager's `WRITES_UNTIL_FAIL`
//! counter so a specific write in the relocation / rekey sequence
//! raises an `io::Error`. They assert that reopen *always* recovers to a
//! consistent state — either the pre-op state or the post-op state,
//! never a third state and never a "DEK is gone" state.
//!
//! Requires the `failpoints` feature. Guarded test-side by
//! `#[cfg(any(test, feature = "failpoints"))]` to match the engine.

#![cfg(any(test, feature = "failpoints"))]

use mnemo::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig};
use tempfile::tempdir;

fn fast_cfg(dimensions: usize) -> MnemoConfig {
    MnemoConfig {
        dimensions,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn mem(i: usize) -> Memory {
    Memory::new(
        format!("m-{i}"),
        MemoryType::Semantic,
        vec![i as f32, 1.0, 2.0, 3.0],
    )
}

fn read_bytes(p: &std::path::Path) -> Vec<u8> {
    std::fs::read(p).unwrap()
}
fn write_bytes(p: &std::path::Path, b: &[u8]) {
    std::fs::write(p, b).unwrap()
}

// ---- sector-level corruption of page 0 ----------------------------------

#[test]
fn sector_0_zeroed_heals_from_wal() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    for i in 0..6 {
        db.remember(mem(i)).unwrap();
    }
    db.close().unwrap();
    drop(db);

    // Zero the first 512-byte sector (which contains the entire 294-byte
    // header). Everything past the sector is untouched.
    let mut bytes = read_bytes(&path);
    for b in bytes.iter_mut().take(512) {
        *b = 0;
    }
    write_bytes(&path, &bytes);

    let db = Mnemo::open(&path, "pw").expect("zeroed sector-0 must heal");
    assert_eq!(db.len(), 6);
}

#[test]
fn sector_0_random_garbage_heals_from_wal() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    for i in 0..8 {
        db.remember(mem(i)).unwrap();
    }
    db.close().unwrap();
    drop(db);

    // Overwrite the first sector with a deterministic non-zero pattern
    // that doesn't collide with the real MAGIC.
    let mut bytes = read_bytes(&path);
    for (i, b) in bytes.iter_mut().take(512).enumerate() {
        *b = (i as u8).wrapping_mul(37).wrapping_add(13);
    }
    write_bytes(&path, &bytes);

    let db = Mnemo::open(&path, "pw").expect("garbaged sector-0 must heal");
    assert_eq!(db.len(), 8);
}

#[test]
fn header_magic_and_version_bytes_corrupted_heal() {
    // Bytes 0..4 are MAGIC, 4..6 are the version. Flipping them trips
    // `BadMagic` / `UnsupportedVersion` before the CRC gate, which is
    // exactly the pre-DUR-1 blind spot (the old recovery only engaged
    // on `HeaderChecksum`).
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    for i in 0..5 {
        db.remember(mem(i)).unwrap();
    }
    db.close().unwrap();
    drop(db);

    let mut bytes = read_bytes(&path);
    for b in bytes.iter_mut().take(6) {
        *b ^= 0xFF;
    }
    write_bytes(&path, &bytes);

    let db = Mnemo::open(&path, "pw").expect("corrupted magic/version must heal");
    assert_eq!(db.len(), 5);
}

#[test]
fn header_seal_fields_stale_report_tampering() {
    // Bytes 242..294 hold the v7 seal (nonce+tag) and v8 cache
    // pointers. The CRC covers 0..238, so flipping bytes in 242..294
    // leaves the CRC intact — `Header::from_page` succeeds and
    // `validate_seal` is the gate that catches the tamper.
    //
    // DUR-1 (Planner decision 2026-11-01, Option B): a seal failure
    // is **always** reported as tampering and never auto-healed from
    // the WAL. The whole 294-byte header lies inside sector 0, which
    // storage writes atomically — a real torn write breaks magic,
    // version or CRC, not just the seal, so this state can only be
    // post-write tampering.
    use mnemo::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    for i in 0..4 {
        db.remember(mem(i)).unwrap();
    }
    db.close().unwrap();
    drop(db);

    let mut bytes = read_bytes(&path);
    // Flip one bit in the seal tag (inside 242..294, outside the CRC
    // range). The CRC still verifies, so we exercise the seal path.
    bytes[260] ^= 0x01;
    write_bytes(&path, &bytes);

    match Mnemo::open(&path, "pw") {
        Err(MnemoError::HeaderTampered) => {}
        Err(other) => panic!("expected HeaderTampered, got {other:?}"),
        Ok(_) => panic!("expected HeaderTampered; silent heal would erase evidence"),
    }
}

#[test]
fn header_sealed_field_altered_with_recomputed_crc_reports_tampering() {
    // Harder tamper: alter a CRC-covered sealed field (here
    // `catalog_start` at offset 42), recompute the header CRC so
    // `Header::from_page` succeeds, and confirm the AES-GCM seal
    // still catches it. This is the scenario an attacker who learned
    // the CRC algorithm would try — the seal (keyed MAC) is what
    // actually keeps the file honest, and Option B says never auto-
    // heal around it.
    use mnemo::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    for i in 0..4 {
        db.remember(mem(i)).unwrap();
    }
    db.close().unwrap();
    drop(db);

    let mut bytes = read_bytes(&path);
    // `catalog_start` is a u64 at offset 42 (inside the 0..238 CRC
    // range). Flip a bit so it points at a nonsense page.
    bytes[42] ^= 0x01;
    // Recompute and rewrite the CRC over bytes 0..238 so the parser
    // waves it through to seal validation.
    let new_crc = mnemo_crc32_adapter(&bytes[0..238]).to_le_bytes();
    bytes[238..242].copy_from_slice(&new_crc);
    write_bytes(&path, &bytes);

    match Mnemo::open(&path, "pw") {
        Err(MnemoError::HeaderTampered) => {}
        Err(other) => panic!("expected HeaderTampered, got {other:?}"),
        Ok(_) => panic!("CRC-recomputed sealed-field tamper slipped past the seal"),
    }
}

/// Re-implements the WAL CRC-32 so the test file can rewrite the header
/// CRC after mutating earlier bytes. Kept in-test (not a public mnemo
/// API) because it only exists to drive the HeaderTampered path with a
/// crafted-but-CRC-valid payload.
fn mnemo_crc32_adapter(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            let mask = (c & 1).wrapping_neg();
            c = (c >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !c
}

// ---- large relocation ---------------------------------------------------

#[test]
fn relocated_txn_larger_than_64_pages_heals_torn_page_0() {
    // Build a catalog that forces the WAL region to grow past the old
    // 64-page recovery window, then zero page 0 so reopen must heal
    // from the relocated WAL region sitting at the file tail. The
    // pre-DUR-1 scan would have stopped at page 1 after 64 pages and
    // never found this region; the streaming file-wide scan does.
    let dir = tempdir().unwrap();
    let path = dir.path().join("big.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(2)).unwrap();
    for i in 0..25_000 {
        db.remember(Memory::new(
            format!("rec-{i}"),
            MemoryType::Semantic,
            vec![((i % 101) + 1) as f32, ((i % 97) + 1) as f32],
        ))
        .unwrap();
    }
    // Flush explicitly so `ensure_wal_capacity` runs and the WAL
    // relocation actually happens before we snapshot `wal_pages`.
    // `stats()` reads from the in-memory header, which only reflects
    // the grown region after a flush — `close() + drop` would also
    // flush, but then we couldn't observe `wal_pages` to assert the
    // test is meaningful.
    db.flush().unwrap();
    let wal_pages_before = db.stats().unwrap().wal_pages;
    db.close().unwrap();
    drop(db);
    assert!(
        wal_pages_before > 64,
        "the WAL must have grown past 64 pages for this test to be meaningful \
         (observed {wal_pages_before} pages)"
    );

    // Zero the first 512-byte sector — real-world torn-write shape.
    let mut bytes = read_bytes(&path);
    for b in bytes.iter_mut().take(512) {
        *b = 0;
    }
    write_bytes(&path, &bytes);

    let mut db = Mnemo::open(&path, "pw")
        .expect("torn page 0 with the WAL at the file tail must heal via the streaming scan");
    let stats = db.stats().unwrap();
    assert_eq!(stats.memories, 25_000);
    assert_eq!(stats.wal_pages, wal_pages_before);
}

// ---- failpoint-driven relocation / rekey crashes ------------------------
//
// The spec requires: a torn page 0 during a relocation or rekey must
// recover to the pre-op state OR the post-op state, never a third state
// and never a "DEK lost" state. We sweep the pager's write counter so
// different `fail_at` values land at different points in the sequence
// (WAL commit write / page-0 overwrite / old-region invalidation), and
// assert the invariant for every one.

fn disarm() {
    mnemo::__failpoints::set_writes_until_fail(-1);
}

/// Build a database just under the current WAL capacity, so the very
/// next flush is guaranteed to trigger a `ensure_wal_capacity`
/// relocation. Returns the handle (open) plus the pre-relocation
/// memory count.
fn seed_up_to_relocation(path: &std::path::Path) -> Mnemo {
    let mut db = Mnemo::create(path, "pw", fast_cfg(2)).unwrap();
    // ~1200 small records keeps the first flush inside the default WAL
    // (and relies on the second large flush to trigger relocation).
    for i in 0..1200 {
        db.remember(Memory::new(
            format!("seed-{i}"),
            MemoryType::Semantic,
            vec![((i % 7) + 1) as f32, ((i % 11) + 1) as f32],
        ))
        .unwrap();
    }
    db.flush().unwrap();
    db
}

#[test]
fn relocation_crash_recovers_to_pre_or_post_state() {
    // Sweep fail_at over the plausible writes inside a relocating
    // flush. Each sub-case runs a fresh database.
    for fail_at in 0i64..32 {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.mnemo");
        let mut db = seed_up_to_relocation(&path);
        let pre = db.len();

        // Stage a large transaction so the next flush must relocate the
        // WAL to accommodate it.
        for i in 0..20_000 {
            db.remember(Memory::new(
                format!("post-{i}"),
                MemoryType::Semantic,
                vec![((i % 13) + 1) as f32, ((i % 17) + 1) as f32],
            ))
            .unwrap();
        }
        let post = pre + 20_000;

        mnemo::__failpoints::set_writes_until_fail(fail_at);
        let flush_result = db.flush();
        disarm();
        drop(db);

        // Reopen must succeed — recovery is deterministic.
        let db = Mnemo::open(&path, "pw")
            .unwrap_or_else(|e| panic!("reopen after fail_at={fail_at} failed: {e:?}"));
        let observed = db.len();
        assert!(
            observed == pre || observed == post,
            "fail_at={fail_at}: observed={observed}, pre={pre}, post={post}, \
             flush_result={flush_result:?}"
        );
    }
}

#[test]
fn rekey_crash_recovers_to_old_or_new_passphrase() {
    for fail_at in 0i64..16 {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.mnemo");
        let mut db = Mnemo::create(&path, "old-pw", fast_cfg(4)).unwrap();
        for i in 0..5 {
            db.remember(mem(i)).unwrap();
        }
        db.flush().unwrap();

        mnemo::__failpoints::set_writes_until_fail(fail_at);
        let _ = db.rekey("new-pw", KdfParams::fast());
        disarm();
        drop(db);

        // At least one of the two passphrases must open the file.
        let opened = match Mnemo::open(&path, "new-pw") {
            Ok(h) => Some(("new", h)),
            Err(_) => Mnemo::open(&path, "old-pw").ok().map(|h| ("old", h)),
        };
        let (which, db) = opened
            .unwrap_or_else(|| panic!("rekey crash at fail_at={fail_at} left no openable file"));
        assert_eq!(db.len(), 5, "rekey crash ({which}) lost memories");
    }
}

#[test]
fn rekey_crash_on_never_flushed_file_keeps_dek() {
    // "never-flushed" variant: open create path, immediately rekey —
    // the on-disk state is just the header plus the (empty) initial WAL.
    for fail_at in 0i64..8 {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.mnemo");
        let mut db = Mnemo::create(&path, "old-pw", fast_cfg(4)).unwrap();

        mnemo::__failpoints::set_writes_until_fail(fail_at);
        let _ = db.rekey("new-pw", KdfParams::fast());
        disarm();
        drop(db);

        // One of the two passphrases must still open the file. (The
        // DEK itself is unchanged across rekey; only its wrapping
        // moves, so losing both means we lost the DEK — the invariant
        // the spec cares about.)
        let opened = Mnemo::open(&path, "new-pw").or_else(|_| Mnemo::open(&path, "old-pw"));
        assert!(
            opened.is_ok(),
            "never-flushed rekey crash at fail_at={fail_at} lost the DEK"
        );
    }
}
