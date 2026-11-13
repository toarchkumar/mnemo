#!/usr/bin/env python3
"""MNemo project memory — dogfood helper for test/project.mnemo."""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

TEST_ROOT = Path(__file__).resolve().parent.parent
MNEMO_PATH = TEST_ROOT / "project.mnemo"
JSONL_PATH = TEST_ROOT / "project-memory.jsonl"
SEED_PATH = Path(__file__).resolve().parent / "seed.json"
ENV_PATH = TEST_ROOT / ".env"

AGENT_ID = "cursor-agent"
DIMENSIONS = 384
# Must match the embedder.name recorded on the dogfood manifest
# (test/scripts/seed.json → test/project.mnemo). PR 10a.1's identity
# check is strict exact-match on the nested metadata.embedder.name;
# sentence-transformers accepts both the short and HF-prefixed forms
# and loads the same weights, so vectors are unchanged.
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
    load_dotenv()
    pp = os.environ.get("MNEMO_PASSPHRASE", "").strip()
    if not pp:
        sys.exit(
            "MNEMO_PASSPHRASE missing. Copy test/.env.example to test/.env or export it."
        )
    return pp


REBUILD_HINT = (
    "Rebuild it into this venv:\n"
    "  cd mnemo-python && VIRTUAL_ENV=\"$(cd ../test/.venv && pwd)\" "
    "PATH=\"$VIRTUAL_ENV/bin:$PATH\" maturin develop --release"
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


# How long a write waits for the brain's lock (12.4 shared mode): a session
# hook's figure, so a busy file delays a call by at most about two seconds.
WRITE_WAIT_SECS = 2.0


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
    Writes open in shared mode (`shared=True, wait=2.0`), so they take the
    lock per call and wait for other sessions instead of failing. Shared
    mode never creates a file, so a missing brain is created first with a
    plain open."""
    mnemo = require_mnemo()
    pp = passphrase()
    if create or not MNEMO_PATH.exists():
        if not write:
            sys.exit(f"no brain at {MNEMO_PATH}")
        if MNEMO_PATH.exists() and create:
            MNEMO_PATH.unlink()
        mnemo.open(str(MNEMO_PATH), pp, dimensions=DIMENSIONS).close()
    if write:
        db = mnemo.open(str(MNEMO_PATH), pp, shared=True, wait=WRITE_WAIT_SECS)
    else:
        db = mnemo.open(str(MNEMO_PATH), pp, read_only=True)
    db.set_embedder(
        embed_texts,
        name=EMBED_MODEL,
        dimensions=DIMENSIONS,
        normalize=True,
    )
    return db


def cmd_bootstrap(_: argparse.Namespace) -> None:
    """Create a fresh database, register the embedder, insert every
    seed entry via `remember_text`. We still export the resulting
    JSONL (for cross-tool diffing) by running `embed_texts` once up
    front — the Phase 3 text path moves embedding to the engine side
    at insert time, so the JSONL is just an aid, not the input."""
    if not SEED_PATH.exists():
        sys.exit(f"seed file not found: {SEED_PATH}")
    entries = json.loads(SEED_PATH.read_text(encoding="utf-8"))
    texts = [e["content"] for e in entries]
    print(f"embedding {len(texts)} seed memories ({EMBED_MODEL}, dim={DIMENSIONS})...")
    vectors = embed_texts(texts)  # kept only for the JSONL export below

    mnemo = require_mnemo()
    if MNEMO_PATH.exists():
        MNEMO_PATH.unlink()
    db = mnemo.open(str(MNEMO_PATH), passphrase(), dimensions=DIMENSIONS)
    db.set_embedder(embed_texts, name=EMBED_MODEL, dimensions=DIMENSIONS, normalize=True)

    jsonl_lines: list[str] = []
    for entry, vector in zip(entries, vectors):
        content = entry["content"]
        mt = entry.get("memory_type", "semantic")
        importance = float(entry.get("importance", 0.7))
        metadata = entry.get("metadata") or {}
        db.remember_text(
            content,
            mt,
            agent_id=AGENT_ID,
            importance=importance,
            metadata=metadata,
        )
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


def cmd_remember(args: argparse.Namespace) -> None:
    db = open_db()
    # Phase 12.1 (v0.8.0): if --key is given, take the keyed-upsert
    # path. A repeat of an identical write is a no-op (helpful when a
    # script re-emits the same "current X" fact), and a new value
    # under the same key supersedes the prior record but keeps it
    # walkable through `project_memory.py history --key`. Without
    # --key the write stays as a plain insert so pre-12.1 callers
    # don't change behavior.
    result = db.remember_text(
        args.content,
        args.type,
        agent_id=AGENT_ID,
        importance=args.importance,
        metadata=json.loads(args.metadata) if args.metadata else None,
        key=args.key,
    )
    db.flush()
    db.close()
    if isinstance(result, dict):
        # Keyed write — print `id\toutcome` so scripts can branch on it.
        print(f"{result['id']}\t{result['outcome']}")
    else:
        # Un-keyed insert — preserve the pre-12.1 stdout shape (just the ULID).
        print(result)


def cmd_remember_file(args: argparse.Namespace) -> None:
    """DOGFOOD-1: many facts in one process, with one model load, one
    embedding call and one open. Each line of the JSONL file is an object
    with `content` and optionally `type`, `importance`, `metadata` and
    `key`. Every fact is still its own write (one window each)."""
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
    vectors = embed_texts([f["content"] for f in facts])
    db = open_db()
    for f, vector in zip(facts, vectors):
        result = db.remember(
            f["content"],
            f.get("type", "semantic"),
            vector,
            agent_id=AGENT_ID,
            importance=float(f.get("importance", 0.75)),
            metadata=f.get("metadata"),
            key=f.get("key"),
        )
        if isinstance(result, dict):
            print(f"{result['id']}\t{result['outcome']}")
        else:
            print(result)
    db.close()


def cmd_recall(args: argparse.Namespace) -> None:
    db = open_db(write=False)
    # Scoring v2 (v0.7.0) made recall's own observation count toward
    # the usage saturation term on later queries. For the dogfood
    # runbook that reads the planner/worker coordination memories
    # dozens of times a day, that feedback is noise, not signal:
    # every recall would "vote for" the memories it just surfaced.
    # Keep tracking off here — the CLI operator still gets exact
    # scoring, but the recency / usage signals stay driven by real
    # writes, not by how often we grep our own memory.
    hits = db.recall_text(
        args.query,
        top_k=args.top_k,
        agent_id=AGENT_ID,
        memory_types=args.types.split(",") if args.types else None,
        track_access=False,
        # DOGFOOD-1: scope by `metadata.area`, and add BM25 keyword matching
        # for identifiers and exact terms (a path, a version, a name).
        filter={"metadata": {"area": args.area}} if args.area else None,
        keywords=args.keywords,
    )
    db.close()

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


def main() -> None:
    global MNEMO_PATH
    parser = argparse.ArgumentParser(description="MNemo project memory (dogfood)")
    parser.add_argument(
        "--brain",
        help=f"the brain to use (default {MNEMO_PATH}); a scratch brain for trials. "
        "MNEMO_PASSPHRASE in the environment overrides test/.env",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_boot = sub.add_parser("bootstrap", help="Create project.mnemo from seed.json")
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
    args.func(args)


if __name__ == "__main__":
    main()
