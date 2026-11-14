//! 12.3 step 1 (`docs/design/12.3-keys.md`, section 10): the CLI's
//! generated key, `recovery-code`, `rekey --new-key`, the chain by kind and
//! the display rule, against the test key store (`MNEMO_DEV_KEYSTORE`, a
//! `dev-hooks` switch).

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use mnemo_engine::keys::{self, recovery};
use mnemo_engine::{KdfParams, Mnemo};
use tempfile::TempDir;

/// Every variable the chain or the display rule reads, cleared, so the
/// test's own environment (an agent harness's, say) can't leak in.
const CLEARED: &[&str] = &[
    "MNEMO_PASSPHRASE",
    "MNEMO_NEW_PASSPHRASE",
    "MNEMO_KEY",
    "MNEMO_KEY_FILE",
    "MNEMO_KEY_COMMAND",
    "MNEMO_DEV_KEYSTORE",
    "MNEMO_DEV_ABORT_INIT_BEFORE_RENAME",
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
];

struct Env {
    dir: TempDir,
}

impl Env {
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("store")).unwrap();
        Env { dir }
    }
    fn store(&self) -> PathBuf {
        self.dir.path().join("store")
    }
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
    fn cmd(&self, args: &[&str], env: &[(&str, &str)], with_store: bool) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_mnemo"));
        c.args(args).stdin(Stdio::null());
        for v in CLEARED {
            c.env_remove(v);
        }
        if with_store {
            c.env("MNEMO_DEV_KEYSTORE", self.store());
        }
        for (k, v) in env {
            c.env(k, v);
        }
        c.output().unwrap()
    }
    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> (bool, String, String) {
        let o = self.cmd(args, env, true);
        (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    }
    fn entries(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(self.store())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_none())
            .collect();
        v.sort();
        v
    }
    /// The key stored for `brain`.
    fn stored_key(&self, brain: &Path) -> [u8; 32] {
        let salt = keys::file_info(brain).unwrap().salt;
        let text = std::fs::read_to_string(self.store().join(keys::hex(&salt))).unwrap();
        let mut k = [0u8; 32];
        let h = text.strip_prefix("key:").unwrap().trim();
        for (i, b) in k.iter_mut().enumerate() {
            *b = u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap();
        }
        k
    }
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[cfg(unix)]
fn owner_only(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600)).unwrap();
}
#[cfg(not(unix))]
fn owner_only(_: &Path) {}

/// Nothing in `out` from which `key` could be rebuilt.
fn no_key_material(out: &str, key: &[u8; 32]) {
    assert!(
        !out.contains(&keys::hex(key)),
        "the key's hex was printed: {out}"
    );
    let code = recovery::encode(key);
    for g in code.split('-').skip(1) {
        assert!(
            !out.contains(&g[..4]),
            "a group of the code ({g}) was printed: {out}"
        );
    }
    assert!(!out.contains("MN1-"), "a code was printed: {out}");
}

// --- Test 9: the end-to-end ---

#[test]
fn init_compact_rekey_rotate_then_open_with_nothing_typed() {
    let e = Env::new();
    let b = e.path("b.mnemo");
    let (ok, out, err) = e.run(&["init", "--dimensions", "4", s(&b)], &[]);
    assert!(ok, "{err}");
    assert!(out.contains("generated key"), "{out}");
    // Test 7: piped, so no key material.
    let k0 = e.stored_key(&b);
    no_key_material(&(out.clone() + &err), &k0);
    assert!(err.contains("wasn't shown"), "{err}");
    assert!(keys::file_info(&b).unwrap().kdf.below_passphrase_floor());
    assert_eq!(e.entries().len(), 1);

    let (ok, _, err) = e.run(&["compact", s(&b)], &[]);
    assert!(ok, "{err}");
    assert_eq!(
        e.entries().len(),
        1,
        "a compaction keeps the salt, so the entry"
    );

    let (ok, out, err) = e.run(&["rekey", "--rotate", "--new-key", s(&b)], &[]);
    assert!(ok, "{err}");
    assert!(out.contains("rotated"), "{out}");
    assert_eq!(e.entries().len(), 2, "the old entry is kept");
    let k1 = e.stored_key(&b);
    assert_ne!(k1, k0);
    no_key_material(&(out + &err), &k1);

    let (ok, out, err) = e.run(&["info", s(&b)], &[]);
    assert!(ok, "{err}");
    assert!(out.contains("key source:  this brain's entry"), "{out}");
    assert!(out.contains("memories:    1"), "{out}");
}

#[test]
fn a_rotation_to_a_passphrase_opens_from_the_environment_with_full_parameters() {
    let e = Env::new();
    let b = e.path("b.mnemo");
    assert!(e.run(&["init", "--dimensions", "4", s(&b)], &[]).0);
    let (ok, _, err) = e.run(
        &["rekey", "--rotate", s(&b)],
        &[("MNEMO_NEW_PASSPHRASE", "a new passphrase")],
    );
    assert!(ok, "{err}");
    let kdf = keys::file_info(&b).unwrap().kdf;
    assert!(kdf.m_cost >= KdfParams::secure().m_cost, "{kdf:?}");
    let (ok, out, err) = e.run(
        &["info", s(&b)],
        &[("MNEMO_PASSPHRASE", "a new passphrase")],
    );
    assert!(ok, "{err}");
    assert!(out.contains("key source:  MNEMO_PASSPHRASE"), "{out}");
    assert!(!out.contains("kdf:"), "full parameters need no word: {out}");
}

// --- Test 10: the chain by kind ---

#[test]
fn each_source_opens_a_key_brain_and_a_passphrase_is_skipped_and_named() {
    let e = Env::new();
    let b = e.path("b.mnemo");
    assert!(e.run(&["init", "--dimensions", "4", s(&b)], &[]).0);
    let key = e.stored_key(&b);
    let hex = keys::hex(&key);
    let code = recovery::encode(&key).to_string();
    let file = e.path("k.key");
    std::fs::write(&file, &hex).unwrap();
    owner_only(&file);

    let info = |env: &[(&str, &str)], store: bool| {
        let o = e.cmd(&["info", s(&b)], env, store);
        (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    };
    for (env, want) in [
        (vec![("MNEMO_KEY", hex.as_str())], "MNEMO_KEY"),
        (vec![("MNEMO_KEY", code.as_str())], "MNEMO_KEY"),
        (vec![("MNEMO_KEY_FILE", s(&file))], "MNEMO_KEY_FILE"),
    ] {
        let (ok, out, err) = info(&env, false);
        assert!(ok, "{want}: {err}");
        assert!(out.contains(&format!("key source:  {want}")), "{out}");
    }
    #[cfg(unix)]
    {
        let cmd = format!("cat {}", s(&file));
        let (ok, out, err) = info(&[("MNEMO_KEY_COMMAND", &cmd)], false);
        assert!(ok, "{err}");
        assert!(out.contains("key source:  MNEMO_KEY_COMMAND"), "{out}");
    }
    // A leftover MNEMO_PASSPHRASE is skipped, and named.
    let (ok, out, err) = info(&[("MNEMO_PASSPHRASE", "left over from v0.14")], true);
    assert!(ok, "{err}");
    assert!(
        err.contains("MNEMO_PASSPHRASE holds a passphrase") && err.contains("skipped"),
        "{err}"
    );
    assert!(out.contains("this brain's entry"), "{out}");

    // A wrong key of the right kind fails, names itself and the next one.
    let wrong = keys::hex(&Mnemo::generate_key());
    let (ok, _, err) = info(&[("MNEMO_KEY", &wrong)], true);
    assert!(!ok);
    assert!(
        err.contains("came from MNEMO_KEY") && err.contains("is also set"),
        "{err}"
    );

    // No source of the file's kind: a passphrase is tried once.
    let (ok, _, err) = info(&[("MNEMO_PASSPHRASE", "not it")], false);
    assert!(!ok);
    assert!(err.contains("trying MNEMO_PASSPHRASE once"), "{err}");
    assert!(err.contains("came from MNEMO_PASSPHRASE"), "{err}");

    // Nothing at all: the key sources are listed.
    let (ok, _, err) = info(&[], false);
    assert!(!ok);
    assert!(
        err.contains("no key for") && err.contains("MNEMO_KEY_FILE"),
        "{err}"
    );

    // --key-file skips the chain.
    let o = e.cmd(
        &["info", s(&b), "--key-file", s(&file)],
        &[("MNEMO_KEY", &wrong)],
        true,
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

// --- Test 11: key files, through the CLI ---

#[cfg(unix)]
#[test]
fn a_key_file_open_to_others_is_refused_naming_chmod() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new();
    let b = e.path("b.mnemo");
    assert!(e.run(&["init", "--dimensions", "4", s(&b)], &[]).0);
    let file = e.path("k.key");
    std::fs::write(&file, keys::hex(&e.stored_key(&b))).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let (ok, _, err) = e.run(&["info", s(&b), "--key-file", s(&file)], &[]);
    assert!(!ok && err.contains("chmod 600"), "{err}");
}

// --- Test 12: init ---

#[test]
fn init_with_no_store_fails_creates_nothing_and_leads_with_ask_passphrase() {
    let e = Env::new();
    let b = e.path("b.mnemo");
    let o = e.cmd(&["init", "--dimensions", "4", s(&b)], &[], false);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success());
    let first = err
        .find("--ask-passphrase")
        .expect("names --ask-passphrase");
    let second = err.find("--key-file").expect("names --key-file");
    assert!(first < second, "--ask-passphrase comes first: {err}");
    assert!(!b.exists());
    let left: Vec<_> = std::fs::read_dir(e.dir.path()).unwrap().collect();
    assert_eq!(left.len(), 1, "only the store directory");
}

#[test]
fn init_with_a_key_file_writes_it_owner_only_and_refuses_an_existing_one() {
    let e = Env::new();
    let b = e.path("b.mnemo");
    let kf = e.path("new.key");
    let o = e.cmd(
        &["init", "--dimensions", "4", "--key-file", s(&kf), s(&b)],
        &[],
        false,
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&kf).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let key = keys::read_key_file(&kf).unwrap();
    assert_eq!(Mnemo::open_with_key(&b, &key).unwrap().len(), 1);
    // An existing key file is refused, and nothing is created.
    let c = e.path("c.mnemo");
    let o = e.cmd(
        &["init", "--dimensions", "4", "--key-file", s(&kf), s(&c)],
        &[],
        false,
    );
    assert!(!o.status.success());
    assert!(!c.exists());
}

#[cfg(unix)]
#[test]
fn init_killed_before_the_rename_leaves_no_brain_and_the_next_init_works() {
    let e = Env::new();
    let b = e.path("b.mnemo");
    let o = e.cmd(
        &["init", "--dimensions", "4", s(&b)],
        &[("MNEMO_DEV_ABORT_INIT_BEFORE_RENAME", "1")],
        true,
    );
    assert!(!o.status.success());
    assert!(!b.exists(), "no brain at the path");
    let temps = |e: &Env| {
        std::fs::read_dir(e.dir.path())
            .unwrap()
            .filter(|d| {
                d.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("b.mnemo.init-")
            })
            .count()
    };
    assert_eq!(temps(&e), 1, "the dead temp is left");
    let (ok, _, err) = e.run(&["init", "--dimensions", "4", s(&b)], &[]);
    assert!(ok, "{err}");
    assert_eq!(temps(&e), 0, "the next init removed it");
    assert!(b.exists());
}

#[test]
fn init_with_a_passphrase_works_as_before() {
    let e = Env::new();
    let b = e.path("b.mnemo");
    let (ok, out, err) = e.run(
        &["init", "--dimensions", "4", s(&b)],
        &[("MNEMO_PASSPHRASE", "a passphrase")],
    );
    assert!(ok, "{err}");
    assert!(out.contains("with a passphrase"), "{out}");
    assert!(e.entries().is_empty());
    assert!(!keys::file_info(&b).unwrap().kdf.below_passphrase_floor());
}

// --- Test 8: recovery-code ---

#[test]
fn recovery_code_refuses_without_a_person_a_passphrase_entry_and_a_stale_key() {
    let e = Env::new();
    let b = e.path("b.mnemo");
    assert!(e.run(&["init", "--dimensions", "4", s(&b)], &[]).0);
    // Piped: refused, with the message and no key material.
    let (ok, out, err) = e.run(&["recovery-code", s(&b)], &[]);
    assert!(!ok && err.contains("not through an agent's shell"), "{err}");
    no_key_material(&(out + &err), &e.stored_key(&b));

    // A passphrase entry has no code.
    let p = e.path("p.mnemo");
    assert!(
        e.run(
            &["init", "--dimensions", "4", s(&p)],
            &[("MNEMO_PASSPHRASE", "pp")]
        )
        .0
    );
    let salt = keys::file_info(&p).unwrap().salt;
    std::fs::write(e.store().join(keys::hex(&salt)), "passphrase:pp").unwrap();
    let (ok, _, err) = e.run(&["recovery-code", s(&p)], &[]);
    assert!(!ok && err.contains("uses a passphrase"), "{err}");

    // A stored key that doesn't open the file isn't shown.
    let salt = keys::file_info(&b).unwrap().salt;
    let other = keys::hex(&Mnemo::generate_key());
    std::fs::write(e.store().join(keys::hex(&salt)), format!("key:{other}")).unwrap();
    let (ok, _, err) = e.run(&["recovery-code", s(&b)], &[]);
    assert!(!ok && err.contains("doesn't open it"), "{err}");
}

// --- Test 7 under a pseudo-terminal, with `script` ---

/// Run `mnemo <args>` under a pseudo-terminal through `script`, feeding
/// `input` after `delay`, and return everything the terminal showed.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn under_pty(
    e: &Env,
    args: &[&str],
    env: &[(&str, &str)],
    input: &[u8],
    delay: std::time::Duration,
) -> Option<(String, String)> {
    use std::io::{Read, Write};
    if Command::new("script").arg("-V").output().is_err() && !Path::new("/usr/bin/script").exists()
    {
        return None;
    }
    let bin = env!("CARGO_BIN_EXE_mnemo");
    let mut c = Command::new("script");
    #[cfg(target_os = "macos")]
    {
        c.arg("-q").arg("/dev/null").arg(bin).args(args);
    }
    #[cfg(target_os = "linux")]
    {
        let line = std::iter::once(bin)
            .chain(args.iter().copied())
            .map(|a| format!("'{a}'"))
            .collect::<Vec<_>>()
            .join(" ");
        c.arg("-qec").arg(line).arg("/dev/null");
    }
    for v in CLEARED {
        c.env_remove(v);
    }
    c.env("MNEMO_DEV_KEYSTORE", e.store());
    for (k, v) in env {
        c.env(k, v);
    }
    c.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().ok()?;
    let mut out = child.stdout.take().unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let reader = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = out.read(&mut buf) {
            if n == 0 {
                break;
            }
            seen2.lock().unwrap().extend_from_slice(&buf[..n]);
        }
    });
    std::thread::sleep(delay);
    let before = String::from_utf8_lossy(&seen.lock().unwrap()).into_owned();
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(input);
    let _ = stdin.flush();
    std::thread::sleep(delay);
    drop(stdin);
    let _ = child.wait();
    let _ = reader.join();
    let all = String::from_utf8_lossy(&seen.lock().unwrap()).into_owned();
    Some((before, all))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn under_a_pseudo_terminal_the_code_waits_for_enter_and_a_marker_refuses() {
    let e = Env::new();
    let b = e.path("b.mnemo");
    assert!(e.run(&["init", "--dimensions", "4", s(&b)], &[]).0);
    let key = e.stored_key(&b);
    let code = recovery::encode(&key).to_string();
    let wait = std::time::Duration::from_millis(1500);

    // With a marker set, refused even at a terminal.
    let Some((_, all)) = under_pty(
        &e,
        &["recovery-code", s(&b)],
        &[("CLAUDECODE", "1")],
        b"",
        wait,
    ) else {
        eprintln!("no `script` here; skipped");
        return;
    };
    assert!(all.contains("CLAUDECODE is set"), "{all}");
    no_key_material(&all, &key);

    // Without one: nothing before Enter, then the code.
    let (before, all) = under_pty(&e, &["recovery-code", s(&b)], &[], b"\n\n", wait).unwrap();
    assert!(before.contains("Press Enter to show"), "{before}");
    assert!(!before.contains(&code), "the code before Enter: {before}");
    assert!(all.contains(&code), "the code after Enter: {all}");
}
