//! `mnemo export` (v0.13, PLAN.md 12.5): the readable plaintext export.
//!
//! Both formats as output, the filters, `--git`'s determinism (a plain read
//! of the file doesn't change it, and one new memory adds only its own
//! lines), the plaintext warning, `-o`, and a v8 file through the read-only
//! path.

use std::path::{Path, PathBuf};
use std::process::Command;

use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, RecallRequest};
use serde_json::Value;
use tempfile::TempDir;

const PW: &str = "export-pw";

fn run(args: &[&str], pp: &str) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_mnemo"))
        .args(args)
        .env("MNEMO_PASSPHRASE", pp)
        .output()
        .expect("run mnemo");
    (
        out.status.success(),
        String::from_utf8(out.stdout).expect("utf-8 stdout"),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn export(path: &Path, extra: &[&str]) -> String {
    let mut args = vec!["export", path.to_str().unwrap()];
    args.extend_from_slice(extra);
    let (ok, out, err) = run(&args, PW);
    assert!(ok, "export failed: {err}");
    out
}

fn v(i: usize) -> Vec<f32> {
    let mut x = vec![0.0; 4];
    x[i % 4] = 1.0;
    x[(i + 1) % 4] += 0.5;
    x
}

/// Four memories over two agents and three types, one of them keyed with a
/// superseded first version.
fn seed() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("e.mnemo");
    let cfg = MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    let mut db = Mnemo::create(&path, PW, cfg).unwrap();
    db.remember(
        Memory::new("the user prefers tea", MemoryType::Semantic, v(0))
            .with_agent("assistant")
            .with_importance(0.7),
    )
    .unwrap();
    db.remember(
        Memory::new("ran the deploy script", MemoryType::Episodic, v(1)).with_agent("assistant"),
    )
    .unwrap();
    db.remember(
        Memory::new(
            "escalate via pager\n```\nnot a fence\n```",
            MemoryType::Procedural,
            v(2),
        )
        .with_agent("ops"),
    )
    .unwrap();
    db.remember_keyed(
        Memory::new("region: eu-west-1", MemoryType::Semantic, v(3)).with_agent("ops"),
        "deploy:region",
    )
    .unwrap();
    db.remember_keyed(
        Memory::new("region: eu-central-1", MemoryType::Semantic, v(3)).with_agent("ops"),
        "deploy:region",
    )
    .unwrap();
    db.close().unwrap();
    (dir, path)
}

fn jsonl(s: &str) -> Vec<Value> {
    s.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn jsonl_has_one_object_per_memory_and_no_vectors() {
    let (_d, path) = seed();
    let rows = jsonl(&export(&path, &["--format", "jsonl"]));
    // Three plain memories and two versions of the key (`Mnemo::create`
    // adds no scaffold manifest; `mnemo init` does).
    assert_eq!(rows.len(), 5);
    for r in &rows {
        assert!(r.get("vector").is_none());
        assert!(r["id"].is_string() && r["content"].is_string());
        assert!(
            r.get("access_count").is_some(),
            "without --git: access stats"
        );
    }
    let tea = rows
        .iter()
        .find(|r| r["content"] == "the user prefers tea")
        .unwrap();
    assert_eq!(tea["type"], "semantic");
    assert_eq!(tea["agent"], "assistant");
    assert_eq!(tea["importance"].to_string(), "0.7");
    let keyed: Vec<&Value> = rows
        .iter()
        .filter(|r| r["key"] == "deploy:region")
        .collect();
    assert_eq!(keyed.len(), 2, "the superseded version is exported too");
    assert!(keyed
        .iter()
        .any(|r| r["metadata"].get("_mnemo.superseded_by").is_some()));
    // Without --git: ordered by id (creation order).
    let ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
}

#[test]
fn markdown_has_a_section_per_memory_with_its_content_intact() {
    let (_d, path) = seed();
    let md = export(&path, &[]);
    assert!(md.starts_with("# mnemo export\n"));
    assert!(md.contains("5 memories. Vectors are left out."));
    assert_eq!(md.matches("\n## ").count(), 5);
    assert!(md.contains("- agent: `ops`"));
    assert!(md.contains("- key: `deploy:region`"));
    // Content holding a fence gets a longer one, so it survives as written.
    assert!(md.contains("````text\nescalate via pager\n```\nnot a fence\n```\n````\n"));
}

#[test]
fn the_filters_restrict_by_type_and_agent() {
    let (_d, path) = seed();
    let rows = jsonl(&export(
        &path,
        &["--format", "jsonl", "--type", "procedural"],
    ));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["type"], "procedural");
    let rows = jsonl(&export(&path, &["--format", "jsonl", "--agent", "ops"]));
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r["agent"] == "ops"));
    let rows = jsonl(&export(
        &path,
        &[
            "--format",
            "jsonl",
            "--agent",
            "assistant",
            "--type",
            "episodic,semantic",
        ],
    ));
    assert_eq!(rows.len(), 2);
    let (ok, _, err) = run(&["export", path.to_str().unwrap(), "--type", "nope"], PW);
    assert!(!ok && err.contains("nope"), "{err}");
}

#[test]
fn git_output_is_byte_identical_across_reads_and_recalls() {
    let (_d, path) = seed();
    for format in ["markdown", "jsonl"] {
        let a = export(&path, &["--git", "--format", format]);
        let b = export(&path, &["--git", "--format", format]);
        assert_eq!(a, b, "{format}: two exports of an unchanged file");
    }
    let git_md = export(&path, &["--git"]);
    let git_jsonl = export(&path, &["--git", "--format", "jsonl"]);
    let plain = export(&path, &["--format", "jsonl"]);
    assert!(!git_md.contains("accessed") && !git_jsonl.contains("access_count"));

    // A tracked recall changes access stats in the file.
    let mut db = Mnemo::open(&path, PW).unwrap();
    let hits = db.recall(&RecallRequest::new(v(0)).top_k(3)).unwrap();
    assert!(!hits.is_empty());
    db.close().unwrap();

    assert_ne!(
        export(&path, &["--format", "jsonl"]),
        plain,
        "the plain export shows access stats"
    );
    assert_eq!(export(&path, &["--git"]), git_md);
    assert_eq!(export(&path, &["--git", "--format", "jsonl"]), git_jsonl);

    // Keyed memories first, in key order, then the rest by id.
    let rows = jsonl(&git_jsonl);
    assert_eq!(rows[0]["key"], "deploy:region");
    assert_eq!(rows[1]["key"], "deploy:region");
    assert!(rows[0]["id"].as_str() < rows[1]["id"].as_str());
    let rest: Vec<&str> = rows[2..]
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    let mut sorted = rest.clone();
    sorted.sort();
    assert_eq!(rest, sorted);
}

/// The lines of `new` that aren't in `old`, given that `new` is `old` with
/// one contiguous block inserted (panics otherwise).
fn inserted_block(old: &str, new: &str) -> Vec<String> {
    let o: Vec<&str> = old.lines().collect();
    let n: Vec<&str> = new.lines().collect();
    let prefix = o.iter().zip(&n).take_while(|(a, b)| a == b).count();
    let suffix = o[prefix..]
        .iter()
        .rev()
        .zip(n[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    assert_eq!(prefix + suffix, o.len(), "lines of the old export changed");
    n[prefix..n.len() - suffix]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn one_new_memory_changes_only_its_own_lines() {
    let (_d, path) = seed();
    let md = export(&path, &["--git"]);
    let jl = export(&path, &["--git", "--format", "jsonl"]);

    let mut db = Mnemo::open(&path, PW).unwrap();
    let id = db
        .remember(Memory::new("a new note", MemoryType::Semantic, v(1)).with_agent("ops"))
        .unwrap();
    db.close().unwrap();

    let block = inserted_block(&md, &export(&path, &["--git"]));
    assert!(block.iter().any(|l| l == &format!("## {id}")), "{block:?}");
    assert!(block.iter().any(|l| l == "a new note"));
    assert!(block.len() < 16, "{block:?}");

    let block = inserted_block(&jl, &export(&path, &["--git", "--format", "jsonl"]));
    assert_eq!(block.len(), 1);
    assert!(block[0].contains(&id.to_string()));
}

#[test]
fn it_warns_that_the_output_is_plaintext_and_writes_a_file_only_with_o() {
    let (dir, path) = seed();
    let before = std::fs::read(&path).unwrap();
    let (ok, out, err) = run(&["export", path.to_str().unwrap()], PW);
    assert!(ok && !out.is_empty());
    assert!(err.contains("plaintext"), "{err}");

    let dest = dir.path().join("out.md");
    let (ok, out, err) = run(
        &[
            "export",
            path.to_str().unwrap(),
            "-o",
            dest.to_str().unwrap(),
        ],
        PW,
    );
    assert!(ok, "{err}");
    assert!(out.is_empty(), "nothing on stdout with -o");
    assert!(
        err.contains("plaintext") && err.contains("exported 5 memories"),
        "{err}"
    );
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), export(&path, &[]));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // Over an existing file with a wider mode: replaced whole, owner-only,
    // with no temp file left behind.
    let shared = dir.path().join("shared.md");
    std::fs::write(&shared, "an older export\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let (ok, _, err) = run(
        &[
            "export",
            path.to_str().unwrap(),
            "-o",
            shared.to_str().unwrap(),
        ],
        PW,
    );
    assert!(ok, "{err}");
    assert_eq!(
        std::fs::read_to_string(&shared).unwrap(),
        export(&path, &[])
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&shared).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "the existing 0644 file is now owner-only"
        );
    }
    let leftovers: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // Never over the file itself.
    let (ok, _, err) = run(
        &[
            "export",
            path.to_str().unwrap(),
            "-o",
            path.to_str().unwrap(),
        ],
        PW,
    );
    assert!(!ok && err.contains("refusing"), "{err}");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "exporting changed the file"
    );
}

#[test]
fn a_v8_file_exports_through_the_read_only_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v8.mnemo");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v8/v8_passphrase.mnemo"),
        &path,
    )
    .unwrap();
    let pp = "v8-fixture-passphrase";
    let before = std::fs::read(&path).unwrap();
    let go = |extra: &[&str]| {
        let mut args = vec!["export", path.to_str().unwrap()];
        args.extend_from_slice(extra);
        let (ok, out, err) = run(&args, pp);
        assert!(ok, "{err}");
        out
    };
    let md = go(&[]);
    assert!(md.contains("(format v8, 8 dimensions)"));
    assert!(md.contains("17 memories."));
    // The thread's 4 turns are memories; --git leaves them out, and (since
    // v0.18.0) the thread's 2 essence items. The project's one stays.
    let all = jsonl(&go(&["--format", "jsonl"]));
    let git = jsonl(&go(&["--format", "jsonl", "--git"]));
    assert_eq!((all.len(), git.len()), (17, 11));
    assert!(git
        .iter()
        .all(|r| r["metadata"].get("_mnemo.kind") != Some(&Value::from("turn"))));
    assert!(git
        .iter()
        .any(|r| r["metadata"].get("_mnemo.kind") == Some(&Value::from("essence"))));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "exporting changed the v8 file"
    );
}

/// v0.18.0 (the backup): `--git` writes findings (every status, in id
/// order) and the project essence (open items by section, then the
/// resolved ones from the last 30 days); never a private finding, a
/// thread's essence, a message or a claim, nor a `SECURITY` finding until
/// it's `verified`; two exports of an unchanged brain are byte-equal.
#[test]
fn git_writes_findings_and_the_project_essence_in_order() {
    use mnemo_engine::{EssenceScope, EssenceSection, FindingSpec};
    let (_d, path) = seed();
    let mut db = Mnemo::open(&path, PW).unwrap();
    db.set_author(Some("reviewer-b".into()));
    let finding_in = |db: &mut Mnemo, id: &str, category: &str| {
        db.finding_add(&{
            let mut built = FindingSpec::default();
            built.id = id.into();
            built.category = category.into();
            built.priority = "LOW".into();
            built.title = format!("finding {id}");
            built
        })
        .unwrap()
    };
    let finding = |db: &mut Mnemo, id: &str| finding_in(db, id, "PERF");
    for id in ["RB-10.1", "RB-9.10", "RB-9.2"] {
        finding(&mut db, id);
    }
    db.finding_triage("RB-9.10", "rejected", "not ours")
        .unwrap();
    // A finding can't be made private by `update` (RB-014); an unverified
    // SECURITY finding is private by itself (below).
    // SECURITY findings: only the verified one is written. A CORRECTNESS
    // finding at `open` is.
    for id in ["RB-11.1", "RB-11.2", "RB-11.3", "RB-11.4"] {
        finding_in(&mut db, id, "SECURITY");
    }
    finding_in(&mut db, "RB-12.1", "CORRECTNESS");
    db.set_author(Some("planner-a".into()));
    db.finding_triage("RB-11.2", "accepted", "yes").unwrap();
    db.finding_triage("RB-11.3", "rejected", "no").unwrap();
    db.finding_triage("RB-11.4", "accepted", "yes").unwrap();
    db.set_author(Some("reviewer-b".into()));
    db.finding_verify("RB-11.4", "v0.18.0").unwrap();
    db.set_author(Some("planner-a".into()));
    db.task_add(&{
        let mut built = mnemo_engine::TaskSpec::default();
        built.title = "Ship it".into();
        built
    })
    .unwrap();
    let add = |db: &mut Mnemo, s: EssenceSection, t: &str| {
        db.essence_add(EssenceScope::Project, s, t, None).unwrap()
    };
    add(&mut db, EssenceSection::Next, "next: the backup");
    add(
        &mut db,
        EssenceSection::Decisions,
        "decided: findings as records",
    );
    let state = add(&mut db, EssenceSection::State, "state: v0180-2 in progress");
    db.essence_resolve(&state).unwrap();
    db.essence_add(
        EssenceScope::Thread("t1".into()),
        EssenceSection::Open,
        "a thread's own item",
        None,
    )
    .unwrap();
    db.post(
        "worker-a",
        "a message",
        "",
        &[],
        None,
        Some("planner-a"),
        None,
    )
    .unwrap();
    db.claim("a topic", None).unwrap();
    db.close().unwrap();

    for format in ["markdown", "jsonl"] {
        let a = export(&path, &["--git", "--format", format]);
        assert_eq!(a, export(&path, &["--git", "--format", format]), "{format}");
        assert!(
            !a.contains("a thread's own item"),
            "thread essence: {format}"
        );
        for id in ["RB-11.1", "RB-11.2", "RB-11.3"] {
            assert!(
                !a.contains(id),
                "an unverified SECURITY finding {id}: {format}"
            );
        }
        assert!(a.contains("RB-11.4") && a.contains("RB-12.1"), "{format}");
        assert!(
            !a.contains("a message") && !a.contains("a topic"),
            "{format}"
        );
    }
    let rows = jsonl(&export(&path, &["--git", "--format", "jsonl"]));
    let what: Vec<String> = rows
        .iter()
        .map(|r| {
            let m = &r["metadata"];
            if let Some(k) = r["key"].as_str() {
                format!("key {k}")
            } else if let Some(s) = m["_mnemo.slug"].as_str() {
                format!("task {s}")
            } else if let Some(f) = m["_mnemo.finding_id"].as_str() {
                format!("finding {f}")
            } else if m["_mnemo.kind"] == "essence" && m.get("_mnemo.thread").is_none() {
                format!(
                    "essence {} {}",
                    m["_mnemo.section"].as_str().unwrap(),
                    m["_mnemo.status"].as_str().unwrap_or("-")
                )
            } else {
                "rest".into()
            }
        })
        .collect();
    let first_rest = what.iter().position(|w| w == "rest").unwrap();
    assert_eq!(
        what[..first_rest],
        [
            "key deploy:region",
            "key deploy:region",
            "task ship-it",
            "finding RB-9.2",
            "finding RB-9.10",
            "finding RB-10.1",
            "finding RB-11.4",
            "finding RB-12.1",
            "essence decisions open",
            "essence next open",
            "essence state resolved",
        ],
        "{what:?}"
    );
    assert!(what[first_rest..].iter().all(|w| w == "rest"), "{what:?}");
    // 31 days on, the resolved item is left out; the open ones stay.
    let later = Command::new(env!("CARGO_BIN_EXE_mnemo"))
        .args([
            "export",
            path.to_str().unwrap(),
            "--git",
            "--format",
            "jsonl",
        ])
        .env("MNEMO_PASSPHRASE", PW)
        .env("MNEMO_DEV_CLOCK_OFFSET", (31 * 86_400).to_string())
        .output()
        .unwrap();
    let later = String::from_utf8(later.stdout).unwrap();
    assert!(!later.contains("v0180-2 in progress"), "{later}");
    assert!(later.contains("next: the backup") && later.contains("RB-9.10"));
    // The plain export keeps thread essence; an unverified SECURITY
    // finding is private (RB-014), so it's there with `--visibility all`,
    // the owner's full copy.
    let plain = export(&path, &["--format", "jsonl"]);
    assert!(plain.contains("a thread's own item") && plain.contains("RB-11.4"));
    assert!(!plain.contains("RB-11.1"));
    let all = export(&path, &["--format", "jsonl", "--visibility", "all"]);
    for s in ["a thread's own item", "RB-11.1", "RB-11.2", "RB-11.3"] {
        assert!(all.contains(s), "{s}");
    }
    // `--visibility all` with `--git` still leaves them out.
    let git_all = export(&path, &["--git", "--visibility", "all"]);
    assert!(!git_all.contains("RB-11.1") && git_all.contains("RB-11.4"));
    // RB-014: the header names what the file is and the build that wrote
    // it; the plain export's doesn't.
    let md = export(&path, &["--git"]);
    let version = format!("written by mnemo {}", env!("CARGO_PKG_VERSION"));
    assert!(
        md.contains(&version) && md.contains("read the diff"),
        "{md}"
    );
    assert!(!export(&path, &[]).contains("written by mnemo"));
    let (ok, _, err) = run(&["export", path.to_str().unwrap(), "--git"], PW);
    assert!(
        ok && err.contains(&version) && err.contains("plaintext"),
        "{err}"
    );
}

/// v0.18.1: with `--git`, a task whose refs name a `SECURITY` finding that
/// isn't verified is left out (an id with leading zeros names the same
/// finding); it comes back once the finding is verified. A ref to an id the
/// brain doesn't have, or to another category's finding, hides nothing, and
/// the plain export keeps every task.
#[test]
fn git_leaves_out_a_task_that_refs_an_unverified_security_finding() {
    use mnemo_engine::{FindingSpec, TaskSpec};
    let (_d, path) = seed();
    let mut db = Mnemo::open(&path, PW).unwrap();
    db.set_author(Some("reviewer-b".into()));
    for (id, category) in [
        ("RB-50.1", "SECURITY"),
        ("RB-50.2", "SECURITY"),
        ("RB-50.3", "SECURITY"),
        ("RB-50.4", "CORRECTNESS"),
    ] {
        db.finding_add(&{
            let mut built = FindingSpec::default();
            built.id = id.into();
            built.category = category.into();
            built.priority = "MEDIUM".into();
            built.title = format!("finding {id}");
            built
        })
        .unwrap();
    }
    db.set_author(Some("planner-a".into()));
    db.finding_triage("RB-50.2", "accepted", "yes").unwrap();
    db.finding_triage("RB-50.3", "accepted", "yes").unwrap();
    for (slug, refs) in [
        ("fix-open", vec!["RB-50.1"]),
        ("fix-accepted", vec!["RB-050.2"]),
        ("fix-soon-verified", vec!["RB-50.3"]),
        ("fix-mixed", vec!["src/a.rs:1", "RB-50.1"]),
        ("fix-unknown", vec!["RB-999.9"]),
        ("fix-correctness", vec!["RB-50.4"]),
        ("no-refs", vec![]),
    ] {
        db.task_add(&{
            let mut built = TaskSpec::default();
            built.title = format!("task {slug}");
            built.slug = Some(slug.into());
            built.refs = refs.into_iter().map(str::to_string).collect();
            built
        })
        .unwrap();
    }
    db.close().unwrap();

    let slugs = |args: &[&str]| -> Vec<String> {
        jsonl(&export(&path, args))
            .iter()
            .filter_map(|r| r["metadata"]["_mnemo.slug"].as_str().map(str::to_string))
            .collect()
    };
    assert_eq!(
        slugs(&["--git", "--format", "jsonl"]),
        ["fix-correctness", "fix-unknown", "no-refs"]
    );
    let md = export(&path, &["--git"]);
    assert!(
        !md.contains("task fix-open") && !md.contains("task fix-mixed"),
        "{md}"
    );
    assert!(md.contains("task fix-unknown"));
    assert_eq!(
        slugs(&["--format", "jsonl", "--visibility", "all"]).len(),
        7,
        "the plain export keeps every task"
    );

    // Verified: its task comes back; the others stay out.
    let mut db = Mnemo::open(&path, PW).unwrap();
    db.set_author(Some("reviewer-b".into()));
    db.finding_verify("RB-50.3", "v0.18.1").unwrap();
    db.close().unwrap();
    assert_eq!(
        slugs(&["--git", "--format", "jsonl"]),
        [
            "fix-correctness",
            "fix-soon-verified",
            "fix-unknown",
            "no-refs"
        ]
    );
}

/// v0.21.0 (v0210-1): a delete's tombstone is an engine record. `--git`
/// leaves it out, as it leaves out messages; the plain export shows it (with
/// no content); `about`'s header total and its kinds leave it out.
#[test]
fn git_and_about_leave_out_a_deletes_tombstone() {
    let (_d, path) = seed();
    let before = jsonl(&export(&path, &["--git", "--format", "jsonl"]));
    let mut db = Mnemo::open(&path, PW).unwrap();
    let gone = db
        .remember(Memory::new("a passing note", MemoryType::Episodic, v(5)).with_agent("ops"))
        .unwrap();
    db.delete(&gone).unwrap();
    db.close().unwrap();
    let after = jsonl(&export(&path, &["--git", "--format", "jsonl"]));
    assert_eq!(
        after, before,
        "the git export is unchanged by a write and its delete"
    );
    let plain = jsonl(&export(&path, &["--format", "jsonl"]));
    let t: Vec<&Value> = plain
        .iter()
        .filter(|r| r["metadata"]["_mnemo.kind"] == "tombstone")
        .collect();
    assert_eq!(t.len(), 1, "{plain:?}");
    assert_eq!(t[0]["content"], "");
    assert_eq!(t[0]["metadata"]["_mnemo.tombstone_of"], gone.to_string());

    let (ok, out, err) = run(&["about", path.to_str().unwrap()], PW);
    assert!(ok, "{err}");
    let header = out.lines().next().unwrap();
    // The total is the sum of the kinds: no tombstone in either.
    let n = |label: &str| -> usize {
        let at = header
            .find(label)
            .unwrap_or_else(|| panic!("{label}: {header}"));
        header[..at]
            .rsplit([' ', '('])
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    let total = n(" memories");
    assert_eq!(
        total,
        n(" facts") + n(" turns") + n(" essence") + n(" coordination"),
        "{header}"
    );
    let (ok, json, err) = run(&["about", path.to_str().unwrap(), "--format", "json"], PW);
    assert!(ok, "{err}");
    let doc: Value = serde_json::from_str(&json).unwrap();
    let stats = &doc["stats"];
    assert_eq!(
        stats["memories"].as_u64().unwrap() as usize,
        total,
        "{json}"
    );
    assert!(stats["counts"].get("tombstones").is_none(), "{json}");
}
