//! RB-014 (deferred to Run 2's first backup, v0.18.1): the committed brain
//! backup, `docs/brain.md`, names the build that wrote it, and that build is
//! 0.18.0 or newer. An older build's `export --git` doesn't know what the
//! backup must leave out, so a backup it wrote fails here. Outside this
//! repository (the published crate has no `docs/`) the test returns at once.

use std::path::Path;

/// `mnemo X.Y.Z` in "written by mnemo X.Y.Z", as numbers.
fn build_of(header: &str) -> Option<(u64, u64, u64)> {
    let rest = header.split("written by mnemo ").nth(1)?;
    let v: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut n = v.split('.').map(|p| p.parse::<u64>().ok());
    Some((n.next()??, n.next()??, n.next()??))
}

#[test]
fn the_committed_backup_was_written_by_0_18_or_newer() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/brain.md");
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("{} isn't here: skipped", path.display());
        return;
    };
    let head: String = text.lines().take(8).collect::<Vec<_>>().join(" ");
    let build = build_of(&head).unwrap_or_else(|| {
        panic!(
            "docs/brain.md's header doesn't name the build that wrote it; re-export it \
             with mnemo 0.18 or newer (RELEASING.md, step 7): {head}"
        )
    });
    assert!(
        build >= (0, 18, 0),
        "docs/brain.md was written by mnemo {}.{}.{}; re-export it with 0.18 or newer",
        build.0,
        build.1,
        build.2
    );
}

#[test]
fn the_build_line_parses() {
    assert_eq!(
        build_of("The brain's backup, written by mnemo 0.18.1 with `--git`"),
        Some((0, 18, 1))
    );
    assert_eq!(build_of("written by mnemo 0.17.2."), Some((0, 17, 2)));
    assert!(build_of("Plaintext export of a `.mnemo` file").is_none());
    assert!(build_of("written by mnemo x").is_none());
}
