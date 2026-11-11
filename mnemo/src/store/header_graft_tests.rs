//! The header graft, as the acceptance test for every path that claims to
//! revoke an earlier secret (SEC-2b). A copy of an earlier page 0 and the
//! earlier secret give the earlier data key; the graft moves that header's
//! key wrap (salt, KDF costs, DEK nonce, wrapped DEK) onto the current
//! header and re-seals it with that data key. If the current file is still
//! under the same data key, the earlier secret opens it, later writes
//! included. A path that revokes writes a fresh data key, so the earlier
//! one fails the current header's seal (`HeaderTampered`).
//!
//! `rekey --rotate` is `rekey` then `compact_file`; the mobile binding's
//! `export_with_passphrase` and `import_with_passphrase` are
//! `export_rekeyed` and `export_rekeyed_with_key` (from a read-only
//! source).

use super::*;
use crate::format::Header;

const DIMS: usize = 4;

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: DIMS,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn mem(i: usize) -> Memory {
    Memory::new(
        format!("memory {i}"),
        MemoryType::Semantic,
        vec![i as f32 + 1.0, 0.5, 0.25, 1.0],
    )
}

fn page0(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()[..PAGE_SIZE].to_vec()
}

/// What the graft gives: `Err(HeaderTampered)` when the earlier data key
/// fails the current seal (revoked), or the number of memories the earlier
/// secret reads from the grafted copy (not revoked).
fn graft(current: &Path, old_page0: &[u8], old_secret: &[u8], out: &Path) -> Result<usize> {
    let old = Header::from_page(old_page0)?;
    let kdf = KdfParams {
        m_cost: old.m_cost,
        t_cost: old.t_cost,
        p_cost: old.p_cost,
    };
    let kek = crypto::derive_kek(old_secret, &old.salt, kdf)?;
    let dek = crypto::unwrap_dek(&kek, &old.dek_nonce, &old.wrapped_dek)?;
    let bytes = std::fs::read(current)?;
    let mut h = Header::from_page(&bytes[..PAGE_SIZE])?;
    h.validate_seal(&dek)?;
    h.salt = old.salt;
    h.dek_nonce = old.dek_nonce;
    h.wrapped_dek = old.wrapped_dek;
    h.m_cost = old.m_cost;
    h.t_cost = old.t_cost;
    h.p_cost = old.p_cost;
    let mut page = h.to_page();
    h.apply_seal(&mut page, &dek)?;
    let mut grafted = bytes;
    grafted[..PAGE_SIZE].copy_from_slice(&page);
    std::fs::write(out, grafted)?;
    Ok(Mnemo::open_read_only_secret(out, old_secret)?.len())
}

#[test]
fn a_plain_rekey_does_not_revoke_the_graft_opens() {
    // Documents what plain `rekey` doesn't do: the data key is unchanged.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plain.mnemo");
    let mut db = Mnemo::create(&path, "passphrase-A", cfg()).unwrap();
    for i in 0..20 {
        db.remember(mem(i)).unwrap();
    }
    db.close().unwrap();
    let before = page0(&path);
    let mut db = Mnemo::open(&path, "passphrase-A").unwrap();
    db.rekey("passphrase-B", KdfParams::fast()).unwrap();
    for i in 20..60 {
        db.remember(mem(i)).unwrap();
        db.flush().unwrap();
    }
    db.close().unwrap();
    let n = graft(&path, &before, b"passphrase-A", &dir.path().join("g.mnemo")).unwrap();
    assert_eq!(
        n, 60,
        "the earlier secret reads everything, later writes included"
    );
}

#[test]
fn rotation_revokes_the_graft_fails_at_the_seal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rot.mnemo");
    let mut db = Mnemo::create(&path, "passphrase-A", cfg()).unwrap();
    for i in 0..20 {
        db.remember(mem(i)).unwrap();
    }
    db.close().unwrap();
    let before = page0(&path);
    // `mnemo rekey --rotate`.
    let mut db = Mnemo::open(&path, "passphrase-A").unwrap();
    db.rekey("passphrase-B", KdfParams::fast()).unwrap();
    db.close().unwrap();
    Mnemo::compact_file(path.to_str().unwrap(), "passphrase-B").unwrap();
    let mut db = Mnemo::open(&path, "passphrase-B").unwrap();
    for i in 20..40 {
        db.remember(mem(i)).unwrap();
        db.flush().unwrap();
    }
    db.close().unwrap();
    assert!(matches!(
        graft(&path, &before, b"passphrase-A", &dir.path().join("g.mnemo")),
        Err(MnemoError::HeaderTampered)
    ));
}

#[test]
fn a_rekeyed_export_revokes_the_live_files_secret() {
    // The binding's export_with_passphrase: the live file's header and secret
    // don't open the export.
    let dir = tempfile::tempdir().unwrap();
    let live = dir.path().join("live.mnemo");
    let key = Mnemo::generate_key();
    let mut db = Mnemo::create_with_key(&live, &key, cfg()).unwrap();
    for i in 0..15 {
        db.remember(mem(i)).unwrap();
    }
    // Read the header with the file closed: Windows locks an open file.
    db.close().unwrap();
    let live_page0 = page0(&live);
    let mut db = Mnemo::open_with_key(&live, &key).unwrap();
    let export = dir.path().join("export.mnemo");
    db.export_rekeyed(&export, "transfer", KdfParams::fast())
        .unwrap();
    db.close().unwrap();
    assert!(matches!(
        graft(&export, &live_page0, &key, &dir.path().join("g.mnemo")),
        Err(MnemoError::HeaderTampered)
    ));
}

#[test]
fn a_rekeyed_import_revokes_the_transfer_passphrase() {
    // The binding's import_with_passphrase: from the export, read-only, to a
    // copy under a fresh data key and the device key. The export's header
    // and the transfer passphrase don't open the imported file, even after
    // the app writes to it.
    let dir = tempfile::tempdir().unwrap();
    let export = dir.path().join("export.mnemo");
    let mut db = Mnemo::create(&export, "transfer", cfg()).unwrap();
    for i in 0..15 {
        db.remember(mem(i)).unwrap();
    }
    db.close().unwrap();
    let export_page0 = page0(&export);
    let device_key = Mnemo::generate_key();
    let imported = dir.path().join("imported.mnemo");
    let mut src = Mnemo::open_read_only(&export, "transfer").unwrap();
    src.export_rekeyed_with_key(&imported, &device_key).unwrap();
    drop(src);
    let mut db = Mnemo::open_with_key(&imported, &device_key).unwrap();
    for i in 15..40 {
        db.remember(mem(i)).unwrap();
        db.flush().unwrap();
    }
    db.close().unwrap();
    assert!(matches!(
        graft(
            &imported,
            &export_page0,
            b"transfer",
            &dir.path().join("g.mnemo")
        ),
        Err(MnemoError::HeaderTampered)
    ));
}
