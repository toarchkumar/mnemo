# The mnemo plugin for Claude Code

This plugin gives Claude Code a project's memory, from a `mnemo` brain:

- **The MCP server** (`.mcp.json`): `mnemo serve --mcp --shared`. It finds
  the project's brain (`.git/mnemo/project.mnemo`) from the project
  directory.
- **The hooks** (`hooks/hooks.json`):
  - `mnemo hook session-start` adds the guide's card, the project's
    essence and a short recall to the session's context.
  - `mnemo hook user-prompt` (v0.17) shows what's new for the session's
    `MNEMO_AGENT` at each turn: unread messages, tasks ready for its
    role, its claims about to lapse. It prints nothing without
    `MNEMO_AGENT` or `MNEMO_ROLE`, or when nothing changed;
  - `mnemo hook pre-compact` and `mnemo hook session-end` save the
    transcript, without tool output and with known secret shapes
    redacted.
- **The skill** (`skills/mnemo/SKILL.md`): when to recall, remember and
  use the essence. It's generated from the guide built into `mnemo`
  (`mnemo onboard --guide --format plugin-skill`), and a test checks it
  matches.

It needs `mnemo` on the PATH and a project brain. The usual way to set up
a project is `mnemo onboard`, which writes the same hooks and skill into
the project's settings instead of using this plugin. `mnemo onboard
--plugin` installs this plugin, and so does:

```sh
claude plugin install mnemo --marketplace toarchkumar/mnemo --scope local
```

That works from v0.16.0: Claude Code reads the marketplace
(`.claude-plugin/marketplace.json`) from the repository's default branch,
which has it once v0.16.0 is released.

Run either from your own terminal. Claude Code ignores `-y` when an agent
runs the command.

**One difference from `onboard`'s settings hooks:** a plugin's SessionEnd
hook can't raise Claude Code's 1.5-second SessionEnd budget ("Timeouts set
on plugin-provided hooks don't raise the budget",
<https://code.claude.com/docs/en/hooks#sessionend>). So a long save at
session end may be cut short. The next save, at the next compaction or
session end, catches up.
