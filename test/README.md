# MNemo dogfooding helpers

This repo keeps its project memory in its **project brain,
`.git/mnemo/project.mnemo`** (since the v0.16 dogfood switch, 2026-11-15).
Git never tracks it, every worktree shares it, and its generated key is in
the user's keychain, so nothing here handles a passphrase. This folder holds
the helper scripts; `test/project.mnemo` and `test/.env` are the old layout,
retired at the switch.

**New AI session?** Your SessionStart hook has already loaded the brief (the
guide's card, then the brain's onboarding notes, essence and a short
recall). To read more, in this repository the CLI takes no path:

```bash
mnemo about                                         # the onboarding briefing
mnemo recall --text "your current task" --no-track  # context for a task
```

`mnemo about` shows the brain's self-describing memories (tagged
`metadata.area="onboarding"`), starting with the manifest: the source of
truth for the embedder, the agent id and the conventions. AGENTS.md's
"Dogfood the engine on yourself" has the rest, including who may write.

## One-time setup

```bash
# 1. The CLI (to ~/.local/bin), then wire the session: the MCP server, the
#    hooks and the skill. onboard finds the existing brain.
curl -fsSL https://toarchkumar.github.io/mnemo/install.sh | sh
mnemo onboard

# 2. Only for the helper's `info` and `perf`: the Python binding, built
#    with the keychain so it can open the brain, in this venv.
python3 -m venv test/.venv
test/.venv/bin/pip install maturin -r test/requirements.txt
cd mnemo-python && VIRTUAL_ENV="$(cd ../test/.venv && pwd)" \
  PATH="$VIRTUAL_ENV/bin:$PATH" maturin develop --release --features keychain,embed-candle
```

If the keychain entry is ever lost, the brain's recovery code restores it:
`mnemo unlock`.

## Commands

`scripts/project_memory.py` runs the installed `mnemo` CLI for `recall` and
`remember` (the first on the PATH that can read the keychain; `$MNEMO_BIN`
overrides it).

```bash
# Pull context before working
python test/scripts/project_memory.py recall "your current task"

# Persist something learned (the Worker only; one fact, with a key; the
# helper passes the brain's agent, cursor-agent)
python test/scripts/project_memory.py remember "the release order is …" \
  --type procedural --key process.release-order

python test/scripts/project_memory.py info

# Track a new performance baseline (one episodic memory per metric)
python test/scripts/project_memory.py perf test/scripts/perf_v0.1.0.json

# Build a scratch brain from seed.json (never the project brain: bootstrap
# wipes what it builds, so it refuses that one)
MNEMO_PASSPHRASE=… python test/scripts/project_memory.py --brain /tmp/scratch.mnemo bootstrap
```

## Performance tracking

Performance measurements live in the brain as episodic memories with
`metadata.area="performance"`. Each measurement is one memory, so you can
recall a single metric across versions or browse a whole baseline at once.

Workflow when you take new measurements:

1. Copy `test/scripts/perf_v0.1.0.json` to `test/scripts/perf_v<next>.json`
   and edit the entries (bump `version` and `measured_at`, update values).
2. Run `python test/scripts/project_memory.py perf test/scripts/perf_v<next>.json`.
3. Recall later with `... recall "performance v0.1.0 list latency"`:
   metadata round-trips, so each hit carries the version, metric, value,
   units, build, and corpus.

Each entry must include `version`, `metric`, and `label`; `measured_at`,
`value`, `units`, `build`, `corpus`, and `notes` are recommended. See
`cmd_perf`'s docstring in `scripts/project_memory.py` for the full schema.

## Layout

| Path | Tracked | Purpose |
|------|---------|---------|
| `../.git/mnemo/project.mnemo` | no (under `.git`) | The project brain |
| `scripts/` | yes | `project_memory.py`, `seed.json` (the canonical onboarding notes), `perf_v*.json` |
| `project-memory.jsonl` | no | Vector export after a scratch `bootstrap` |
| `.venv/`, `.cache/` | no | The helper's Python and model caches |
| `project.mnemo`, `.env` | no | The old layout, retired at the switch |

## Settings

- **384 dimensions:** the built-in `snowflake-arctic-embed-xs` (local, no API
  key; `all-MiniLM-L6-v2` before v0.15)
- **agent_id:** `cursor-agent`. A key's slot is per agent, so keyed writes
  name it.
- **The key:** in the user's keychain; `mnemo recovery-code` shows its
  recovery code at a terminal of the user's own
