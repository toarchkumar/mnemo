//! v0.15: the embed command and the key command start with the brain's
//! secret removed from their environment (`MNEMO_PASSPHRASE`, `MNEMO_KEY`,
//! `MNEMO_KEY_FILE`, `MNEMO_KEY_COMMAND`). Each child here writes down the
//! names of the `MNEMO_*` variables it was started with.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use mnemo_engine::{keys, KdfParams, Mnemo, MnemoConfig};
use tempfile::tempdir;

const PASS: &str = "child-env-test-passphrase";
const SECRETS: [&str; 4] = [
    "MNEMO_PASSPHRASE",
    "MNEMO_KEY",
    "MNEMO_KEY_FILE",
    "MNEMO_KEY_COMMAND",
];

fn python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

/// An embed command of width 4 that writes the names of its `MNEMO_*`
/// variables to argv 1 at start, then answers every line.
const EMBED_PY: &str = r#"
import sys, json, os
with open(sys.argv[1], "w") as f:
    f.write("\n".join(sorted(k for k in os.environ if k.startswith("MNEMO_"))))
for line in sys.stdin:
    s = json.loads(line)
    print(json.dumps([1.0, float(len(s)) + 1.0, 0.5, 0.25]), flush=True)
"#;

/// A key command that writes the names of its `MNEMO_*` variables to argv 1
/// and prints the key in argv 2.
const KEY_PY: &str = r#"
import sys, os
with open(sys.argv[1], "w") as f:
    f.write("\n".join(sorted(k for k in os.environ if k.startswith("MNEMO_"))))
print(open(sys.argv[2]).read().strip())
"#;

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("embed.py"), EMBED_PY).unwrap();
        std::fs::write(dir.path().join("key.py"), KEY_PY).unwrap();
        Env { dir }
    }
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
    fn script(&self, name: &str, args: &[&Path]) -> String {
        let mut s = format!("{} \"{}\"", python(), self.path(name).display());
        for a in args {
            s.push_str(&format!(" \"{}\"", a.display()));
        }
        s
    }
    /// `mnemo remember` with the embed command, a clean environment plus
    /// `env`, and a harmless variable that must reach the child.
    fn remember(&self, brain: &Path, env: &[(&str, String)]) -> Output {
        let seen = self.path("embed.seen");
        let mut c = Command::new(env!("CARGO_BIN_EXE_mnemo"));
        c.args([
            "remember",
            brain.to_str().unwrap(),
            "--text",
            "a fact",
            "--embed-cmd",
            &self.script("embed.py", &[&seen]),
            "--embed-name",
            "env-test",
        ]);
        for (k, _) in std::env::vars_os() {
            let k = k.to_string_lossy().to_string();
            if k.starts_with("MNEMO_") || k == "CLAUDECODE" || k == "CLAUDE_CODE_CHILD_SESSION" {
                c.env_remove(k);
            }
        }
        c.env("MNEMO_CHILD_ENV_TEST", "kept");
        for (k, v) in env {
            c.env(k, v);
        }
        c.output().unwrap()
    }
}

fn names(p: &Path) -> Vec<String> {
    std::fs::read_to_string(p)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn assert_no_secret(seen: &[String], source: &str) {
    for s in SECRETS {
        assert!(
            !seen.iter().any(|n| n == s),
            "{source}: the child had {s}: {seen:?}"
        );
    }
    // The rest of the environment still reaches it.
    assert!(
        seen.iter().any(|n| n == "MNEMO_CHILD_ENV_TEST"),
        "{source}: {seen:?}"
    );
}

#[test]
fn the_embed_command_never_sees_the_brains_secret() {
    let e = Env::new();
    // A passphrase brain, opened through MNEMO_PASSPHRASE.
    let pb = e.path("p.mnemo");
    drop(Mnemo::create(&pb, PASS, cfg()).unwrap());
    let o = e.remember(&pb, &[("MNEMO_PASSPHRASE", PASS.to_string())]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_no_secret(&names(&e.path("embed.seen")), "MNEMO_PASSPHRASE");

    // A key brain, through MNEMO_KEY, MNEMO_KEY_FILE and MNEMO_KEY_COMMAND.
    let key = Mnemo::generate_key();
    let kb = e.path("k.mnemo");
    drop(Mnemo::create_with_key(&kb, &key, cfg()).unwrap());
    let key_file = e.path("k.key");
    std::fs::write(&key_file, keys::hex(&key)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let key_seen = e.path("key.seen");
    let sources = vec![
        ("MNEMO_KEY", keys::hex(&key)),
        ("MNEMO_KEY_FILE", key_file.display().to_string()),
        (
            "MNEMO_KEY_COMMAND",
            e.script("key.py", &[&key_seen, &key_file]),
        ),
    ];
    for (var, value) in sources {
        let _ = std::fs::remove_file(e.path("embed.seen"));
        // Another brain's passphrase is in the environment too.
        let env = [
            (var, value),
            ("MNEMO_PASSPHRASE", "another-brain".to_string()),
        ];
        let o = e.remember(&kb, &env);
        assert!(
            o.status.success(),
            "{var}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert_no_secret(&names(&e.path("embed.seen")), var);
    }
    let db = Mnemo::open_with_key(&kb, &key).unwrap();
    assert_eq!(db.len(), 3);
}

#[test]
fn the_key_command_never_sees_another_brains_secret() {
    let e = Env::new();
    let key = Mnemo::generate_key();
    let kb = e.path("k.mnemo");
    drop(Mnemo::create_with_key(&kb, &key, cfg()).unwrap());
    let key_file = e.path("k.key");
    std::fs::write(&key_file, keys::hex(&key)).unwrap();
    let key_seen = e.path("key.seen");
    // The key command runs for a key brain when no earlier key source is
    // set; a passphrase for another brain sits in the environment.
    let o = e.remember(
        &kb,
        &[
            (
                "MNEMO_KEY_COMMAND",
                e.script("key.py", &[&key_seen, &key_file]),
            ),
            ("MNEMO_PASSPHRASE", "another-brain".to_string()),
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_no_secret(&names(&key_seen), "the key command");
}
