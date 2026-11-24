# The mnemo plugin for Antigravity

This plugin gives Antigravity a project's memory, from a `mnemo` brain: one
encrypted file in the repository (`.git/mnemo/project.mnemo`) that holds
what the project's sessions learned, the tasks they hand each other and a
running summary of where the work stands.

## What it runs

Everything goes through `scripts/mnemo.sh`, a short shell script that runs
the `mnemo` command you installed. The plugin carries no binary. The script
tries `$MNEMO_BIN`, then `mnemo` on the PATH, then `~/.local/bin/mnemo`
(where the install script puts it) and `~/.cargo/bin/mnemo`. Without one it
prints how to install `mnemo`.

- **The MCP server** (`mcp_config.json`): `mnemo serve --mcp --shared`,
  which finds the project's brain from the project directory.
- **The skill** (`skills/mnemo/SKILL.md`): when to recall, remember and
  use the essence. It's generated from the guide built into `mnemo`
  (`mnemo onboard --guide --format plugin-skill`), and a test checks it
  matches.

It has no hooks: `mnemo hook` speaks Claude Code's and Cursor's formats
only, so sessions aren't saved automatically here. Save what matters with
the `remember` tool.

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

Until it's in Antigravity's marketplace, install it from this folder of a
clone of the repository, with Antigravity's plugin command (`agy plugin`).

It needs `mnemo` (see <https://toarchkumar.github.io/mnemo/>) and a project
brain: run `mnemo onboard` in the repository once. On Windows the script
needs a POSIX shell; without one, add an MCP server to the client by hand
that runs `mnemo serve --mcp --shared`.

## License

Apache-2.0, as `mnemo` is.
