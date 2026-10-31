//! PR SEC-1 (v0.5.2): page-nonce uniqueness invariants.
//!
//! Before SEC-1, page nonces were `page_no || write_counter` — unique
//! within a file but **reused across copies of a file**. If you copied
//! a `.mnemo` file to another host and both the original and the copy
//! got written to (two laptops syncing a file; a restored older backup
//! resuming operation), each copy's writer produced pages whose nonces
//! collided with the other's at the same `(page_no, write_counter)`
//! coordinates, under the same DEK (`random_dek()` is only called at
//! create). That breaks AES-GCM confidentiality (XOR of plaintexts
//! recoverable) and tamper-evidence (GHASH key recoverable → page
//! forgery possible).
//!
//! The fix is a per-session random prefix: every writable open draws
//! a fresh 64-bit `session_random`, and the nonce becomes
//! `session_random || session_counter`. Cross-session collisions now
//! require two sessions to independently draw the same 64-bit random
//! value (negligible). This file pins the three invariants the planner
//! asked for:
//!
//! 1. **No nonce in both copies.** Fork a file, write to both forks,
//!    scan every page in both — the raw-nonce sets must be disjoint.
//! 2. **Nonces differ across reopen sessions.** Open, write, close,
//!    reopen, write — the two sessions' nonces must not overlap.
//! 3. **v0.5.1 fixtures still open.** A file produced by the old
//!    deterministic-nonce code must still decrypt, since decrypt reads
//!    the stored nonce rather than recomputing one. (Simulated here by
//!    using the current code to produce a file and reopening it — the
//!    on-disk schema didn't change in SEC-1, so the equivalence is
//!    structural, not just empirical.)

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom};

use mnemo::{Memory, MemoryType, Mnemo, MnemoConfig};
use tempfile::TempDir;

const PAGE_SIZE: usize = 8192;
const NONCE_LEN: usize = 12;

fn fast_cfg(dims: usize) -> MnemoConfig {
    MnemoConfig {
        dimensions: dims,
        kdf: mnemo::KdfParams::fast(),
        ..Default::default()
    }
}

/// WAL frame magic. A page starting with these four bytes is a WAL
/// frame, not an encrypted data page — its first 12 bytes are
/// `MAGIC(4) || kind(1) || txn_id(8)...`, not an AES-GCM nonce.
/// Skipped by [`collect_nonces`] so the test doesn't compare frame
/// headers across copies (two files recovered from the same state
/// will naturally share identical frame headers at the same offsets).
const WAL_FRAME_MAGIC: &[u8; 4] = b"MWAL";

/// Collect every encrypted page's 12-byte nonce from a `.mnemo` file.
///
/// Skip:
/// - Page 0 (plaintext header, not AES-GCM).
/// - WAL frame slots whose first 4 bytes are `MWAL` — these are
///   log-frame headers, not nonces. Two files that share WAL frames
///   (e.g. one is a `cp` of the other) will have identical frame
///   headers at the same positions; that's structural, not a nonce
///   reuse.
/// - All-zero page slots (unwritten space in the WAL region or at
///   the file tail).
///
/// Everything else is a nonce from an AES-GCM encrypted page.
fn collect_nonces(path: &std::path::Path) -> HashSet<[u8; NONCE_LEN]> {
    let mut file = fs::File::open(path).expect("open for nonce scan");
    let len = file.metadata().expect("stat").len();
    assert!(len >= PAGE_SIZE as u64, "file must have at least one page");
    let page_count = (len / PAGE_SIZE as u64) as usize;

    let mut out = HashSet::new();
    for page_no in 1..page_count {
        file.seek(SeekFrom::Start((page_no as u64) * PAGE_SIZE as u64))
            .expect("seek");
        let mut page = [0u8; PAGE_SIZE];
        if file.read_exact(&mut page).is_err() {
            break;
        }
        if &page[..4] == WAL_FRAME_MAGIC {
            continue;
        }
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&page[..NONCE_LEN]);
        if nonce != [0u8; NONCE_LEN] {
            out.insert(nonce);
        }
    }
    out
}

/// Insert N distinct memories + flush. Caller owns the handle; this
/// just populates it with pages worth encrypting.
fn seed(db: &mut Mnemo, start: usize, count: usize) {
    for i in start..start + count {
        let v = (0..4).map(|k| (i as f32 + k as f32) * 0.123).collect();
        db.remember(Memory::new(format!("mem-{i}"), MemoryType::Semantic, v))
            .expect("remember");
    }
    db.flush().expect("flush");
}

#[test]
fn sec1_no_nonce_appears_in_both_copies() {
    // Property: new pages written to two diverged copies of the same
    // file carry disjoint nonces. This is the exact failure mode SEC-1
    // fixes — before it, pages at the same (page_no, write_counter)
    // coordinates would carry byte-identical nonces in both copies
    // under the same DEK with different plaintexts.
    //
    // Nonces present *before* the fork are shared by construction
    // (byte-for-byte copy of A to B), so the test snapshots those and
    // only compares nonces introduced *after* the fork — those are the
    // ones that would collide under the pre-SEC-1 scheme.
    let td = TempDir::new().unwrap();
    let a = td.path().join("a.mnemo");
    let b = td.path().join("b.mnemo");

    // Create A with some initial content, flush, close.
    {
        let mut db = Mnemo::create(&a, "pw", fast_cfg(4)).unwrap();
        seed(&mut db, 0, 10);
        db.close().unwrap();
    }

    // Snapshot nonces present in A before the fork; these will be
    // duplicated in B by construction (same byte content, same stored
    // nonce). Any nonce in this set is already-shared history, not a
    // reuse violation.
    let pre_fork = collect_nonces(&a);

    // Fork: byte-for-byte copy A -> B. Represents a user running `cp`
    // for backup, a Dropbox sync propagating the file, or a restored
    // snapshot.
    fs::copy(&a, &b).expect("fork copy");

    // Writer A: open the original and append new content.
    {
        let mut db = Mnemo::open(&a, "pw").unwrap();
        seed(&mut db, 100, 10);
        db.close().unwrap();
    }

    // Writer B: open the copy and append its own, different content.
    // Pre-SEC-1, A and B would use the same `(page_no, write_counter)`
    // sequence for their new pages → identical nonces → nonce reuse.
    {
        let mut db = Mnemo::open(&b, "pw").unwrap();
        seed(&mut db, 200, 10);
        db.close().unwrap();
    }

    let na_after = collect_nonces(&a);
    let nb_after = collect_nonces(&b);

    // New nonces introduced in each copy = everything that wasn't in
    // the pre-fork snapshot. These are the writes we care about.
    let a_new: HashSet<_> = na_after.difference(&pre_fork).copied().collect();
    let b_new: HashSet<_> = nb_after.difference(&pre_fork).copied().collect();
    assert!(!a_new.is_empty(), "A must have written new encrypted pages");
    assert!(!b_new.is_empty(), "B must have written new encrypted pages");

    let overlap: Vec<_> = a_new.intersection(&b_new).collect();
    assert!(
        overlap.is_empty(),
        "SEC-1 violation: {} nonce(s) reused across post-fork writes to \
         copies A and B. Overlapping bytes: {:x?}",
        overlap.len(),
        overlap,
    );
}

#[test]
fn sec1_nonces_differ_across_reopen_sessions() {
    // Property: each writable open gets a fresh `session_random`, so
    // the second session's nonces are disjoint from the first's.
    // Pre-SEC-1 this held *per file* via the monotonic `write_counter`;
    // post-SEC-1 the per-session random prefix makes it robust against
    // cross-copy and cross-restore scenarios too.
    let td = TempDir::new().unwrap();
    let path = td.path().join("sessions.mnemo");

    // Session 1.
    {
        let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        seed(&mut db, 0, 10);
        db.close().unwrap();
    }
    let after_s1 = collect_nonces(&path);

    // Session 2 — fresh open on the same file.
    {
        let mut db = Mnemo::open(&path, "pw").unwrap();
        seed(&mut db, 100, 10);
        db.close().unwrap();
    }
    let after_s2 = collect_nonces(&path);

    // The second session wrote additional pages, so after_s2 should be
    // a superset of after_s1 (same pages present with same nonces,
    // plus new ones). What we're asserting here is a specific property
    // of the new scheme: the pages written in session 2 use nonces
    // that start with a fresh random prefix — so **no new nonce in
    // session 2 has the same 8-byte prefix as a nonce from session 1**
    // (except by the astronomical ~2^-64 birthday chance).
    let new_in_s2: HashSet<[u8; NONCE_LEN]> = after_s2.difference(&after_s1).copied().collect();
    assert!(
        !new_in_s2.is_empty(),
        "session 2 should have written at least one new page"
    );
    let s1_prefixes: HashSet<[u8; 8]> = after_s1
        .iter()
        .map(|n| {
            let mut p = [0u8; 8];
            p.copy_from_slice(&n[..8]);
            p
        })
        .collect();
    let s2_prefixes: HashSet<[u8; 8]> = new_in_s2
        .iter()
        .map(|n| {
            let mut p = [0u8; 8];
            p.copy_from_slice(&n[..8]);
            p
        })
        .collect();
    let shared: Vec<_> = s1_prefixes.intersection(&s2_prefixes).collect();
    assert!(
        shared.is_empty(),
        "SEC-1 violation: {} session-random prefix(es) shared across \
         two writable opens of the same file; the second session should \
         have minted a fresh 64-bit prefix. Shared: {:x?}",
        shared.len(),
        shared,
    );
}

#[test]
fn sec1_decrypt_reads_stored_nonce_unchanged_format() {
    // Property: the on-disk schema is unchanged — each page still
    // stores its own 12-byte nonce in bytes [0..12] and decrypt reads
    // it from there. A file produced by the current build must open
    // and read every memory back correctly after reopen, since that's
    // the same code path a v0.5.1 file would exercise.
    let td = TempDir::new().unwrap();
    let path = td.path().join("roundtrip.mnemo");
    let want = 20usize;

    {
        let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        seed(&mut db, 0, want);
        db.close().unwrap();
    }
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.memories().unwrap().len(), want);
}
