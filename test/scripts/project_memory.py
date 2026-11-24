#!/usr/bin/env python3
"""MNemo project memory: the dogfood helper for this repository's brain.

The brain is `.git/mnemo/project.mnemo` (since the v0.16 dogfood switch;
`test/project.mnemo` is the old layout, still used where it's the only
one). Its key is in the user's keychain, found by the key-source chain, so
nothing here handles a passphrase. `recall` and `remember` run the `mnemo`
CLI (v0.15 or later, able to read the keychain: $MNEMO_BIN, then each
`mnemo` on the PATH, then this repo's release build); `perf` and `info` use
the Python binding. Only the Worker writes facts, each with a key
(AGENTS.md, tmux rule 9). `bootstrap` rebuilds a scratch brain from
seed.json and refuses the project brain, which it would wipe.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

TEST_ROOT = Path(__file__).resolve().parent.parent
REPO_ROOT = TEST_ROOT.parent
LEGACY_PATH = TEST_ROOT / "project.mnemo"


def default_brain() -> Path:
    """v0.16 (12.2 step 7a): the project brain, `<git-common-dir>/mnemo/
    project.mnemo`, once the dogfood switch has moved it there; until then
    `test/project.mnemo`. So the script works on both sides of the move."""
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--git-common-dir"],
            cwd=TEST_ROOT,
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
        moved = (TEST_ROOT / out).resolve() / "mnemo" / "project.mnemo"
        if moved.is_file():
            return moved
    except (OSError, subprocess.CalledProcessError):
        pass
    return LEGACY_PATH


MNEMO_PATH = default_brain()
JSONL_PATH = TEST_ROOT / "project-memory.jsonl"
SEED_PATH = Path(__file__).resolve().parent / "seed.json"
ENV_PATH = TEST_ROOT / ".env"

AGENT_ID = "cursor-agent"
DIMENSIONS = 384
# v0.15 (12.3 step 5): the brain uses the engine's built-in embedder
# (snowflake-arctic-embed-xs, 384-dim), through `use_builtin_embedder`. A
# brain whose manifest still records MiniLM (before `mnemo reembed` moved
# it) keeps sentence-transformers, so the script works on both sides of
# the move. The name must match the manifest's embedder.name exactly.
BUILTIN_PREFIX = "Snowflake/snowflake-arctic-embed-xs@"
EMBED_MODEL = "sentence-transformers/all-MiniLM-L6-v2"


def load_dotenv() -> None:
    if not ENV_PATH.exists():
        return
    for line in ENV_PATH.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        key, value = key.strip(), value.strip().strip("'\"")
        if key and key not in os.environ:
            os.environ[key] = value


def passphrase() -> str:
    """A passphrase, for creating a scratch brain (`bootstrap --brain`)."""
    pp = optional_passphrase()
    if not pp:
        sys.exit(
            "MNEMO_PASSPHRASE isn't set: export one for the scratch brain. (The project "
            "brain needs none; its key is in the keychain.)"
        )
    return pp


def optional_passphrase() -> str | None:
    """MNEMO_PASSPHRASE from the environment (or the old layout's test/.env,
    if one is still there), else None: the project brain has a generated key
    in the keychain, and the key-source chain finds it."""
    load_dotenv()
    return os.environ.get("MNEMO_PASSPHRASE", "").strip() or None


# --- The CLI (v0.16, 12.2 step 7a) ----------------------------------------
#
# `recall` and `remember` are thin wrappers over the `mnemo` CLI, so the
# dogfood path is the one the hooks and the MCP server use. The secret
# comes from the key-source chain: the keychain holds the project brain's
# key (a scratch brain's MNEMO_PASSPHRASE reaches the child through the
# environment, never a command line).

MIN_CLI = (0, 15, 0)


def cli_version(path: str) -> tuple[int, ...] | None:
    try:
        out = subprocess.run(
            [path, "--version"], capture_output=True, text=True, timeout=30
        ).stdout.split()
        return tuple(int(x) for x in out[-1].split(".")[:3])
    except (OSError, ValueError, IndexError, subprocess.SubprocessError):
        return None


_CLI: str | None = None


def path_clis() -> list[str]:
    """Every `mnemo` on the PATH, in order (an older one can sit behind the
    installed one, as ~/.cargo/bin's did)."""
    out = []
    for d in os.environ.get("PATH", "").split(os.pathsep):
        c = Path(d or ".") / "mnemo"
        if c.is_file() and os.access(c, os.X_OK) and str(c) not in out:
            out.append(str(c))
    return out


def reads_the_brain(path: str) -> str | None:
    """Why `path` can't open the brain with the secret this process has, or
    None when it can. The CLI lists no build features, so this asks it:
    `info` on the brain, read-only. A build without the `keychain` feature
    fails on a key brain with "none in this build"; that's the one failure
    that rules it out. A timeout is told apart (G16): after a new binary is
    installed, macOS asks again for the keychain, and `info` waits on that
    prompt. A scratch brain opened with MNEMO_PASSPHRASE needs no keychain,
    so it isn't asked."""
    if optional_passphrase() or not MNEMO_PATH.is_file():
        return None
    try:
        r = subprocess.run(
            [path, "info", str(MNEMO_PATH)], capture_output=True, text=True, timeout=60
        )
    except subprocess.TimeoutExpired:
        return (
            "timed out reading the brain's key: approve the keychain prompt "
            "for this binary, then run this again"
        )
    except (OSError, subprocess.SubprocessError) as e:
        return f"could not run it: {e}"
    if r.returncode != 0 and "none in this build" in r.stderr:
        return "no keychain in this build"
    return None


def mnemo_cli() -> str:
    """The `mnemo` to run: the first of $MNEMO_BIN, each `mnemo` on the PATH,
    and this repo's release build that is v0.15 or later and can read the
    brain's key (`reads_the_brain`). The PATH's come before the repo build:
    the installed release is the one the hooks and the MCP server run, with
    the keychain, while a local build may lack it (a v0.15.0 one did)."""
    global _CLI
    if _CLI:
        return _CLI
    candidates = [
        os.environ.get("MNEMO_BIN"),
        *path_clis(),
        str(REPO_ROOT / "mnemo" / "target" / "release" / "mnemo"),
    ]
    seen = []
    for c in candidates:
        if not c or not Path(c).is_file() or any(s.startswith(c + " ") for s in seen):
            continue
        v = cli_version(c)
        label = ".".join(map(str, v)) if v else "unknown version"
        if not v or v < MIN_CLI:
            seen.append(f"{c} ({label}, too old)")
            continue
        why = reads_the_brain(c)
        if why:
            seen.append(f"{c} ({label}, {why})")
            continue
        _CLI = c
        return c
    found = "(found: " + (", ".join(seen) or "none") + ")"
    if any("timed out" in s for s in seen):
        # G16: a keychain prompt is open; installing another build won't help.
        sys.exit(
            "no mnemo CLI read the brain's key in time " + found + ". macOS is "
            "likely asking whether this mnemo may use the keychain: approve the "
            "prompt, then run this again"
        )
    sys.exit(
        "no mnemo CLI at v0.15 or later that can read the brain's key "
        + found
        + ". Install one: curl -fsSL https://toarchkumar.github.io/mnemo/install.sh | sh, "
        "or build: cd mnemo && cargo build --release --features keychain,embed-candle, "
        "or set MNEMO_BIN"
    )


def run_cli(args: list[str]) -> str:
    """Run the CLI and return its stdout; its notes go to stderr."""
    load_dotenv()
    r = subprocess.run([mnemo_cli(), *args], capture_output=True, text=True)
    if r.stderr:
        sys.stderr.write(r.stderr)
    if r.returncode != 0:
        sys.exit(r.returncode)
    return r.stdout


REBUILD_HINT = (
    "Rebuild it into this venv (with the keychain, to open the project brain):\n"
    "  cd mnemo-python && VIRTUAL_ENV=\"$(cd ../test/.venv && pwd)\" "
    "PATH=\"$VIRTUAL_ENV/bin:$PATH\" maturin develop --release --features keychain,embed-candle"
)

# DOGFOOD-1: names the newest tree's binding has. A wheel built before them
# reports the same version number (it only moves at a release), so this is
# how a stale binding shows itself. Add the newest name here when the
# binding gains one this script relies on.
REQUIRED_NAMES = ["CancelToken"]
REQUIRED_METHODS = ["rekey_cleanup_error", "notices"]


def require_mnemo():
    try:
        import mnemo_engine as mnemo  # noqa: PLC0415
    except ImportError:
        sys.exit("mnemo Python package not installed. " + REBUILD_HINT)
    missing = [n for n in REQUIRED_NAMES if not hasattr(mnemo, n)]
    missing += [f"Mnemo.{m}" for m in REQUIRED_METHODS if not hasattr(mnemo.Mnemo, m)]
    if missing:
        sys.exit(
            f"the installed mnemo_engine ({mnemo.__version__}) is older than this tree "
            f"(missing {', '.join(missing)}). " + REBUILD_HINT
        )
    return mnemo


_MODEL = None


def embedder():
    """The sentence-transformers model, loaded once per process."""
    global _MODEL
    if _MODEL is not None:
        return _MODEL
    try:
        from sentence_transformers import SentenceTransformer  # noqa: PLC0415
    except ImportError:
        sys.exit(
            "sentence-transformers not installed. Run:\n"
            f"  pip install -r {TEST_ROOT / 'requirements.txt'}"
        )
    cache = TEST_ROOT / ".cache" / "embeddings"
    cache.mkdir(parents=True, exist_ok=True)
    os.environ.setdefault("SENTENCE_TRANSFORMERS_HOME", str(cache))
    _MODEL = SentenceTransformer(EMBED_MODEL)
    return _MODEL


def embed_texts(texts: list[str]) -> list[list[float]]:
    model = embedder()
    vectors = model.encode(texts, normalize_embeddings=True)
    return [v.tolist() for v in vectors]


def recorded_embedder(db) -> str | None:
    """The embedder name the brain's manifest records, if any."""
    for m in db.about():
        e = (m.get("metadata") or {}).get("embedder")
        if isinstance(e, dict) and e.get("name"):
            return e["name"]
    return None


def attach_embedder(db) -> None:
    """The built-in embedder for a brain that records it (or a new one),
    sentence-transformers' MiniLM for a brain that records MiniLM."""
    name = recorded_embedder(db)
    if name is None or name.startswith(BUILTIN_PREFIX):
        if hasattr(db, "use_builtin_embedder"):
            db.use_builtin_embedder()
            return
        if name is not None:
            sys.exit(
                "this brain uses the built-in embedder, and the installed wheel "
                "has none. " + REBUILD_HINT
            )
    db.set_embedder(
        embed_texts,
        name=EMBED_MODEL,
        dimensions=DIMENSIONS,
        normalize=True,
    )


def open_db(*, create: bool = False, write: bool = True):
    """Open the test database AND register the embedder on the handle.
    Phase 3 (PR 10a) added `db.set_embedder(fn, ...)` as a per-handle
    hook; registering once at open time means every subsequent
    `db.remember_text(...)` / `db.recall_text(...)` call uses the same
    embedder identity. The Rust core also persists the identity on the
    scaffold manifest, so swapping embedders between opens is caught
    loudly.

    DOGFOOD-1: reads (`write=False`) open read-only, so a Planner or a
    Reviewer (who must not write) can use them, alongside other readers.
    Writes open in shared mode (`shared=True`, with the default 10 s wait:
    a save keeps the default, since busy readers can hold a write longer
    than a read hook's 2 s; RB-008.1), so they take the lock per call and
    wait for other sessions instead of failing. Shared
    mode never creates a file, so a missing brain is created first with a
    plain open."""
    mnemo = require_mnemo()
    # The key-source chain finds the project brain's key in the keychain;
    # a scratch brain's passphrase comes from MNEMO_PASSPHRASE.
    pp = optional_passphrase()
    if create or not MNEMO_PATH.exists():
        if not write:
            sys.exit(f"no brain at {MNEMO_PATH}")
        if MNEMO_PATH.exists() and create:
            MNEMO_PATH.unlink()
        mnemo.open(str(MNEMO_PATH), passphrase(), dimensions=DIMENSIONS).close()
    if write:
        db = mnemo.open(str(MNEMO_PATH), pp, shared=True)
    else:
        db = mnemo.open(str(MNEMO_PATH), pp, read_only=True)
    attach_embedder(db)
    return db


def cmd_bootstrap(_: argparse.Namespace) -> None:
    """Create a fresh database, register the embedder, insert every
    seed entry via `remember_text`. We still export the resulting
    JSONL (for cross-tool diffing) by running `embed_texts` once up
    front — the Phase 3 text path moves embedding to the engine side
    at insert time, so the JSONL is just an aid, not the input."""
    if not SEED_PATH.exists():
        sys.exit(f"seed file not found: {SEED_PATH}")
    if not BRAIN_GIVEN or MNEMO_PATH == default_brain().resolve():
        sys.exit(
            f"bootstrap wipes the brain it builds, so it won't touch the project brain "
            f"({default_brain()}). Give a scratch brain: --brain /tmp/scratch.mnemo, with "
            "MNEMO_PASSPHRASE set for it"
        )
    entries = json.loads(SEED_PATH.read_text(encoding="utf-8"))
    print(f"embedding {len(entries)} seed memories (dim={DIMENSIONS})...")

    mnemo = require_mnemo()
    if MNEMO_PATH.exists():
        MNEMO_PATH.unlink()
    db = mnemo.open(str(MNEMO_PATH), passphrase(), dimensions=DIMENSIONS)
    # A new brain: the built-in embedder when the wheel has it.
    attach_embedder(db)

    jsonl_lines: list[str] = []
    for entry in entries:
        content = entry["content"]
        mt = entry.get("memory_type", "semantic")
        importance = float(entry.get("importance", 0.7))
        metadata = dict(entry.get("metadata") or {})
        # v0.17: the manifest's default agent is a reserved key the engine
        # sets itself, so it's applied after the write.
        default_agent = metadata.pop("_mnemo.default_agent", None)
        kw = {"key": entry["key"]} if entry.get("key") else {}
        r = db.remember_text(
            content,
            mt,
            agent_id=AGENT_ID,
            importance=importance,
            metadata=metadata,
            **kw,
        )
        # A keyed write returns {"id", "outcome"}.
        mid = r["id"] if isinstance(r, dict) else r
        if default_agent and hasattr(db, "set_default_agent"):
            db.set_default_agent(default_agent)
        vector = db.get(mid)["vector"]  # for the JSONL export below
        jsonl_lines.append(
            json.dumps(
                {
                    "content": content,
                    "vector": vector,
                    "memory_type": mt,
                    "agent_id": AGENT_ID,
                    "importance": importance,
                    "metadata": metadata,
                }
            )
        )

    db.flush()
    if len(db) >= 32:
        print("building ANN index...")
        db.build_index()
        db.flush()
    db.close()

    JSONL_PATH.write_text("\n".join(jsonl_lines) + "\n", encoding="utf-8")
    print(f"bootstrapped {len(entries)} memories -> {MNEMO_PATH}")
    print(f"exported vectors -> {JSONL_PATH}")


def remember_args(
    content: str,
    mtype: str,
    importance: float,
    metadata: dict | None,
    key: str | None,
    supersedes: str | None = None,
) -> list[str]:
    """`mnemo remember`'s arguments for one fact. With a key it's the
    keyed upsert (Phase 12.1): an identical re-write is a no-op, and a new
    value supersedes the old one, which stays in `history --key`."""
    a = [
        "remember",
        str(MNEMO_PATH),
        "--text",
        content,
        "--type",
        mtype,
        "--importance",
        str(importance),
        "--agent",
        AGENT_ID,
    ]
    if metadata:
        a += ["--metadata", json.dumps(metadata)]
    if key:
        a += ["--key", key]
    if supersedes:
        a += ["--supersedes", supersedes]
    return a


def cmd_remember(args: argparse.Namespace) -> None:
    # The CLI prints `id\toutcome` for a keyed write, the id for a plain one.
    out = run_cli(
        remember_args(
            args.content,
            args.type,
            args.importance,
            json.loads(args.metadata) if args.metadata else None,
            args.key,
            args.supersedes,
        )
    )
    print(out.strip())


def cmd_remember_file(args: argparse.Namespace) -> None:
    """DOGFOOD-1: many facts in one process, with one model load, one
    embedding call and one open. Each line of the JSONL file is an object
    with `content` and optionally `type`, `importance`, `metadata`, `key`
    and `supersedes` (with `key`: the id of an unkeyed memory this write
    adopts and supersedes). Every fact is still its own write (one window
    each)."""
    path = Path(args.from_file).expanduser().resolve()
    facts = []
    for i, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        f = json.loads(line)
        if not isinstance(f, dict) or not f.get("content"):
            sys.exit(f"{path}:{i}: each line needs an object with 'content'")
        facts.append(f)
    if not facts:
        sys.exit(f"{path}: no facts")
    # One CLI run per fact, each its own write.
    for f in facts:
        out = run_cli(
            remember_args(
                f["content"],
                f.get("type", "semantic"),
                float(f.get("importance", 0.75)),
                f.get("metadata"),
                f.get("key"),
                f.get("supersedes"),
            )
        )
        print(out.strip())


def cmd_recall(args: argparse.Namespace) -> None:
    # `--no-track`: a read that doesn't count as an access, under a shared
    # lock, so the Planner and the Reviewers can run it (they never write).
    # Scoring v2 counts an observed recall toward usage on later queries;
    # for a runbook that reads the coordination memories dozens of times a
    # day that's noise, so the dogfood recall never tracks.
    a = [
        "recall",
        str(MNEMO_PATH),
        "--text",
        args.query,
        "--top-k",
        str(args.top_k),
        "--no-track",
        "--format",
        "json",
        "--agent",
        AGENT_ID,
    ]
    if args.types:
        a += ["--type", args.types]
    # DOGFOOD-1: scope by `metadata.area`, and add BM25 keyword matching
    # for identifiers and exact terms (a path, a version, a name).
    if args.area:
        a += ["--filter", f"area={args.area}"]
    if args.keywords:
        a += ["--keywords", args.keywords]
    out = run_cli(a)
    hits = []
    for h in json.loads(out).get("hits", []):
        m = dict(h["memory"])
        m["memory_type"] = str(m.get("memory_type", "")).lower()
        m["score"] = h["score"]
        m["similarity"] = h["similarity"]
        hits.append(m)

    if args.format == "json":
        print(json.dumps(hits, indent=2))
        return

    if not hits:
        print("(no memories matched)")
        return

    print(f"# Project memory recall ({len(hits)} hits)\n")
    for i, h in enumerate(hits, 1):
        meta = h.get("metadata") or {}
        area = meta.get("area", "")
        tag = f" [{area}]" if area else ""
        print(f"## {i}. score={h['score']:.3f} sim={h['similarity']:.3f} ({h['memory_type']}){tag}")
        print(h["content"])
        print()


def cmd_perf(args: argparse.Namespace) -> None:
    """Ingest a JSON file of performance measurements as episodic memories.

    Each entry is stored as one memory with metadata.area="performance" so
    later sessions can recall by version, metric, or build (and human
    readers see one durable observation per call).

    Expected JSON shape — a list of objects, each with at least:
        version     short tag, e.g. "v0.1.0"
        measured_at ISO date or unix timestamp, e.g. "2026-05-25"
        metric      machine-friendly key, e.g. "cli_info_latency_ms"
        label       one-line human description
        value       numeric measurement (number or null)
        units       e.g. "ms", "MB", "%"
        build       what was measured, e.g. "debug-cli" / "release-python"
        corpus      what the measurement was taken against
        notes       (optional) caveats, context, or interpretation
    """
    path = Path(args.file).expanduser().resolve()
    if not path.exists():
        sys.exit(f"perf file not found: {path}")
    try:
        entries = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        sys.exit(f"{path}: invalid JSON: {e}")
    if not isinstance(entries, list) or not entries:
        sys.exit(f"{path}: expected a non-empty JSON array of perf entries")

    rendered: list[tuple[str, dict]] = []
    for i, e in enumerate(entries):
        if not isinstance(e, dict):
            sys.exit(f"{path}[{i}]: expected an object, got {type(e).__name__}")
        for required in ("version", "metric", "label"):
            if required not in e:
                sys.exit(f"{path}[{i}]: missing required key '{required}'")
        version = str(e["version"])
        metric = str(e["metric"])
        label = str(e["label"])
        value = e.get("value")
        units = e.get("units") or ""
        build = e.get("build", "unspecified")
        corpus = e.get("corpus", "unspecified")
        notes = e.get("notes", "")
        measured_at = e.get("measured_at", "")

        value_str = "n/a" if value is None else f"{value} {units}".strip()
        date_str = f", {measured_at}" if measured_at else ""
        notes_tail = f" — {notes}" if notes else ""
        content = (
            f"PERF [{version}{date_str}] {label}: {value_str} "
            f"(build={build}; corpus={corpus}){notes_tail}"
        )
        metadata = {
            "area": "performance",
            "topic": metric,
            "version": version,
            "metric": metric,
            "value": value,
            "units": units,
            "build": build,
            "corpus": corpus,
            "measured_at": measured_at,
        }
        if notes:
            metadata["notes"] = notes
        rendered.append((content, metadata))

    db = open_db()
    written: list[str] = []
    for content, metadata in rendered:
        mid = db.remember_text(
            content,
            "episodic",
            agent_id=AGENT_ID,
            importance=args.importance,
            metadata=metadata,
        )
        written.append(mid)
        print(f"  + {metadata['version']}/{metadata['metric']}  {mid}")
    db.flush()
    db.close()
    print(f"ingested {len(written)} perf memories from {path}")


def cmd_info(_: argparse.Namespace) -> None:
    db = open_db(write=False)
    stats = db.stats()
    has_index = db.has_index()
    snaps = db.snapshots()
    db.close()
    print(f"path:       {MNEMO_PATH}")
    print(f"memories:   {stats['memories']}")
    print(f"dimensions: {stats['dimensions']}")
    print(f"file_bytes: {stats['file_bytes']}")
    print(f"has_index:  {has_index}")
    print(f"snapshots:  {len(snaps)}")


BRAIN_GIVEN = False


def main() -> None:
    global MNEMO_PATH, BRAIN_GIVEN
    parser = argparse.ArgumentParser(description="MNemo project memory (dogfood)")
    parser.add_argument(
        "--brain",
        help=f"the brain to use (default {MNEMO_PATH}); a scratch brain for trials, "
        "opened with MNEMO_PASSPHRASE from the environment",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_boot = sub.add_parser(
        "bootstrap",
        help="Build a scratch brain (--brain) from seed.json; never the project brain",
    )
    p_boot.set_defaults(func=cmd_bootstrap)

    p_rem = sub.add_parser("remember", help="Store one new memory, or a file of them")
    p_rem.add_argument("content", nargs="?")
    p_rem.add_argument(
        "--from-file",
        metavar="JSONL",
        help="store every fact in a JSON Lines file (content, type, importance, "
        "metadata, key per line) in one process",
    )
    p_rem.add_argument(
        "--type",
        default="semantic",
        choices=["episodic", "semantic", "procedural", "working"],
    )
    p_rem.add_argument("--importance", type=float, default=0.75)
    p_rem.add_argument("--metadata", help='JSON object, e.g. \'{"area":"dogfood"}\'')
    p_rem.add_argument(
        "--key",
        help=(
            "Phase 12.1 upsert slot. A later `remember --key K` with new "
            "content supersedes the prior record; an identical re-write is "
            "a no-op. Omit for a plain insert."
        ),
    )
    p_rem.add_argument(
        "--supersedes",
        metavar="ID",
        help=(
            "v0.15, with --key: the id of a memory written without a key. "
            "This write adopts it as the key's first version and supersedes "
            "it, so it stops showing in recall but stays in `history --key`."
        ),
    )
    p_rem.set_defaults(func=lambda a: cmd_remember_file(a) if a.from_file else cmd_remember(a))

    p_rec = sub.add_parser("recall", help="Retrieve context for a query")
    p_rec.add_argument("query")
    p_rec.add_argument("--top-k", type=int, default=8)
    p_rec.add_argument("--types", help="Comma-separated memory types filter")
    p_rec.add_argument("--format", choices=["md", "json"], default="md")
    p_rec.add_argument("--area", help="only memories whose metadata.area is this")
    p_rec.add_argument(
        "--keywords",
        help="identifiers or exact terms to match too (BM25), e.g. a path or a version",
    )
    p_rec.set_defaults(func=cmd_recall)

    p_info = sub.add_parser("info", help="Show database stats")
    p_info.set_defaults(func=cmd_info)

    p_perf = sub.add_parser(
        "perf",
        help="Ingest a JSON file of perf measurements as episodic memories",
    )
    p_perf.add_argument("file", help="Path to a JSON list of perf entries")
    p_perf.add_argument(
        "--importance",
        type=float,
        default=0.7,
        help="Importance score for ingested perf memories (default: 0.7)",
    )
    p_perf.set_defaults(func=cmd_perf)

    args = parser.parse_args()
    if args.cmd == "remember" and not args.from_file and not args.content:
        parser.error("remember needs CONTENT or --from-file")
    if args.brain:
        MNEMO_PATH = Path(args.brain).expanduser().resolve()
        BRAIN_GIVEN = True
    args.func(args)


if __name__ == "__main__":
    main()
