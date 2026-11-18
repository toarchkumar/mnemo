//! A small, synthetic brain for screenshots of `mnemo inspect` (v0.19.0):
//! review findings `DEMO-1.1` onward in every status, a few tasks, and a
//! handful of memories. Nothing here comes from a real brain: the names are
//! roles and the text is made up. `test/scripts/inspect_screenshot.sh`
//! builds it and photographs the Findings tab.
//!
//! ```text
//! cargo run --release --example inspect_demo_brain -- <out.mnemo> <passphrase>
//! ```

use mnemo_engine::{FindingSpec, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, TaskSpec};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (path, pw) = match (args.get(1), args.get(2)) {
        (Some(p), Some(w)) => (p.clone(), w.clone()),
        _ => {
            eprintln!("usage: inspect_demo_brain <out.mnemo> <passphrase>");
            std::process::exit(2);
        }
    };
    let _ = std::fs::remove_file(&path);
    let cfg = MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    };
    let mut db = Mnemo::create(&path, &pw, cfg).expect("create");
    db.remember(Memory::scaffold_manifest(4)).expect("manifest");
    for (i, text) in [
        "Deploys go out on Tuesdays, after the staging check.",
        "The cache key includes the model name and its version.",
        "Release notes are drafted from the merged PR titles.",
    ]
    .iter()
    .enumerate()
    {
        db.remember(Memory::new(
            *text,
            MemoryType::Semantic,
            vec![1.0 + i as f32, 0.5, 0.25, 1.0],
        ))
        .expect("remember");
    }

    let add =
        |db: &mut Mnemo, id: &str, category: &str, priority: &str, title: &str, body: &str| {
            let mut f = FindingSpec::new(id, category, priority, title);
            f.body = body.into();
            f.refs = vec![format!(
                "src/demo/{}.rs:42",
                id.to_lowercase().replace(['-', '.'], "_")
            )];
            db.finding_add(&f).expect("finding");
        };
    db.set_author(Some("reviewer".into()));
    add(
        &mut db,
        "DEMO-1.1",
        "CORRECTNESS",
        "HIGH",
        "A retry can run twice after a timeout",
        "The second attempt starts before the first one is cancelled.",
    );
    add(
        &mut db,
        "DEMO-1.2",
        "PERF",
        "MEDIUM",
        "The export reads every page twice",
        "One pass would do; the second re-reads what the first had.",
    );
    add(
        &mut db,
        "DEMO-1.3",
        "SECURITY",
        "MEDIUM",
        "A debug log line includes a request header",
        "Held until the fix ships.",
    );
    add(
        &mut db,
        "DEMO-1.4",
        "DX/DOCS",
        "LOW",
        "The quick start skips the install check",
        "A new user can't tell whether the install worked.",
    );
    add(
        &mut db,
        "DEMO-1.5",
        "PRODUCT",
        "IDEA",
        "Show a finding's history as a timeline",
        "The table has the data; a timeline would read faster.",
    );
    add(
        &mut db,
        "DEMO-2.1",
        "CORRECTNESS",
        "LOW",
        "An empty filter matches nothing instead of everything",
        "Leaving the field blank should mean no filter.",
    );
    add(
        &mut db,
        "DEMO-2.2",
        "PLAN-REFINEMENT",
        "MEDIUM",
        "Split the import step from the index rebuild",
        "They have different failure modes and owners.",
    );

    db.set_author(Some("planner".into()));
    db.finding_triage("DEMO-1.1", "accepted", "Fix in the next release.")
        .expect("triage");
    db.finding_triage("DEMO-1.2", "accepted", "Worth it at 100K memories.")
        .expect("triage");
    db.finding_triage("DEMO-1.3", "accepted", "Fix before the tag.")
        .expect("triage");
    db.finding_triage("DEMO-1.4", "deferred", "With the docs pass.")
        .expect("triage");
    db.finding_triage("DEMO-1.5", "needs-user", "A product call.")
        .expect("triage");
    db.finding_triage("DEMO-2.2", "rejected", "One step keeps it atomic.")
        .expect("triage");
    db.set_author(Some("reviewer".into()));
    db.finding_verify("DEMO-1.2", "a1b2c3d").expect("verify");

    db.set_author(Some("planner".into()));
    for (slug, title, refs) in [
        ("fix-retry", "Fix the double retry", vec!["DEMO-1.1"]),
        (
            "faster-export",
            "Read each page once in the export",
            vec!["DEMO-1.2"],
        ),
        ("docs-pass", "The docs pass", vec!["DEMO-1.4"]),
    ] {
        let mut t = TaskSpec::new(title);
        t.slug = Some(slug.into());
        t.role = Some("worker".into());
        t.refs = refs.into_iter().map(str::to_string).collect();
        db.task_add(&t).expect("task");
    }
    db.set_author(Some("worker".into()));
    let _ = db.task_next(Some("worker")).expect("next");
    db.close().expect("close");
    println!("{path}");
}
