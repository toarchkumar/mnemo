//! v0.16: an identical keyed re-write is a no-op on a v9 file with a
//! real-valued vector. The live vector is read from the fp16 column
//! (FMT-9b), so it's compared with the incoming one after the same
//! rounding; before, each such write superseded the last and added a
//! version.

use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, WriteOutcome};
use std::process::Command;
use tempfile::tempdir;

const PW: &str = "keyed-identical-test-passphrase";

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn mem(text: &str, v: [f32; 4]) -> Memory {
    Memory::new(text.to_string(), MemoryType::Semantic, v.to_vec())
}

const V: [f32; 4] = [0.1, 0.2, 0.3, 0.7];

#[test]
fn the_same_value_and_vector_is_unchanged_before_and_after_a_reopen() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("k.mnemo");
    let mut db = Mnemo::create(&p, PW, cfg()).unwrap();
    let (id, o) = db.remember_keyed(mem("the value", V), "fact.same").unwrap();
    assert_eq!(o, WriteOutcome::Added);
    // Before the flush (the staged column entry) and after it.
    let (again, o) = db.remember_keyed(mem("the value", V), "fact.same").unwrap();
    assert_eq!((again, o), (id, WriteOutcome::Unchanged));
    db.close().unwrap();
    let mut db = Mnemo::open(&p, PW).unwrap();
    let (again, o) = db.remember_keyed(mem("the value", V), "fact.same").unwrap();
    assert_eq!((again, o), (id, WriteOutcome::Unchanged));
    assert_eq!(db.history("fact.same", None).unwrap().len(), 1);
    // A vector that differs after the rounding, or other content, is a new
    // version.
    let (_, o) = db
        .remember_keyed(mem("the value", [0.1, 0.2, 0.3, 0.8]), "fact.same")
        .unwrap();
    assert_eq!(o, WriteOutcome::Superseded);
    let (_, o) = db
        .remember_keyed(mem("another value", [0.1, 0.2, 0.3, 0.8]), "fact.same")
        .unwrap();
    assert_eq!(o, WriteOutcome::Superseded);
    assert_eq!(db.history("fact.same", None).unwrap().len(), 3);
}

/// Through the CLI, with an embed command that gives real values.
#[test]
fn the_cli_says_unchanged() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("k.mnemo");
    Mnemo::create(&p, PW, cfg()).unwrap().close().unwrap();
    let script = dir.path().join("embed.py");
    std::fs::write(
        &script,
        "import sys, json\nfor line in sys.stdin:\n    s = json.loads(line)\n    \
         print(json.dumps([0.1, 0.2 + len(s) / 1000.0, 0.3, 0.7]), flush=True)\n",
    )
    .unwrap();
    let python = if cfg!(windows) { "python" } else { "python3" };
    let embed = format!("{python} \"{}\"", script.display());
    let run = || {
        let mut c = Command::new(env!("CARGO_BIN_EXE_mnemo"));
        for (k, _) in std::env::vars_os() {
            if k.to_string_lossy().starts_with("MNEMO_") {
                c.env_remove(k);
            }
        }
        let o = c
            .args([
                "remember",
                p.to_str().unwrap(),
                "--text",
                "the value",
                "--key",
                "fact.same",
            ])
            .args(["--embed-cmd", &embed, "--embed-name", "test-4"])
            .env("MNEMO_PASSPHRASE", PW)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    };
    let first = run();
    assert!(first.contains("added"), "{first}");
    let second = run();
    assert!(second.contains("unchanged"), "{second}");
}
