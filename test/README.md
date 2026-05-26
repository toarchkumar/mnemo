# MNemo dogfooding sandbox

Local **project memory** for this repo: encrypted `project.mnemo`, scripts, and
secrets. Runtime files are gitignored; `test/scripts/` and this README are tracked.

**New AI session?** After one-time setup below, run:

```bash
source test/.venv/bin/activate
# Orient first — manifest + onboarding briefing, sourced from the .mnemo file itself
mnemo about test/project.mnemo
# (or: python -c "import mnemo, os; print(mnemo.open('test/project.mnemo', os.environ['MNEMO_PASSPHRASE']).about())")

# Then pull task-specific context
python test/scripts/project_memory.py recall "project goals architecture dogfooding"
```

`mnemo about` surfaces the database's self-describing memories (tagged
`metadata.area="onboarding"`), starting with the manifest. Treat it as the
source of truth for embedder, agent_id, and conventions — see
`.cursor/rules/project-memory.mdc` for the agent workflow rule.

## One-time setup

```bash
# 1. Python bindings
cd mnemo-python && pip install maturin && maturin develop

# 2. Dogfood deps
pip install -r test/requirements.txt

# 3. Passphrase (generated on first setup — see test/.env, gitignored)
cp test/.env.example test/.env   # only if you need to set it manually
```

## Commands

```bash
# Create / refresh memory from seed.json (canonical onboarding pack)
python test/scripts/project_memory.py bootstrap

# Pull context before working (agent uses this)
python test/scripts/project_memory.py recall "your current task"

# Persist something learned
python test/scripts/project_memory.py remember "we chose 384-dim MiniLM embeddings" \
  --type episodic --importance 0.8

python test/scripts/project_memory.py info

# Track a new performance baseline (one episodic memory per metric)
python test/scripts/project_memory.py perf test/scripts/perf_v0.1.0.json
```

## Performance tracking

Performance measurements live in the database as episodic memories with
`metadata.area="performance"`. Each measurement is one memory, so you can
recall a single metric across versions or browse a whole baseline at once.

Workflow when you take new measurements:

1. Copy `test/scripts/perf_v0.1.0.json` to `test/scripts/perf_v<next>.json`
   and edit the entries (bump `version` and `measured_at`, update values).
2. Run `python test/scripts/project_memory.py perf test/scripts/perf_v<next>.json`.
3. Recall later with `... recall "performance v0.1.0 list latency"` —
   metadata round-trips, so each hit carries the version, metric, value,
   units, build, and corpus.

Each entry must include `version`, `metric`, and `label`; `measured_at`,
`value`, `units`, `build`, `corpus`, and `notes` are recommended. See
`cmd_perf`'s docstring in `scripts/project_memory.py` for the full schema.

## Layout

| Path | Tracked | Purpose |
|------|---------|---------|
| `project.mnemo` | no | Encrypted memory file |
| `.env` | no | `MNEMO_PASSPHRASE` |
| `project-memory.jsonl` | no | Vector export after bootstrap |
| `scripts/` | yes | `project_memory.py`, `seed.json`, `perf_v*.json` |
| `.cache/` | no | Embedding model cache |

## Settings (chosen for this dogfood run)

- **384 dimensions** — `all-MiniLM-L6-v2` (local, no API key)
- **agent_id** — `cursor-agent`
- **Passphrase** — `test/.env` (dev secret; rotate with `mnemo rekey` if needed)

## Cursor agent

See `.cursor/rules/project-memory.mdc` — recall project memory at the start of
substantive tasks and `remember` durable facts after sessions.
