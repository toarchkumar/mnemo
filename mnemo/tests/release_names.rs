//! 12.2 step 6: the `cargo binstall` template in `Cargo.toml` expands to the
//! names the release workflow gives the CLI binaries, for every target it
//! builds, and the install scripts name the same files.

use std::path::Path;

fn read(rel: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel))
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// A `key = "value"` line's value in the binstall table.
fn binstall(key: &str) -> String {
    let toml = read("Cargo.toml");
    let table = toml
        .split("[package.metadata.binstall]")
        .nth(1)
        .expect("a [package.metadata.binstall] table");
    let line = table
        .lines()
        .find(|l| l.trim_start().starts_with(&format!("{key} =")))
        .unwrap_or_else(|| panic!("no {key} in the binstall table"));
    line.split_once('=')
        .unwrap()
        .1
        .trim()
        .trim_matches('"')
        .to_string()
}

/// The (target, ext) pairs of `build-cli`'s matrix in release.yml.
fn release_targets() -> Vec<(String, String)> {
    let yml = read("../.github/workflows/release.yml");
    let job = yml.split("  build-cli:").nth(1).unwrap();
    let job = job.split("\n  build-wheels:").next().unwrap();
    let mut out = Vec::new();
    let mut target = None;
    for l in job.lines() {
        let l = l.trim();
        if let Some(t) = l.strip_prefix("- target:") {
            target = Some(t.trim().to_string());
        } else if let Some(x) = l.strip_prefix("ext:") {
            if let Some(t) = target.take() {
                out.push((t, x.trim().trim_matches('"').to_string()));
            }
        }
    }
    out
}

#[test]
fn the_binstall_template_names_every_release_binary() {
    let repo = "https://github.com/toarchkumar/mnemo";
    assert!(read("Cargo.toml").contains(&format!("repository = \"{repo}\"")));
    assert_eq!(binstall("pkg-fmt"), "bin");
    let template = binstall("pkg-url");
    let targets = release_targets();
    assert_eq!(targets.len(), 5, "{targets:?}");
    let version = "0.16.0";
    for (target, ext) in &targets {
        let url = template
            .replace("{ repo }", repo)
            .replace("{ version }", version)
            .replace("{ target }", target)
            .replace("{ binary-ext }", ext);
        // The release's own name for it (release.yml's "Stage and rename").
        let name = format!("mnemo-v{version}-{target}{ext}");
        assert_eq!(
            url,
            format!("{repo}/releases/download/v{version}/{name}"),
            "{target}"
        );
        // And the install scripts download the same name.
        if ext.is_empty() {
            assert!(read("../install.sh").contains("name=\"mnemo-v$version-$tgt\""));
            assert!(
                read("../install.sh").contains(&format!("echo \"{target}\"")),
                "{target}"
            );
        } else {
            assert!(read("../install.ps1").contains("$name = \"mnemo-v$version-$target.exe\""));
            assert!(read("../install.ps1").contains(&format!("$target = '{target}'")));
        }
    }
}
