# The mnemo plugin for Claude Code

This plugin gives Claude Code a project's memory, from a `mnemo` brain: one
encrypted file in the repository (`.git/mnemo/project.mnemo`) that holds
what the project's sessions learned, the tasks they hand each other and a
running summary of where the work stands.

## What it runs

Everything goes through `scripts/mnemo.sh`, a short shell script that runs
the `mnemo` command you installed. The plugin carries no binary. The script
tries `$MNEMO_BIN`, then `mnemo` on the PATH, then `~/.local/bin/mnemo`
(where the install script puts it) and `~/.cargo/bin/mnemo`, since an app
started from the Dock or a desktop menu may not have those on its PATH.
Without one it prints how to install `mnemo`; a hook then exits 0, so a
session never fails because `mnemo` is missing.

- **The MCP server** (`.mcp.json`): `mnemo serve --mcp --shared`. It finds
  the project's brain from the project directory.
- **The hooks** (`hooks/hooks.json`):
  - `mnemo hook session-start` adds the guide's card, the project's
    essence and a short recall to the session's context.
  - `mnemo hook user-prompt` shows what's new for the session's
    `MNEMO_AGENT` at each turn: unread messages, tasks ready for its
    role, its claims about to lapse. It prints nothing without
    `MNEMO_AGENT` or `MNEMO_ROLE`, or when nothing changed.
  - `mnemo hook pre-compact` and `mnemo hook session-end` save the
    transcript into the brain, without tool output and with known secret
    shapes redacted.
- **The skill** (`skills/mnemo/SKILL.md`): when to recall, remember and
  use the essence. It's generated from the guide built into `mnemo`
  (`mnemo onboard --guide --format plugin-skill`), and a test checks it
  matches.

## Privacy

The brain stays on your machine, encrypted, with its key in your
system's key store. Neither the plugin nor `mnemo` sends what you save, your
prompts or your code anywhere. **The one network request:** the first time
`mnemo` embeds text with its built-in embedder, it downloads that model,
`Snowflake/snowflake-arctic-embed-xs` at a pinned revision (three files,
about 91 MB), from `huggingface.co` over HTTPS, checks each file's SHA-256
and keeps it in your cache directory (`~/Library/Caches/mnemo` on macOS,
`~/.cache/mnemo` on Linux, `%LOCALAPPDATA%\mnemo\cache` on Windows).
After that it works offline. `MNEMO_MODEL_URL` points the download at a
mirror. The hooks never download anything. Installing `mnemo` itself is a
step you take, from its releases on GitHub.

## Install

It needs `mnemo` (see <https://toarchkumar.github.io/mnemo/>) and a project
brain. The usual way to set up a project is `mnemo onboard`, which writes
the same hooks and skill into the project's settings instead of using this
plugin. `mnemo onboard --plugin` installs this plugin, and so does:

```sh
claude plugin install mnemo --marketplace toarchkumar/mnemo --scope local
```

Claude Code reads the marketplace (`.claude-plugin/marketplace.json`) from
the repository's default branch. Run either from your own terminal: Claude
Code ignores `-y` when an agent runs the command.

**On Windows,** `scripts/mnemo.sh` needs a POSIX shell to run it, which a
Windows machine may not have. Use `mnemo onboard` there, which writes the
hooks and the server as `mnemo` itself.

**One difference from `onboard`'s settings hooks:** a plugin's SessionEnd
hook can't raise Claude Code's 1.5-second SessionEnd budget ("Timeouts set
on plugin-provided hooks don't raise the budget",
<https://code.claude.com/docs/en/hooks#sessionend>). So a long save at
session end may be cut short. The next save, at the next compaction or
session end, catches up.

## License

Apache-2.0, as `mnemo` is.
