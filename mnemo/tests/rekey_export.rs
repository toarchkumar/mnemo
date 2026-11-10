//! Rekey, rotation and the rekeyed export (SEC-2).
//!
//! "Splice" tests write an earlier page 0 (the header, which carries the
//! salt, the KDF costs and the wrapped data key) over a copy of the current
//! file and try the earlier secret. After a plain rekey that still opens,
//! by design (the data key is unchanged); after a rotation or a rekeyed
//! export it must not.

use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig};
use std::path::Path;
use tempfile::tempdir;

const DIMS: usize = 8;
const PAGE: usize = 8192;

fn cfg(version: u16) -> MnemoConfig {
    MnemoConfig {
        dimensions: DIMS,
        kdf: KdfParams::fast(),
        format_version: version,
        ..Default::default()
    }
}

fn mem(i: usize) -> Memory {
    let v: Vec<f32> = (0..DIMS).map(|k| ((i + k) % 5) as f32 + 1.0).collect();
    Memory::new(format!("memory {i}"), MemoryType::Semantic, v)
}

/// A file with `n` memories under passphrase `pw`.
fn make(path: &Path, version: u16, pw: &str, n: usize) {
    let mut db = Mnemo::create(path, pw, cfg(version)).unwrap();
    for i in 0..n {
        db.remember(mem(i)).unwrap();
    }
    db.flush().unwrap();
    db.close().unwrap();
}

/// The header's key wrap: salt, DEK nonce and wrapped DEK (bytes 86..162).
fn wrap_of(file: &[u8]) -> Vec<u8> {
    file[86..162].to_vec()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// `current` with `page0` written over its first page, at `out`.
fn splice(current: &Path, page0: &[u8], out: &Path) {
    let mut bytes = std::fs::read(current).unwrap();
    bytes[..PAGE].copy_from_slice(&page0[..PAGE]);
    std::fs::write(out, bytes).unwrap();
}

#[test]
fn rekey_leaves_no_copy_of_the_old_key_wrap_in_the_file() {
    for version in [8u16, 9] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("r.mnemo");
        make(&path, version, "passphrase-A", 20);
        let old_wrap = wrap_of(&std::fs::read(&path).unwrap());

        let mut db = Mnemo::open(&path, "passphrase-A").unwrap();
        db.rekey("passphrase-B", KdfParams::fast()).unwrap();
        for i in 20..25 {
            db.remember(mem(i)).unwrap();
        }
        db.flush().unwrap();
        db.close().unwrap();

        let after = std::fs::read(&path).unwrap();
        assert!(
            !contains(&after, &old_wrap),
            "v{version}: the old key wrap is still in the file"
        );
        assert!(Mnemo::open(&path, "passphrase-A").is_err());
        assert_eq!(
            Mnemo::open_read_only(&path, "passphrase-B").unwrap().len(),
            25
        );
    }
}

#[test]
fn a_plain_rekey_does_not_revoke_an_earlier_copy_of_the_header() {
    // The documented limit of `rekey`: the data key stays, so an earlier
    // header plus the old secret still opens the file. `--rotate` exists
    // for this.
    let dir = tempdir().unwrap();
    let path = dir.path().join("p.mnemo");
    make(&path, 9, "passphrase-A", 10);
    let old_page0 = std::fs::read(&path).unwrap()[..PAGE].to_vec();
    let mut db = Mnemo::open(&path, "passphrase-A").unwrap();
    db.rekey("passphrase-B", KdfParams::fast()).unwrap();
    db.close().unwrap();
    let spliced = dir.path().join("spliced.mnemo");
    splice(&path, &old_page0, &spliced);
    assert_eq!(
        Mnemo::open_read_only(&spliced, "passphrase-A")
            .unwrap()
            .len(),
        10
    );
}

#[test]
fn rotating_revokes_the_old_passphrase() {
    for version in [8u16, 9] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rot.mnemo");
        make(&path, version, "passphrase-A", 20);
        let old_page0 = std::fs::read(&path).unwrap()[..PAGE].to_vec();

        // `mnemo rekey --rotate`: rekey, then compact under a fresh data key.
        let mut db = Mnemo::open(&path, "passphrase-A").unwrap();
        db.rekey("passphrase-B", KdfParams::fast()).unwrap();
        db.close().unwrap();
        let report = Mnemo::compact_file(path.to_str().unwrap(), "passphrase-B").unwrap();
        assert_eq!(report.after, 20);
        let mut db = Mnemo::open(&path, "passphrase-B").unwrap();
        for i in 20..30 {
            db.remember(mem(i)).unwrap();
        }
        db.flush().unwrap();
        db.close().unwrap();

        let spliced = dir.path().join("spliced.mnemo");
        splice(&path, &old_page0, &spliced);
        assert!(
            Mnemo::open_read_only(&spliced, "passphrase-A").is_err(),
            "v{version}: the pre-rotation header still opens the rotated file"
        );
        assert_eq!(
            Mnemo::open_read_only(&path, "passphrase-B").unwrap().len(),
            30
        );
    }
}

#[test]
fn a_rekeyed_export_shares_no_key_material_with_its_source() {
    let dir = tempdir().unwrap();
    let live = dir.path().join("live.mnemo");
    let copy = dir.path().join("copy.mnemo");
    make(&live, 9, "device-secret", 15);
    let live_page0 = std::fs::read(&live).unwrap()[..PAGE].to_vec();

    let mut db = Mnemo::open(&live, "device-secret").unwrap();
    db.cache_put("ns", "k", b"cached", Default::default())
        .unwrap();
    let report = db
        .export_rekeyed(&copy, "transfer-passphrase", KdfParams::fast())
        .unwrap();
    assert_eq!((report.after, report.cache_after), (15, 1));
    // The source keeps working.
    db.remember(mem(99)).unwrap();
    db.close().unwrap();

    // The copy opens with its own passphrase only.
    let mut c = Mnemo::open(&copy, "transfer-passphrase").unwrap();
    assert_eq!(c.len(), 15);
    assert_eq!(c.cache_get("ns", "k").unwrap().unwrap().value, b"cached");
    c.close().unwrap();
    assert!(Mnemo::open(&copy, "device-secret").is_err());

    // Neither header opens the other file.
    let export_page0 = std::fs::read(&copy).unwrap()[..PAGE].to_vec();
    let a = dir.path().join("live-with-export-header.mnemo");
    splice(&live, &export_page0, &a);
    assert!(Mnemo::open_read_only(&a, "transfer-passphrase").is_err());
    let b = dir.path().join("export-with-live-header.mnemo");
    splice(&copy, &live_page0, &b);
    assert!(Mnemo::open_read_only(&b, "device-secret").is_err());
}

#[test]
fn a_rekeyed_export_with_a_key_and_from_a_read_only_handle() {
    let dir = tempdir().unwrap();
    let live = dir.path().join("src.mnemo");
    make(&live, 8, "pw", 12);
    let key = Mnemo::generate_key();
    let mut ro = Mnemo::open_read_only(&live, "pw").unwrap();
    let copy = dir.path().join("keyed.mnemo");
    ro.export_rekeyed_with_key(&copy, &key).unwrap();
    let c = Mnemo::open_read_only_with_key(&copy, &key).unwrap();
    assert_eq!(c.len(), 12);
    assert_eq!(c.stats_format_version(), 9, "the copy is v9");
    // It refuses an existing destination and the open file itself.
    assert!(ro.export_rekeyed_with_key(&copy, &key).is_err());
    assert!(ro.export_rekeyed(&live, "x", KdfParams::fast()).is_err());
    assert!(!dir.path().join("keyed.mnemo.export-tmp").exists());
}

/// `stats().format_version` on a read-only handle.
trait FormatVersion {
    fn stats_format_version(self) -> u16;
}

impl FormatVersion for Mnemo {
    fn stats_format_version(mut self) -> u16 {
        self.stats().unwrap().format_version
    }
}
