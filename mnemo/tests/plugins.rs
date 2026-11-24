//! v0.21.1 (v0211-3, decision.plugin-directory): the plugins for Claude
//! Code, Gemini CLI, Codex and Antigravity. Each runs the installed `mnemo`
//! through its own copy of one launcher (`scripts/mnemo.sh`) and carries no
//! binary. Their manifests parse, carry this version, and point at a
//! launcher that's there; the copies of the launcher and of the guide's
//! skill match; and the launcher runs `mnemo`, or says how to install it
//! (a hook still exits 0).

use std::path::{Path, PathBuf};

use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn json(rel: &str) -> Value {
    serde_json::from_str(&read(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

const LAUNCHER: &str = "integrations/claude-code/scripts/mnemo.sh";
const COPIES: [&str; 3] = [
    "integrations/gemini/scripts/mnemo.sh",
    "integrations/codex/scripts/mnemo.sh",
    "integrations/antigravity/scripts/mnemo.sh",
];

#[test]
fn the_manifests_carry_this_version() {
    let v = env!("CARGO_PKG_VERSION");
    for rel in [
        "integrations/claude-code/.claude-plugin/plugin.json",
        "gemini-extension.json",
        "integrations/codex/plugin.json",
        "integrations/antigravity/plugin.json",
    ] {
        let m = json(rel);
        assert_eq!(m["name"], "mnemo", "{rel}");
        assert_eq!(m["version"], v, "{rel}: bump it with the manifests");
    }
    // The marketplaces parse and name the plugin's folder.
    let claude = json(".claude-plugin/marketplace.json");
    assert_eq!(claude["plugins"][0]["source"], "./integrations/claude-code");
    let codex = json(".agents/plugins/marketplace.json");
    let entry = &codex["plugins"][0];
    assert_eq!(entry["source"]["path"], "./integrations/codex");
    for k in ["installation", "authentication"] {
        assert!(entry["policy"][k].is_string(), "{k}");
    }
    assert!(entry["category"].is_string());
}

#[test]
fn every_command_runs_the_plugin_s_own_launcher() {
    // Claude Code: the server and each hook, from ${CLAUDE_PLUGIN_ROOT}
    // (the directory's checklist), the hooks' path quoted.
    let mcp = json("integrations/claude-code/.mcp.json");
    assert_eq!(
        mcp["mcpServers"]["mnemo"]["command"],
        "${CLAUDE_PLUGIN_ROOT}/scripts/mnemo.sh"
    );
    let hooks = json("integrations/claude-code/hooks/hooks.json");
    let mut n = 0;
    for (_, groups) in hooks["hooks"].as_object().unwrap() {
        for g in groups.as_array().unwrap() {
            for h in g["hooks"].as_array().unwrap() {
                let c = h["command"].as_str().unwrap();
                assert!(
                    c.starts_with("\"${CLAUDE_PLUGIN_ROOT}/scripts/mnemo.sh\" hook ")
                        && c.ends_with(" --client claude-code"),
                    "{c}"
                );
                n += 1;
            }
        }
    }
    assert_eq!(n, 4);
    // Gemini CLI: the extension is the whole repository.
    let gemini = json("gemini-extension.json");
    let cmd = gemini["mcpServers"]["mnemo"]["command"]
        .as_str()
        .unwrap()
        .replace("${extensionPath}", "")
        .replace("${/}", "/");
    assert_eq!(cmd, "/integrations/gemini/scripts/mnemo.sh");
    assert!(root()
        .join(gemini["contextFileName"].as_str().unwrap())
        .is_file());
    // Codex and Antigravity: from ${PLUGIN_ROOT}; Antigravity expands it
    // only in the arguments, so `sh` runs the script.
    let codex = json("integrations/codex/mcp.json");
    assert_eq!(
        codex["mcpServers"]["mnemo"]["command"],
        "${PLUGIN_ROOT}/scripts/mnemo.sh"
    );
    let agy = json("integrations/antigravity/mcp_config.json");
    assert_eq!(agy["mcpServers"]["mnemo"]["command"], "sh");
    assert_eq!(
        agy["mcpServers"]["mnemo"]["args"][0],
        "${PLUGIN_ROOT}/scripts/mnemo.sh"
    );
    for server in [&mcp, &gemini, &codex] {
        assert_eq!(
            server["mcpServers"]["mnemo"]["args"],
            serde_json::json!(["serve", "--mcp", "--shared"])
        );
    }
    assert!(root()
        .join("integrations/antigravity")
        .join(
            json("integrations/antigravity/plugin.json")["logo"]
                .as_str()
                .unwrap()
        )
        .is_file());
}

#[test]
fn the_copies_match() {
    let launcher = read(LAUNCHER);
    for rel in COPIES {
        assert_eq!(
            read(rel),
            launcher,
            "{rel} differs from {LAUNCHER}: copy it again"
        );
    }
    #[cfg(unix)]
    for rel in std::iter::once(LAUNCHER).chain(COPIES) {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(root().join(rel))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "{rel} isn't executable");
    }
    let skill = mnemo_engine::guide::plugin_skill_md();
    for rel in [
        "integrations/codex/skills/mnemo/SKILL.md",
        "integrations/antigravity/skills/mnemo/SKILL.md",
        "integrations/gemini/GEMINI.md",
    ] {
        assert!(
            read(rel) == skill,
            "{rel} is stale: run `mnemo onboard --guide --format plugin-skill > {rel}` from the \
             repository root"
        );
    }
}

#[test]
fn each_readme_says_what_leaves_the_machine() {
    for p in ["claude-code", "gemini", "codex", "antigravity"] {
        let t = read(&format!("integrations/{p}/README.md"));
        assert!(t.split_whitespace().count() >= 40, "{p}");
        for needle in [
            "## Privacy",
            "huggingface.co",
            "scripts/mnemo.sh",
            "Apache-2.0",
        ] {
            assert!(t.contains(needle), "{p}'s README lacks {needle:?}");
        }
    }
}

#[cfg(unix)]
#[test]
fn the_launcher_runs_mnemo_or_says_how_to_install_it() {
    use std::process::Command;
    let launcher = root().join(LAUNCHER);
    let home = tempfile::tempdir().unwrap();
    let run = |args: &[&str], bin: Option<&str>| {
        let mut c = Command::new(&launcher);
        c.args(args)
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", "/usr/bin:/bin");
        if let Some(b) = bin {
            c.env("MNEMO_BIN", b);
        }
        c.output().unwrap()
    };
    // $MNEMO_BIN, then a plain run on the PATH it was given.
    let o = run(&["--version"], Some(env!("CARGO_BIN_EXE_mnemo")));
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).starts_with("mnemo "));
    // ~/.local/bin, where the install script puts it.
    let local = home.path().join(".local/bin");
    std::fs::create_dir_all(&local).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_mnemo"), local.join("mnemo")).unwrap();
    let o = run(&["--version"], None);
    assert!(o.status.success());
    std::fs::remove_file(local.join("mnemo")).unwrap();
    // None: the server's start fails (127), a hook exits 0; both say how.
    for (args, code) in [
        (&["serve", "--mcp"][..], 127),
        (&["hook", "session-start"][..], 0),
    ] {
        let o = run(args, None);
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(code), "{args:?}: {err}");
        assert!(o.stdout.is_empty());
        assert!(
            err.contains("isn't installed") && err.contains("install.sh | sh"),
            "{err}"
        );
    }
}
