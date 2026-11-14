//! 12.3 step 2 (`docs/design/12.3-keys.md`, "Tests in step 2"): `mnemo
//! unlock`, the lazy key-store lookup, a damaged page 0, the durability rule
//! for `rekey --new-key`, and the MCP server's chain, against the test key
//! store (`MNEMO_DEV_KEYSTORE`, a `dev-hooks` switch). The OS keychain's own
//! round trip is `tests/keychain_os.rs`.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use mnemo_engine::keys::{self, recovery};
use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig};
use tempfile::TempDir;

const CLEARED: &[&str] = &[
    "MNEMO_PASSPHRASE",
    "MNEMO_NEW_PASSPHRASE",
    "MNEMO_KEY",
    "MNEMO_KEY_FILE",
    "MNEMO_KEY_COMMAND",
    "MNEMO_DEV_KEYSTORE",
    "MNEMO_DEV_NO_KEYCHAIN",
    "MNEMO_DEV_KEYSTORE_NOT_DURABLE",
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
    fn command(&self, args: &[&str], env: &[(&str, &str)], store: Option<&Path>) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_mnemo"));
        c.args(args);
        for v in CLEARED {
            c.env_remove(v);
        }
        if let Some(s) = store {
            c.env("MNEMO_DEV_KEYSTORE", s);
        } else {
            // In a `keychain` build, "no store" must not mean the real one.
            c.env("MNEMO_DEV_NO_KEYCHAIN", "1");
        }
        for (k, v) in env {
            c.env(k, v);
        }
        c
    }
    fn run_in(&self, args: &[&str], env: &[(&str, &str)], input: &str) -> (bool, String, String) {
        let store = self.store();
        let mut c = self.command(args, env, Some(&store));
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let o: Output = child.wait_with_output().unwrap();
        (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    }
    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> (bool, String, String) {
        self.run_in(args, env, "")
    }
    fn entry(&self, brain: &Path) -> PathBuf {
        self.store()
            .join(keys::hex(&keys::file_info(brain).unwrap().salt))
    }
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn cfg(kdf: KdfParams) -> MnemoConfig {
    MnemoConfig {
        dimensions: 2,
        kdf,
        ..Default::default()
    }
}

/// A raw-key brain with one memory, made with the library (not kept in any
/// store), and its key.
fn key_brain(p: &Path) -> [u8; 32] {
    let key = Mnemo::generate_key();
    let mut db = Mnemo::create_with_key(p, &key, cfg(KdfParams::raw_key())).unwrap();
    db.remember(Memory::new("kept", MemoryType::Semantic, vec![1.0, 0.0]))
        .unwrap();
    db.close().unwrap();
    key
}

// --- unlock ---

#[test]
fn unlock_stores_a_good_code_and_refuses_the_rest() {
    let e = Env::new();
    let b = e.path("b.mnemo");
    let key = key_brain(&b);
    let code = recovery::encode(&key).to_string();

    // A typo is named by group, and nothing is stored.
    let mut gs: Vec<String> = code.split('-').map(str::to_string).collect();
    let c = if gs[3].starts_with('Z') { "Y" } else { "Z" };
    gs[3].replace_range(0..1, c);
    let typo = gs.join("-");
    let (ok, _, err) = e.run_in(&["unlock", s(&b)], &[], &format!("{typo}\n"));
    assert!(!ok && err.contains("group 3"), "{err}");
    assert!(!e.entry(&b).exists());

    // A well-formed code that doesn't open the file isn't stored.
    let other = recovery::encode(&Mnemo::generate_key()).to_string();
    let (ok, _, err) = e.run_in(&["unlock", s(&b)], &[], &format!("{other}\n"));
    assert!(!ok && err.contains("doesn't open"), "{err}");
    assert!(!e.entry(&b).exists());

    // The right one is stored, typed loosely; the brain then opens from it.
    let loose = code.to_lowercase().replace('-', " ");
    let (ok, out, err) = e.run_in(&["unlock", s(&b)], &[], &format!("{loose}\n"));
    assert!(ok, "{err}");
    assert!(out.contains("unlocked"), "{out}");
    let (ok, out, err) = e.run(&["info", s(&b)], &[]);
    assert!(ok && out.contains("this brain's entry"), "{out}{err}");

    // Again: already unlocked, nothing changed.
    let (ok, out, _) = e.run_in(&["unlock", s(&b)], &[], &format!("{code}\n"));
    assert!(ok && out.contains("already unlocked"), "{out}");

    // A stale entry is replaced.
    std::fs::write(
        e.entry(&b),
        format!("key:{}", keys::hex(&Mnemo::generate_key())),
    )
    .unwrap();
    let (ok, _, err) = e.run_in(&["unlock", s(&b)], &[], &format!("{code}\n"));
    assert!(ok && err.contains("replacing it"), "{err}");
    let (ok, _, err) = e.run(&["info", s(&b)], &[]);
    assert!(ok, "{err}");
}

#[test]
fn unlock_refuses_a_passphrase_brain_and_takes_no_argument() {
    let e = Env::new();
    let p = e.path("p.mnemo");
    Mnemo::create(&p, "a passphrase", cfg(KdfParams::fast()))
        .unwrap()
        .close()
        .unwrap();
    let code = recovery::encode(&Mnemo::generate_key()).to_string();
    let (ok, _, err) = e.run_in(&["unlock", s(&p)], &[], &format!("{code}\n"));
    assert!(!ok && err.contains("protected by a passphrase"), "{err}");
    // There's no argument to put a code in.
    let (ok, _, err) = e.run(&["unlock", s(&p), &code], &[]);
    assert!(!ok && err.contains("unexpected argument"), "{err}");
}

// --- the lazy store lookup ---

#[test]
fn a_store_that_errors_is_left_alone_when_mnemo_key_answers() {
    let e = Env::new();
    let b = e.path("b.mnemo");
    let key = key_brain(&b);
    let hex = keys::hex(&key);
    // A directory where this brain's entry file should be: reading the
    // entry fails, on every platform (Windows reads a path under a regular
    // file as "not found", so a file where the store should be won't do).
    let broken = e.path("broken-store");
    let salt = keys::file_info(&b).unwrap().salt;
    std::fs::create_dir_all(broken.join(keys::hex(&salt))).unwrap();
    let run = |env: &[(&str, &str)]| {
        let o = e
            .command(&["info", s(&b)], env, Some(&broken))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    };
    // MNEMO_KEY answers; the store isn't touched, so its error never shows.
    let (ok, out, err) = run(&[("MNEMO_KEY", &hex)]);
    assert!(ok, "{err}");
    assert!(out.contains("key source:  MNEMO_KEY"), "{out}");
    assert!(!err.contains("couldn't be read"), "{err}");
    // Only a passphrase of the other kind: the store is tried, its error is a
    // note, and the passphrase gets its one try.
    let (ok, _, err) = run(&[("MNEMO_PASSPHRASE", "not it")]);
    assert!(!ok);
    assert!(err.contains("couldn't be read"), "{err}");
    assert!(err.contains("trying MNEMO_PASSPHRASE once"), "{err}");
    // Nothing else: the store's error is the error.
    let (ok, _, err) = run(&[]);
    assert!(!ok);
    assert!(err.contains("the test key store at"), "{err}");
}

// --- a damaged page 0 ---

#[test]
fn a_key_brain_with_page_0_zeroed_opens_from_the_store_through_the_wal_salts() {
    let e = Env::new();
    let b = e.path("b.mnemo");
    let key = key_brain(&b);
    let salt = keys::file_info(&b).unwrap().salt;
    std::fs::write(
        e.store().join(keys::hex(&salt)),
        format!("key:{}", keys::hex(&key)),
    )
    .unwrap();
    // Zero the first sector: page 0 no longer parses.
    let mut bytes = std::fs::read(&b).unwrap();
    bytes[..512].fill(0);
    std::fs::write(&b, &bytes).unwrap();
    assert!(keys::file_info(&b).is_err());

    // A write open finds the entry through the WAL's header frames, and the
    // open heals the header.
    let (ok, _, err) = e.run(&["compact", s(&b)], &[]);
    assert!(ok, "{err}");
    assert!(err.contains("write-ahead log"), "{err}");
    assert_eq!(keys::file_info(&b).unwrap().salt, salt);
    let (ok, out, err) = e.run(&["info", s(&b)], &[]);
    assert!(ok, "{err}");
    assert!(out.contains("memories:    1"), "{out}");
}

// --- rekey --new-key and a store that doesn't promise durability ---

#[test]
fn a_new_key_the_code_cant_show_needs_a_key_file_on_a_store_that_isnt_durable() {
    let e = Env::new();
    let b = e.path("b.mnemo");
    assert!(e.run(&["init", "--dimensions", "4", s(&b)], &[]).0);
    let not_durable = [("MNEMO_DEV_KEYSTORE_NOT_DURABLE", "1")];
    // Piped, so the code can't be shown: refused, nothing changed.
    let before = keys::file_info(&b).unwrap().salt;
    let (ok, _, err) = e.run(&["rekey", "--new-key", s(&b)], &not_durable);
    assert!(!ok && err.contains("--new-key-file"), "{err}");
    assert_eq!(keys::file_info(&b).unwrap().salt, before);

    // With --new-key-file: the key is written there (0600) and kept.
    let kf = e.path("new.key");
    let (ok, _, err) = e.run(
        &["rekey", "--new-key", "--new-key-file", s(&kf), s(&b)],
        &not_durable,
    );
    assert!(ok, "{err}");
    let key = keys::read_key_file(&kf).unwrap();
    assert_eq!(Mnemo::open_with_key(&b, &key).unwrap().len(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&kf).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

// --- the MCP server's chain ---

/// Start `serve --mcp` on `brain` with `env`, call `list`, and return the
/// response and the server's stderr.
fn serve_and_list(e: &Env, brain: &Path, env: &[(&str, &str)]) -> (serde_json::Value, String) {
    let store = e.store();
    let mut child = e
        .command(&["serve", "--mcp", s(brain)], env, Some(&store))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut call = |line: &str| -> serde_json::Value {
        // A server that failed at start has closed its stdin already.
        if writeln!(stdin, "{line}")
            .and_then(|_| stdin.flush())
            .is_err()
        {
            return serde_json::Value::Null;
        }
        match stdout.next() {
            Some(Ok(l)) => serde_json::from_str(&l).unwrap(),
            _ => serde_json::Value::Null,
        }
    };
    call(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{}}}"#,
    );
    let r = call(
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list","arguments":{}}}"#,
    );
    drop(stdin);
    let _ = child.wait();
    let mut err = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut err).unwrap();
    (r, err)
}

#[test]
fn one_environment_serves_a_key_brain_from_the_store_and_a_passphrase_brain() {
    let e = Env::new();
    let kb = e.path("project.mnemo");
    assert!(e.run(&["init", "--dimensions", "4", s(&kb)], &[]).0);
    let pb = e.path("personal.mnemo");
    let mut db = Mnemo::create(&pb, "personal passphrase", cfg(KdfParams::fast())).unwrap();
    db.remember(Memory::new(
        "personal fact",
        MemoryType::Semantic,
        vec![1.0, 0.0],
    ))
    .unwrap();
    db.close().unwrap();
    // One environment for both servers, as an MCP client would give them.
    let env = [("MNEMO_PASSPHRASE", "personal passphrase")];

    let (r, err) = serve_and_list(&e, &kb, &env);
    assert!(r["result"].is_object(), "{r} {err}");
    assert!(
        err.contains("with the secret from this brain's entry"),
        "{err}"
    );
    assert!(err.contains("MNEMO_PASSPHRASE holds a passphrase"), "{err}");
    assert!(
        !err.contains("personal passphrase"),
        "the secret is never logged: {err}"
    );

    let (r, err) = serve_and_list(&e, &pb, &env);
    assert!(
        r["result"].to_string().contains("personal fact"),
        "{r} {err}"
    );
    assert!(
        err.contains("with the secret from MNEMO_PASSPHRASE"),
        "{err}"
    );

    // No source at all: the server fails at start, naming the sources.
    let (_, err) = serve_and_list(&e, &pb, &[]);
    assert!(
        err.contains("MNEMO_KEY_COMMAND") && err.contains("never prompts"),
        "{err}"
    );
}
