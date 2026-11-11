//! INSP-1 step 4 (RB-001.25): `Stats` and `StatsSummary` report the restore
//! points that exist (the retained snapshot count, the oldest restorable
//! time and the bytes the snapshots pin), from what `snapshots()` holds
//! rather than from the retention policy.

use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig};
use std::path::Path;

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn v(i: usize) -> Vec<f32> {
    vec![(i as f32) + 1.0, 0.5, (i % 3) as f32, 1.0]
}

/// Both stats paths agree with `snapshots()`.
fn check_against_snapshots(db: &mut Mnemo) -> (usize, Option<i64>, u64) {
    let snaps = db.snapshots();
    let sum = db.stats_summary();
    let full = db.stats().unwrap();
    assert_eq!(sum.snapshots_retained, snaps.len());
    assert_eq!(
        sum.oldest_restorable_at,
        snaps.first().map(|s| s.created_at)
    );
    assert_eq!(
        (
            full.snapshots_retained,
            full.oldest_restorable_at,
            full.snapshot_pinned_bytes
        ),
        (
            sum.snapshots_retained,
            sum.oldest_restorable_at,
            sum.snapshot_pinned_bytes
        )
    );
    (
        sum.snapshots_retained,
        sum.oldest_restorable_at,
        sum.snapshot_pinned_bytes,
    )
}

#[test]
fn the_fields_follow_the_retained_snapshots_under_churn() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg()).unwrap();
    let (n, oldest, _) = check_against_snapshots(&mut db);
    assert_eq!(n == 0, oldest.is_none());

    let mut ids = Vec::new();
    for i in 0..60 {
        ids.push(
            db.remember(Memory::new(format!("note {i}"), MemoryType::Semantic, v(i)))
                .unwrap(),
        );
    }
    db.flush().unwrap();
    // Churn: each commit deletes some memories and adds others, freeing
    // pages that the older snapshots keep from reuse.
    for round in 0..6 {
        for id in ids.drain(..10) {
            db.delete(&id).unwrap();
        }
        for i in 0..10 {
            let k = 100 + round * 10 + i;
            ids.push(
                db.remember(Memory::new(
                    format!("churn {k}"),
                    MemoryType::Semantic,
                    v(k),
                ))
                .unwrap(),
            );
        }
        db.flush().unwrap();
    }
    let (n, oldest, pinned) = check_against_snapshots(&mut db);
    assert!(n >= 2, "{n}");
    assert!(oldest.is_some());
    assert!(pinned > 0, "older snapshots pin the pages the churn freed");
    assert_eq!(pinned % 8192, 0, "whole pages");

    // Keep only the newest: it pins nothing (what the last commit freed is
    // stamped with its own transaction).
    db.set_max_snapshots(1);
    db.remember(Memory::new("one more", MemoryType::Semantic, v(7)))
        .unwrap();
    db.flush().unwrap();
    let (n, oldest, pinned) = check_against_snapshots(&mut db);
    assert_eq!((n, pinned), (1, 0));
    assert_eq!(oldest, db.snapshots().last().map(|s| s.created_at));
    db.close().unwrap();

    // A read-only handle reports the same.
    let mut ro = Mnemo::open_read_only(&path, "pw").unwrap();
    assert_eq!(check_against_snapshots(&mut ro).0, 1);
}

#[test]
fn compaction_resets_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.mnemo");
    let mut db = Mnemo::create(&path, "pw", cfg()).unwrap();
    for i in 0..5 {
        let id = db
            .remember(Memory::new(format!("x {i}"), MemoryType::Semantic, v(i)))
            .unwrap();
        db.flush().unwrap();
        if i % 2 == 0 {
            db.delete(&id).unwrap();
            db.flush().unwrap();
        }
    }
    assert!(check_against_snapshots(&mut db).0 > 1);
    db.close().unwrap();
    Mnemo::compact_file(path.to_str().unwrap(), "pw").unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    let (n, oldest, pinned) = check_against_snapshots(&mut db);
    assert!(n <= 1, "{n}");
    assert_eq!(n == 1, oldest.is_some());
    assert_eq!(pinned, 0);
}

#[test]
fn a_v8_file_reports_its_snapshots_and_pins_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v8.mnemo");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v8/v8_passphrase.mnemo"),
        &path,
    )
    .unwrap();
    let mut db = Mnemo::open_read_only(&path, "v8-fixture-passphrase").unwrap();
    let (n, oldest, pinned) = check_against_snapshots(&mut db);
    assert_eq!(n, 7, "expected.json: 7 snapshots");
    assert!(oldest.is_some());
    assert_eq!(pinned, 0, "v8 never reuses pages");
}
