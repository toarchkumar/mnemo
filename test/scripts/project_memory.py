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
EMBED_MODEL = "all-MiniLM-L6-v2"


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


def require_mnemo():
    try:
        import mnemo  # noqa: PLC0415
    except ImportError:
        sys.exit(
            "mnemo Python package not installed. Run:\n"
            "  cd mnemo-python && pip install maturin && maturin develop"
        )
    return mnemo


def embedder():
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
    return SentenceTransformer(EMBED_MODEL)


def embed_texts(texts: list[str]) -> list[list[float]]:
    model = embedder()
    vectors = model.encode(texts, normalize_embeddings=True)
    return [v.tolist() for v in vectors]


def open_db(*, create: bool = False):
    mnemo = require_mnemo()
    pp = passphrase()
    if create or not MNEMO_PATH.exists():
        if MNEMO_PATH.exists() and create:
            MNEMO_PATH.unlink()
        return mnemo.open(str(MNEMO_PATH), pp, dimensions=DIMENSIONS)
    return mnemo.open(str(MNEMO_PATH), pp)


def cmd_bootstrap(_: argparse.Namespace) -> None:
    if not SEED_PATH.exists():
        sys.exit(f"seed file not found: {SEED_PATH}")
    entries = json.loads(SEED_PATH.read_text(encoding="utf-8"))
    texts = [e["content"] for e in entries]
    print(f"embedding {len(texts)} seed memories ({EMBED_MODEL}, dim={DIMENSIONS})...")
    vectors = embed_texts(texts)

    mnemo = require_mnemo()
    if MNEMO_PATH.exists():
        MNEMO_PATH.unlink()
    db = mnemo.open(str(MNEMO_PATH), passphrase(), dimensions=DIMENSIONS)

    jsonl_lines: list[str] = []
    for entry, vector in zip(entries, vectors):
        content = entry["content"]
        mt = entry.get("memory_type", "semantic")
        importance = float(entry.get("importance", 0.7))
        metadata = entry.get("metadata") or {}
        db.remember(
            content,
            mt,
            vector,
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
    vector = embed_texts([args.content])[0]
    db = open_db()
    mid = db.remember(
        args.content,
        args.type,
        vector,
        agent_id=AGENT_ID,
        importance=args.importance,
        metadata=json.loads(args.metadata) if args.metadata else None,
    )
    db.flush()
    db.close()
    print(mid)


def cmd_recall(args: argparse.Namespace) -> None:
    query_vec = embed_texts([args.query])[0]
    db = open_db()
    hits = db.recall(
        query_vec,
        top_k=args.top_k,
        agent_id=AGENT_ID,
        memory_types=args.types.split(",") if args.types else None,
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


def cmd_info(_: argparse.Namespace) -> None:
    db = open_db()
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
    parser = argparse.ArgumentParser(description="MNemo project memory (dogfood)")
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_boot = sub.add_parser("bootstrap", help="Create project.mnemo from seed.json")
    p_boot.set_defaults(func=cmd_bootstrap)

    p_rem = sub.add_parser("remember", help="Store one new memory")
    p_rem.add_argument("content")
    p_rem.add_argument(
        "--type",
        default="semantic",
        choices=["episodic", "semantic", "procedural", "working"],
    )
    p_rem.add_argument("--importance", type=float, default=0.75)
    p_rem.add_argument("--metadata", help='JSON object, e.g. \'{"area":"dogfood"}\'')
    p_rem.set_defaults(func=cmd_remember)

    p_rec = sub.add_parser("recall", help="Retrieve context for a query")
    p_rec.add_argument("query")
    p_rec.add_argument("--top-k", type=int, default=8)
    p_rec.add_argument("--types", help="Comma-separated memory types filter")
    p_rec.add_argument("--format", choices=["md", "json"], default="md")
    p_rec.set_defaults(func=cmd_recall)

    p_info = sub.add_parser("info", help="Show database stats")
    p_info.set_defaults(func=cmd_info)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
