"""Generate the fixture JSON that the scoring v2 Rust harness consumes.

Produces two files under ``test/scripts/`` next to this script:

* ``ranking_v2_phase6_fixture.json`` — the exact Phase 6 corpus +
  queries that ``ranking_harness.py`` already uses, embedded once
  here so the Rust harness can replay V0 against it as a sanity
  gate. V0 on this fixture must reproduce the Phase 6 numbers in
  ``perf_v0.5.0.json["ranking"]`` within noise; if it doesn't, the
  Rust harness logs BLOCKED and refuses to run the age-spread
  measurement.

* ``ranking_v2_fixture.json`` — the new, age-spread corpus. Every
  memory carries a ``created_at_offset_secs`` field (how long
  before "now" the harness will backdate it), varied importance,
  and superseded-fact pairs so queries like "what's the current X?"
  have an unambiguous correct answer the harness can score.

Why Python runs this (and nothing else): embedding needs
``sentence-transformers`` + torch, which the Rust side does not
link. Everything downstream of embedding — building the ``.mnemo``
file with backdated memories, running the variants through
``RecallRequest``, computing hit@1/@5/MRR/drift, writing the
``ranking_v2`` block to ``perf_v0.5.0.json`` — runs in Rust,
through the public API (NAME-1 Planner DECISION 2026-11-05: no new
timestamp-setter API; the harness just assigns to the already-pub
``Memory.created_at`` / ``Memory.accessed_at`` fields before
``remember``).

The generated fixtures are committed so re-runs of the harness are
deterministic without a fresh embedding pass.

Usage::

    python test/scripts/gen_ranking_v2_fixture.py

Dependencies: ``sentence-transformers`` (plus torch). Same as
``ranking_harness.py``; the dogfood workflow installs them.
"""

from __future__ import annotations

import json
import sys
from dataclasses import dataclass, field
from pathlib import Path

# The Phase 6 corpus + queries live in ranking_harness.py; import so the
# fixture tracks any drift there (if either changes, re-run this script
# and commit the regenerated fixtures).
_THIS_DIR = Path(__file__).parent
sys.path.insert(0, str(_THIS_DIR))
from ranking_harness import CORPUS as PHASE6_CORPUS  # noqa: E402
from ranking_harness import QUERIES as PHASE6_QUERIES  # noqa: E402
from ranking_harness import DIMENSIONS, MODEL_NAME, NORMALIZE  # noqa: E402

from sentence_transformers import SentenceTransformer  # noqa: E402


# ----------------------------------------------------------------------
# Age-spread corpus
# ----------------------------------------------------------------------
#
# Every entry carries a ``created_at_offset_secs`` — how many seconds
# before the harness's "now" the memory should look like it was
# written. The Rust harness subtracts this from `Utc::now()` and
# assigns both `Memory.created_at` and `Memory.accessed_at` to that
# value before `remember`, so the recency clock has something to
# decay.
#
# The spread is deliberately long-tailed:
# - 4 memories ≤ 1 day old (current API, current release)
# - 6 memories 3–14 days old (recent PRs, recent decisions)
# - 6 memories 1–3 months old (older decisions, mid-lifecycle PRs)
# - 6 memories 6+ months old (ancient releases, legacy decisions)
# - noise padding (older, lower importance)
#
# **Superseded-fact pairs** are the critical addition. Each pair is
# two memories about the *same* subject at different times:
# - an older version (lower importance, older created_at) carrying
#   the old claim verbatim;
# - a newer version (higher importance, fresh created_at) carrying
#   the current claim.
# The paired query asks "what's the current X?" and the only correct
# answer is the newer memory. Any formula that ranks by recency with
# `created_at` as the clock should pass these; the shipped V0, which
# uses `accessed_at`, may not — that's the signal scoring v2 exists
# to make measurable.


DAY = 24 * 3600
MONTH = 30 * DAY


@dataclass
class LabeledMemory:
    label: str
    content: str
    importance: float
    created_at_offset_secs: int
    agent_id: str = "assistant"


@dataclass
class LabeledQuery:
    text: str
    category: str
    # Every query names at least one expected memory. Superseded-fact
    # queries only accept the newer member of the pair.
    expected_labels: list[str] = field(default_factory=list)


# --- Memories --------------------------------------------------------
#
# Mnemonic prefixes:
#   fresh:*    — ≤1 day old
#   recent:*   — 3–14 days old
#   mid:*      — 1–3 months old
#   old:*      — 6+ months old
#   noise:*    — distractors, varied importance, varied age
#   sup:<key>:current / sup:<key>:past — superseded-fact pairs

SUPERSEDED_FACT_PAIRS: list[tuple[LabeledMemory, LabeledMemory, str]] = [
    (
        LabeledMemory(
            label="sup:release:current",
            content=(
                "Current mnemo release is v0.7.0 — the import name is now "
                "mnemo_engine (NAME-1 shipped the rename). Recall scoring v2 "
                "also landed here."
            ),
            importance=0.95,
            created_at_offset_secs=1 * DAY,
        ),
        LabeledMemory(
            label="sup:release:past",
            content=(
                "Current mnemo release is v0.6.1 — the import name is "
                "mnemo. BP.1 added mnemo index --rebuild and db.rebuild_index()."
            ),
            importance=0.6,
            created_at_offset_secs=14 * DAY,
        ),
        "what's the current mnemo release?",
    ),
    (
        LabeledMemory(
            label="sup:ann_default:current",
            content=(
                "The default ANN index now uses pq_subspaces = dims / 4 "
                "(96 for the 384-d MiniLM embedder). Recall@10 is ~0.98 at "
                "100K memories after this change."
            ),
            importance=0.9,
            created_at_offset_secs=2 * DAY,
        ),
        LabeledMemory(
            label="sup:ann_default:past",
            content=(
                "The default ANN index uses pq_subspaces = dims / 8 "
                "(48 for 384-d vectors), giving ~0.40 recall@10 at 100K "
                "memories."
            ),
            importance=0.5,
            created_at_offset_secs=30 * DAY,
        ),
        "what are the current ANN index defaults?",
    ),
    (
        LabeledMemory(
            label="sup:lib_name:current",
            content=(
                "The Rust library name is mnemo_engine (Cargo's default from "
                "the mnemo-engine package name). CLI binary is still mnemo."
            ),
            importance=0.9,
            created_at_offset_secs=1 * DAY,
        ),
        LabeledMemory(
            label="sup:lib_name:past",
            content=(
                "The Rust library name is mnemo (set via [lib] name override "
                "in Cargo.toml), even though the crate itself is mnemo-engine."
            ),
            importance=0.5,
            created_at_offset_secs=21 * DAY,
        ),
        "what is the Rust library name?",
    ),
    (
        LabeledMemory(
            label="sup:fsyncs:current",
            content=(
                "A steady-state flush now costs 3 fsyncs — the pre-flush "
                "leased-header write is gone after DUR-1."
            ),
            importance=0.85,
            created_at_offset_secs=3 * DAY,
        ),
        LabeledMemory(
            label="sup:fsyncs:past",
            content=(
                "A steady-state flush costs 4 fsyncs — data pages, WAL "
                "commit, checkpoint, and the pre-flush leased-header write."
            ),
            importance=0.4,
            created_at_offset_secs=45 * DAY,
        ),
        "how many fsyncs does a flush cost right now?",
    ),
    (
        LabeledMemory(
            label="sup:compact_cache:current",
            content=(
                "mnemo compact preserves the result cache — FIX-1 fixed the "
                "pre-v0.6.0 behaviour where compaction silently emptied it."
            ),
            importance=0.8,
            created_at_offset_secs=5 * DAY,
        ),
        LabeledMemory(
            label="sup:compact_cache:past",
            content=(
                "mnemo compact rewrites the file but empties the result "
                "cache; cached values are recomputed on next use."
            ),
            importance=0.5,
            created_at_offset_secs=40 * DAY,
        ),
        "does compaction keep the result cache?",
    ),
]


FRESH_MEMORIES: list[LabeledMemory] = [
    LabeledMemory(
        label="fresh:planner_dispatch",
        content=(
            "Planner dispatched scoring v2: Python only embeds, Rust runs "
            "everything else, V1–V5 behind a hidden selector, ship only the "
            "winner, query-time only (no format bump)."
        ),
        importance=0.85,
        created_at_offset_secs=4 * 3600,
    ),
    LabeledMemory(
        label="fresh:option_a_rationale",
        content=(
            "Option A chosen for NAME-1: drop the import mnemo alias so the "
            "wheel ships no top-level mnemo/ package, ending the collision "
            "with mnemo-db and PyPI mnemo in one release instead of two."
        ),
        importance=0.8,
        created_at_offset_secs=6 * 3600,
    ),
]

RECENT_MEMORIES: list[LabeledMemory] = [
    LabeledMemory(
        label="recent:bp1",
        content=(
            "BP.1 added CLI mnemo index --rebuild and Python db.rebuild_index(); "
            "both call Mnemo::rebuild_index on the core which re-runs "
            "build_index_with(IndexConfig::default())."
        ),
        importance=0.7,
        created_at_offset_secs=3 * DAY,
    ),
    LabeledMemory(
        label="recent:dur1",
        content=(
            "DUR-1 landed Option B: a validate_seal failure is always "
            "HeaderTampered and never auto-healed; recovery heals only on "
            "parse errors (BadMagic, UnsupportedVersion, HeaderChecksum)."
        ),
        importance=0.9,
        created_at_offset_secs=7 * DAY,
    ),
    LabeledMemory(
        label="recent:fix1",
        content=(
            "FIX-1 landed the correctness sweep: compact preserves the "
            "cache, export_encrypted refuses live-file overwrite, kdf_params "
            "accessor, EmbedCmd rejects zero/non-finite, lazy agent set."
        ),
        importance=0.8,
        created_at_offset_secs=9 * DAY,
    ),
    LabeledMemory(
        label="recent:py1",
        content=(
            "PY-1 made db.close() release the OS file lock synchronously + "
            "released the GIL on flush, recall, verify, rekey, build_index."
        ),
        importance=0.8,
        created_at_offset_secs=11 * DAY,
    ),
    LabeledMemory(
        label="recent:ann_measured_numbers",
        content=(
            "Measured on M1 Pro, hardware AES, v0.6.1: indexed recall at "
            "~1.4ms / ~9.4ms / ~134ms for 10K / 100K / 1M memories."
        ),
        importance=0.65,
        created_at_offset_secs=6 * DAY,
    ),
    LabeledMemory(
        label="recent:write_bench",
        content=(
            "Steady-state write benches on v0.6.1: 18.9 ms per single flush, "
            "24.7 ms per batch-of-100 flush — about -13%/-11% vs v0.5.1 "
            "after DUR-1 removed one fsync per commit."
        ),
        importance=0.6,
        created_at_offset_secs=8 * DAY,
    ),
]

MID_MEMORIES: list[LabeledMemory] = [
    LabeledMemory(
        label="mid:name1_rename",
        content=(
            "NAME-1 renamed the lib and Python import from mnemo to "
            "mnemo_engine to end the collision with mnemo-db."
        ),
        importance=0.7,
        created_at_offset_secs=35 * DAY,
    ),
    LabeledMemory(
        label="mid:wal_semantics",
        content=(
            "The WAL is a single-fsync commit: data pages copy-on-write, "
            "control plane logged, one fsync makes the transaction durable, "
            "then a checkpoint folds frames into their home pages."
        ),
        importance=0.75,
        created_at_offset_secs=50 * DAY,
    ),
    LabeledMemory(
        label="mid:ann_arch",
        content=(
            "ANN index is IVF + Product Quantization: k-means over the "
            "vectors gives n_partitions centroids; each partition's vectors "
            "are PQ-coded for a compact second-level index."
        ),
        importance=0.65,
        created_at_offset_secs=60 * DAY,
    ),
    LabeledMemory(
        label="mid:crypto_layers",
        content=(
            "Two-tier key hierarchy: Argon2id derives a KEK from the "
            "passphrase; AES-GCM wraps the random DEK with the KEK; the "
            "DEK encrypts every page under per-session random nonces."
        ),
        importance=0.8,
        created_at_offset_secs=70 * DAY,
    ),
    LabeledMemory(
        label="mid:session_api",
        content=(
            "The Session API records conversation turns as working memory; "
            "on close it consolidates turns into episodic memory via the "
            "normal remember path."
        ),
        importance=0.55,
        created_at_offset_secs=55 * DAY,
    ),
    LabeledMemory(
        label="mid:mcp_server",
        content=(
            "mnemo serve --mcp runs the MCP JSON-RPC server over stdio; "
            "clients can register tools for remember, recall, cache_get, "
            "cache_put, compact."
        ),
        importance=0.5,
        created_at_offset_secs=48 * DAY,
    ),
]

OLD_MEMORIES: list[LabeledMemory] = [
    LabeledMemory(
        label="old:v0_1_0",
        content=(
            "v0.1.0 was the initial public release of mnemo: four memory "
            "types, multi-signal recall, Python bindings, CLI."
        ),
        importance=0.3,
        created_at_offset_secs=200 * DAY,
    ),
    LabeledMemory(
        label="old:sqlite_analogy",
        content=(
            "Mnemo takes the SQLite position for agent memory: one file, "
            "no server, no daemon, no schema migration."
        ),
        importance=0.4,
        created_at_offset_secs=180 * DAY,
    ),
    LabeledMemory(
        label="old:phase6_methodology",
        content=(
            "Phase 6 benchmarks measure cold recall, warm recall, flush "
            "latency, and ANN recall@10 against an exact-scan baseline."
        ),
        importance=0.35,
        created_at_offset_secs=150 * DAY,
    ),
    LabeledMemory(
        label="old:legacy_scoring",
        content=(
            "The pre-scoring-v2 formula weighted similarity + recency + "
            "importance + ln(uses+1), with recency measured from "
            "accessed_at so recalls themselves bumped the recency score."
        ),
        importance=0.45,
        created_at_offset_secs=220 * DAY,
    ),
    LabeledMemory(
        label="old:why_encryption",
        content=(
            "An agent's memory is a valuable, private asset — a long list "
            "of past tool outputs, personal preferences, facts. Encrypting "
            "at rest makes copying and backup safe by default."
        ),
        importance=0.35,
        created_at_offset_secs=240 * DAY,
    ),
    LabeledMemory(
        label="old:argon2id_params",
        content=(
            "Default Argon2id params: 19 MiB memory cost, 2 iterations, "
            "1 lane. Takes ~50–100 ms to derive the KEK, which is the "
            "point: brute-forcing the passphrase stays expensive."
        ),
        importance=0.5,
        created_at_offset_secs=160 * DAY,
    ),
]

NOISE_MEMORIES: list[LabeledMemory] = [
    LabeledMemory(
        label=f"noise:{i}",
        content=content,
        importance=0.2,
        created_at_offset_secs=age,
    )
    for i, (content, age) in enumerate(
        [
            ("Weather note: it was raining when the doc was written.", 90 * DAY),
            ("Reminder: coffee tastes better when brewed at 94°C.", 120 * DAY),
            ("Random IP address: 203.0.113.17. Reserved for documentation.", 110 * DAY),
            ("Reminder that the author's cat is named Mozzarella.", 95 * DAY),
            ("Historical note: SQLite was first released in 2000.", 300 * DAY),
            ("Fun fact: Rust's mascot is a crab named Ferris.", 140 * DAY),
            ("Note: GitHub Actions runners typically run Ubuntu 22.04.", 20 * DAY),
            ("Note: MessagePack is a binary format; JSON is text.", 180 * DAY),
            ("Reminder: HTTP 418 means 'I'm a teapot'.", 75 * DAY),
            ("Note: ARM64 hardware AES is ~6x faster than the software fallback.", 25 * DAY),
        ],
        start=0,
    )
]


def collect_corpus() -> list[LabeledMemory]:
    out: list[LabeledMemory] = []
    for pair in SUPERSEDED_FACT_PAIRS:
        out.append(pair[0])
        out.append(pair[1])
    out.extend(FRESH_MEMORIES)
    out.extend(RECENT_MEMORIES)
    out.extend(MID_MEMORIES)
    out.extend(OLD_MEMORIES)
    out.extend(NOISE_MEMORIES)
    return out


# --- Queries ---------------------------------------------------------
#
# Categories follow the Phase 6 harness convention so the perf-JSON
# per_category block stays comparable: (release|pr|function|decision)_
# (semantic|exact). We add two scoring-v2-specific categories:
#   superseded_fact — paired query, only the :current member is correct.
#   recency_sensitive — query where the right answer is "the newest X".

SUPERSEDED_FACT_QUERIES: list[LabeledQuery] = [
    LabeledQuery(
        text=triple[2],
        category="superseded_fact",
        expected_labels=[triple[0].label],
    )
    for triple in SUPERSEDED_FACT_PAIRS
]

RECENCY_SENSITIVE_QUERIES: list[LabeledQuery] = [
    LabeledQuery(
        text="what's the most recent planner dispatch?",
        category="recency_sensitive",
        expected_labels=["fresh:planner_dispatch"],
    ),
    LabeledQuery(
        text="what did BP.1 ship?",
        category="recency_sensitive",
        expected_labels=["recent:bp1"],
    ),
    LabeledQuery(
        text="latest write-latency benchmark on M1",
        category="recency_sensitive",
        expected_labels=["recent:write_bench"],
    ),
]

SEMANTIC_QUERIES: list[LabeledQuery] = [
    LabeledQuery(
        text="how is the WAL committed?",
        category="architecture_semantic",
        expected_labels=["mid:wal_semantics"],
    ),
    LabeledQuery(
        text="how does the IVF+PQ index work?",
        category="architecture_semantic",
        expected_labels=["mid:ann_arch"],
    ),
    LabeledQuery(
        text="how is encryption layered?",
        category="architecture_semantic",
        expected_labels=["mid:crypto_layers"],
    ),
    LabeledQuery(
        text="how do sessions consolidate turns?",
        category="architecture_semantic",
        expected_labels=["mid:session_api"],
    ),
    LabeledQuery(
        text="what runs the MCP server?",
        category="architecture_semantic",
        expected_labels=["mid:mcp_server"],
    ),
]


def collect_queries() -> list[LabeledQuery]:
    out: list[LabeledQuery] = []
    out.extend(SUPERSEDED_FACT_QUERIES)
    out.extend(RECENCY_SENSITIVE_QUERIES)
    out.extend(SEMANTIC_QUERIES)
    return out


# ----------------------------------------------------------------------
# Fixture writers
# ----------------------------------------------------------------------


def _embed(model: SentenceTransformer, texts: list[str]) -> list[list[float]]:
    arr = model.encode(texts, normalize_embeddings=NORMALIZE)
    # Convert numpy → plain Python lists for JSON.
    return [list(map(float, row)) for row in arr]


def write_phase6_fixture(model: SentenceTransformer, out_path: Path) -> None:
    """Embeds the Phase 6 corpus + queries as-is for the sanity gate.

    Every memory gets ``created_at_offset_secs = 0`` and ``importance``
    from the Phase 6 CORPUS tuple, because the Phase 6 harness built
    the corpus all-at-once — the sanity gate's job is to reproduce
    *that* setup (and therefore the Phase 6 numbers) under V0.
    """
    contents = [row[1] for row in PHASE6_CORPUS]
    vectors = _embed(model, contents)
    query_texts = [q[0] for q in PHASE6_QUERIES]
    query_vectors = _embed(model, query_texts)

    fixture = {
        "meta": {
            "model": MODEL_NAME,
            "dimensions": DIMENSIONS,
            "normalize": NORMALIZE,
            "source": "test/scripts/ranking_harness.py Phase 6 CORPUS + QUERIES",
            "purpose": (
                "Scoring v2 sanity gate — Rust harness runs V0 on this "
                "fixture; results must match perf_v0.5.0.json['ranking'] "
                "within noise before the age-spread measurement runs."
            ),
        },
        "memories": [
            {
                "label": label,
                "content": content,
                "importance": importance,
                "created_at_offset_secs": 0,
                "agent_id": "assistant",
                "vector": vector,
            }
            for (label, content, importance), vector in zip(
                PHASE6_CORPUS, vectors, strict=True
            )
        ],
        "queries": [
            {
                "text": text,
                "category": category,
                "expected_labels": [expected_label],
                "vector": vector,
            }
            for (text, expected_label, category), vector in zip(
                PHASE6_QUERIES, query_vectors, strict=True
            )
        ],
    }

    out_path.write_text(json.dumps(fixture, indent=2) + "\n")
    print(
        f"[phase6] wrote {len(fixture['memories'])} memories + "
        f"{len(fixture['queries'])} queries to {out_path.name}"
    )


def write_v2_fixture(model: SentenceTransformer, out_path: Path) -> None:
    """Embeds the age-spread corpus + queries for the main v2 measurement."""
    corpus = collect_corpus()
    queries = collect_queries()

    vectors = _embed(model, [m.content for m in corpus])
    query_vectors = _embed(model, [q.text for q in queries])

    fixture = {
        "meta": {
            "model": MODEL_NAME,
            "dimensions": DIMENSIONS,
            "normalize": NORMALIZE,
            "generator": "test/scripts/gen_ranking_v2_fixture.py",
            "purpose": (
                "Age-spread corpus for scoring v2 measurement. Each memory "
                "carries created_at_offset_secs; the Rust harness backdates "
                "Memory.created_at / accessed_at by that amount before "
                "remember so the recency clock has something to decay. "
                "SUPERSEDED_FACT_PAIRS guarantee each 'what's the current X' "
                "query has a single unambiguous correct answer."
            ),
            "superseded_fact_pair_count": len(SUPERSEDED_FACT_PAIRS),
        },
        "memories": [
            {
                "label": m.label,
                "content": m.content,
                "importance": m.importance,
                "created_at_offset_secs": m.created_at_offset_secs,
                "agent_id": m.agent_id,
                "vector": vector,
            }
            for m, vector in zip(corpus, vectors, strict=True)
        ],
        "queries": [
            {
                "text": q.text,
                "category": q.category,
                "expected_labels": q.expected_labels,
                "vector": vector,
            }
            for q, vector in zip(queries, query_vectors, strict=True)
        ],
    }

    out_path.write_text(json.dumps(fixture, indent=2) + "\n")
    print(
        f"[v2    ] wrote {len(fixture['memories'])} memories + "
        f"{len(fixture['queries'])} queries to {out_path.name}"
    )


def main() -> None:
    print(f"Loading {MODEL_NAME} …")
    model = SentenceTransformer(MODEL_NAME)
    write_phase6_fixture(model, _THIS_DIR / "ranking_v2_phase6_fixture.json")
    write_v2_fixture(model, _THIS_DIR / "ranking_v2_fixture.json")
    print("done — commit both fixtures so the Rust harness is reproducible.")


if __name__ == "__main__":
    main()
