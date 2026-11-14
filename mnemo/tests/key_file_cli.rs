//! RB-003.6 (v0.13.1): `--key-file` on the CLI's read and convert commands,
//! so a raw-key file (what `create_with_key` writes, and what the phone
//! libraries keep) can be read, exported and compacted without the library.

use std::path::{Path, PathBuf};
use std::process::Command;

use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig};
use tempfile::TempDir;

fn mnemo(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_mnemo"))
        .args(args)
        .env_remove("MNEMO_PASSPHRASE")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// 12.3: a key file must be this user's with no access for group or others,
/// and git checks files out 0644, so a test sets the mode first.
fn owner_only(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn hex(key: &[u8]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

/// A raw-key v9 file with three memories, and its key written twice: as 64
/// hex digits and as 32 raw bytes.
fn raw_key_file() -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.mnemo");
    let key = Mnemo::generate_key();
    let cfg = MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    let mut db = Mnemo::create_with_key(&path, &key, cfg).unwrap();
    for (i, text) in [
        "oat milk, not dairy",
        "the deploy runs on Tuesdays",
        "a third note",
    ]
    .iter()
    .enumerate()
    {
        db.remember(Memory::new(
            *text,
            MemoryType::Semantic,
            vec![i as f32 + 1.0, 0.5, 0.25, 1.0],
        ))
        .unwrap();
    }
    db.close().unwrap();
    let hex_path = dir.path().join("app.key");
    std::fs::write(&hex_path, hex(&key) + "\n").unwrap();
    owner_only(&hex_path);
    let raw_path = dir.path().join("app.raw");
    std::fs::write(&raw_path, key).unwrap();
    owner_only(&raw_path);
    (dir, path, hex_path, raw_path)
}

#[test]
fn the_read_commands_take_a_key_file() {
    let (dir, path, hex_key, raw_key) = raw_key_file();
    let p = path.to_str().unwrap();
    let before = std::fs::read(&path).unwrap();
    for key in [&hex_key, &raw_key] {
        let k = key.to_str().unwrap();
        let (ok, out, err) = mnemo(&["info", p, "--key-file", k]);
        assert!(ok && out.contains("memories:    3"), "{out}{err}");
        let (ok, out, err) = mnemo(&["list", p, "--key-file", k, "--format", "jsonl"]);
        assert!(ok && out.lines().count() == 3, "{out}{err}");
        let (ok, _, err) = mnemo(&["about", p, "--key-file", k]);
        assert!(ok, "{err}");
        let (ok, out, err) = mnemo(&["verify", p, "--key-file", k]);
        assert!(ok && out.contains("verified 3 records"), "{out}{err}");
        let (ok, out, err) = mnemo(&["export", p, "--key-file", k, "--git", "--format", "jsonl"]);
        assert!(ok && out.contains("oat milk"), "{out}{err}");
        let (ok, out, err) = mnemo(&[
            "recall",
            p,
            "--key-file",
            k,
            "--query",
            "1,0.5,0.25,1",
            "--top-k",
            "2",
            "--no-track",
        ]);
        assert!(ok && out.contains("oat milk"), "{out}{err}");
    }
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "a read command changed the file"
    );

    // A wrong key, a malformed key file, and a key file with a passphrase.
    let wrong = dir.path().join("wrong.key");
    std::fs::write(&wrong, hex(&Mnemo::generate_key())).unwrap();
    owner_only(&wrong);
    let (ok, _, err) = mnemo(&["info", p, "--key-file", wrong.to_str().unwrap()]);
    assert!(!ok && err.contains("wrong passphrase"), "{err}");
    let bad = dir.path().join("bad.key");
    std::fs::write(&bad, "not a key").unwrap();
    owner_only(&bad);
    let (ok, _, err) = mnemo(&["list", p, "--key-file", bad.to_str().unwrap()]);
    assert!(
        !ok && err.contains("32 raw bytes, 64 hex digits or a recovery code"),
        "{err}"
    );
    let (ok, _, err) = mnemo(&[
        "list",
        p,
        "--key-file",
        hex_key.to_str().unwrap(),
        "--passphrase",
        "x",
    ]);
    assert!(!ok && err.contains("cannot be used with"), "{err}");
}

#[test]
fn compact_takes_a_key_file_and_converts_a_raw_key_v8_file() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v8");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("k.mnemo");
    std::fs::copy(fixtures.join("v8_key.mnemo"), &path).unwrap();
    let p = path.to_str().unwrap();
    // The fixture is checked out 0644; a copy gets the mode a key file needs.
    let key = dir.path().join("v8_key.key");
    std::fs::copy(fixtures.join("v8_key.key"), &key).unwrap();
    owner_only(&key);
    let k = key.to_str().unwrap();

    // The refusal names the raw-key route.
    let raw = std::fs::read_to_string(&key).unwrap();
    let key_bytes: [u8; 32] =
        std::array::from_fn(|i| u8::from_str_radix(&raw.trim()[2 * i..2 * i + 2], 16).unwrap());
    let msg = Mnemo::open_with_key(&path, &key_bytes)
        .err()
        .unwrap()
        .to_string();
    assert!(msg.contains("--key-file"), "{msg}");

    let (ok, out, err) = mnemo(&["compact", p, "--key-file", k]);
    assert!(ok, "{err}");
    assert!(out.contains("(v9): 3 -> 3 live memories"), "{out}");
    let (ok, out, err) = mnemo(&["info", p, "--key-file", k]);
    assert!(ok && out.contains("format:      v9"), "{out}{err}");
    let db = Mnemo::open_with_key(&path, &key_bytes).unwrap();
    assert_eq!(db.len(), 3);
}
