//! v0.21.0 step 3 (spec.v0210 rev 2, 13.7): `mnemo merge a b -o out`. The
//! command runs `Mnemo::merge_files` (its rules are `tests/merge.rs`'s) and
//! prints the report as a table or JSON; a refusal exits 1 with the reason.
//! The second copy takes its own secret. `about` counts the records the
//! merge flagged.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use mnemo_engine::{KdfParams, Memory, MemoryPatch, MemoryType, Mnemo, MnemoConfig, Ulid};
use serde_json::Value;

const PW: &str = "merge-cli-passphrase";

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn v(x: f32) -> Vec<f32> {
    vec![x, 1.0, 0.5, 0.25]
}

/// `mnemo <args>` with the passphrase in the environment and no key store.
fn mnemo(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mnemo"));
    cmd.args(args)
        .env("MNEMO_DEV_NO_KEYCHAIN", "1")
        .env_remove("MNEMO_PASSPHRASE")
        .env_remove("MNEMO_KEY")
        .env_remove("MNEMO_KEY_FILE")
        .env_remove("MNEMO_KEY_COMMAND")
        .env_remove("MNEMO_AGENT")
        .env_remove("CLAUDE_PROJECT_DIR")
        .stdin(Stdio::null());
    for (k, val) in env {
        cmd.env(k, val);
    }
    cmd.output().expect("run mnemo")
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Two copies of one brain: a base fact edited differently in each (a
/// conflict), and a new fact in each. Returns (a, b, the base fact).
fn two_copies(dir: &Path) -> (PathBuf, PathBuf, Ulid) {
    let base = dir.join("base.mnemo");
    let mut db = Mnemo::create(&base, PW, cfg()).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    let fact = db
        .remember(Memory::new("the base fact", MemoryType::Semantic, v(1.0)))
        .unwrap();
    db.close().unwrap();
    let (a, b) = (dir.join("a.mnemo"), dir.join("b.mnemo"));
    for (path, edit, new, x) in [
        (&a, "edited in a", "written in a", 2.0),
        (&b, "edited in b", "written in b", 3.0),
    ] {
        std::fs::copy(&base, path).unwrap();
        let mut db = Mnemo::open(path, PW).unwrap();
        db.update(
            &fact,
            MemoryPatch {
                content: Some(edit.into()),
                vector: Some(v(x)),
                ..Default::default()
            },
        )
        .unwrap();
        db.remember(Memory::new(new, MemoryType::Semantic, v(x + 1.0)))
            .unwrap();
        db.close().unwrap();
    }
    (a, b, fact)
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn merge_prints_what_it_did_and_json_gives_the_ids() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b, _) = two_copies(dir.path());
    let out = dir.path().join("out.mnemo");
    let o = mnemo(
        &["merge", s(&a), s(&b), "-o", s(&out)],
        &[("MNEMO_PASSPHRASE", PW)],
    );
    let (stdout, stderr) = (text(&o.stdout), text(&o.stderr));
    assert!(o.status.success(), "{stderr}");
    assert!(stdout.contains(&format!("into {}", s(&out))), "{stdout}");
    assert!(
        stdout.contains("only: 1, from ") && stdout.contains(" only: 1, in both: 2"),
        "{stdout}"
    );
    assert!(
        stdout.contains("conflicts: 1 fact (1 record flagged"),
        "{stdout}"
    );
    assert!(
        stdout.contains("still hold the records the merge removed"),
        "{stdout}"
    );

    let json_out = dir.path().join("out2.mnemo");
    let o = mnemo(
        &[
            "merge",
            s(&a),
            s(&b),
            "-o",
            s(&json_out),
            "--format",
            "json",
        ],
        &[("MNEMO_PASSPHRASE", PW)],
    );
    assert!(o.status.success(), "{}", text(&o.stderr));
    let r: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(r["a_only"].as_array().unwrap().len(), 1);
    assert_eq!(r["b_only"].as_array().unwrap().len(), 1);
    assert_eq!(r["both"], 2);
    assert_eq!(r["conflicts"]["fact"], 1);
    assert_eq!(r["conflict_ids"].as_array().unwrap().len(), 1);
    assert_eq!(r["removed"], serde_json::json!([]));
    assert_eq!(r["lead_ms"].as_array().unwrap().len(), 2);

    // `about` counts what the merge flagged, in both forms.
    let o = mnemo(&["about", s(&out)], &[("MNEMO_PASSPHRASE", PW)]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(
        text(&o.stdout).contains("# 1 record flagged by a merge as in conflict"),
        "{}",
        text(&o.stdout)
    );
    let o = mnemo(
        &["about", s(&out), "--format", "json"],
        &[("MNEMO_PASSPHRASE", PW)],
    );
    let doc: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(doc["stats"]["merge_conflicts"], 1);
    // An input the merge didn't flag says nothing.
    let o = mnemo(&["about", s(&a)], &[("MNEMO_PASSPHRASE", PW)]);
    assert!(!text(&o.stdout).contains("flagged by a merge"));
}

#[test]
fn a_refused_merge_exits_1_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b, _) = two_copies(dir.path());
    // The output exists.
    let o = mnemo(
        &["merge", s(&a), s(&b), "-o", s(&b)],
        &[("MNEMO_PASSPHRASE", PW)],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(
        text(&o.stderr).contains("merge refused: the output")
            && text(&o.stderr).contains("is one of the inputs"),
        "{}",
        text(&o.stderr)
    );
    // Not one brain.
    let other = dir.path().join("other.mnemo");
    let mut db = Mnemo::create(&other, PW, cfg()).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.close().unwrap();
    let out = dir.path().join("out.mnemo");
    let o = mnemo(
        &["merge", s(&a), s(&other), "-o", s(&out)],
        &[("MNEMO_PASSPHRASE", PW)],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(
        text(&o.stderr).contains("merge refused"),
        "{}",
        text(&o.stderr)
    );
    assert!(!out.exists(), "a refused merge leaves nothing");
}

#[test]
fn the_second_copy_takes_its_own_secret() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b, _) = two_copies(dir.path());
    let mut db = Mnemo::open(&b, PW).unwrap();
    db.rekey("b's own passphrase", KdfParams::fast()).unwrap();
    db.close().unwrap();
    // The first copy's secret doesn't open the second.
    let out = dir.path().join("out.mnemo");
    let o = mnemo(
        &["merge", s(&a), s(&b), "-o", s(&out), "--passphrase", PW],
        &[],
    );
    assert_eq!(o.status.code(), Some(1), "{}", text(&o.stdout));
    assert!(!out.exists());
    // RB-020.6: the error names the second copy and how to give its own.
    let e = text(&o.stderr);
    assert!(
        e.contains(&format!("the second copy, {}", s(&b))) && e.contains("--b-passphrase"),
        "{e}"
    );
    assert!(e.contains("MNEMO_B_PASSPHRASE"), "{e}");
    // A wrong flag for it says the same.
    let o = mnemo(
        &[
            "merge",
            s(&a),
            s(&b),
            "-o",
            s(&out),
            "--passphrase",
            PW,
            "--b-passphrase",
            "not b's",
        ],
        &[],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(
        text(&o.stderr).contains("the second copy"),
        "{}",
        text(&o.stderr)
    );
    // MNEMO_B_PASSPHRASE opens it, with no flag in shell history.
    let o = mnemo(
        &["merge", s(&a), s(&b), "-o", s(&out), "--passphrase", PW],
        &[("MNEMO_B_PASSPHRASE", "b's own passphrase")],
    );
    assert!(o.status.success(), "{}", text(&o.stderr));
    Mnemo::open_read_only(&out, PW).unwrap();
    std::fs::remove_file(&out).unwrap();
    // A wrong secret for the first copy names it.
    let o = mnemo(
        &[
            "merge",
            s(&a),
            s(&b),
            "-o",
            s(&out),
            "--passphrase",
            "not a's",
        ],
        &[("MNEMO_B_PASSPHRASE", "b's own passphrase")],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(
        text(&o.stderr).contains(&format!("the first copy, {}", s(&a))),
        "{}",
        text(&o.stderr)
    );
    // Its own flag does; the output opens with the first copy's.
    let o = mnemo(
        &[
            "merge",
            s(&a),
            s(&b),
            "-o",
            s(&out),
            "--passphrase",
            PW,
            "--b-passphrase",
            "b's own passphrase",
        ],
        &[],
    );
    assert!(o.status.success(), "{}", text(&o.stderr));
    Mnemo::open_read_only(&out, PW).unwrap();
}
