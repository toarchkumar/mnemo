# MNemo dogfooding sandbox

Local **project memory** for this repo: encrypted `project.mnemo`, scripts, and
secrets. Runtime files are gitignored; `test/scripts/` and this README are tracked.

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
# Create / refresh memory from seed.json (16 onboarding memories)
python test/scripts/project_memory.py bootstrap

# Pull context before working (agent uses this)
python test/scripts/project_memory.py recall "your current task"

# Persist something learned
python test/scripts/project_memory.py remember "we chose 384-dim MiniLM embeddings" \
  --type episodic --importance 0.8

python test/scripts/project_memory.py info
```

## Layout

| Path | Tracked | Purpose |
|------|---------|---------|
| `project.mnemo` | no | Encrypted memory file |
| `.env` | no | `MNEMO_PASSPHRASE` |
| `project-memory.jsonl` | no | Vector export after bootstrap |
| `scripts/` | yes | `project_memory.py`, `seed.json` |
| `.cache/` | no | Embedding model cache |

## Settings (chosen for this dogfood run)

- **384 dimensions** — `all-MiniLM-L6-v2` (local, no API key)
- **agent_id** — `cursor-agent`
- **Passphrase** — `test/.env` (dev secret; rotate with `mnemo rekey` if needed)

## Cursor agent

See `.cursor/rules/project-memory.mdc` — recall project memory at the start of
substantive tasks and `remember` durable facts after sessions.
