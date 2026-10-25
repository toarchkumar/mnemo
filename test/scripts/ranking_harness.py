"""Ranking-quality harness for mnemo recall (Phase 6 amendment, v0.5.0).

Why this exists
---------------
ANN recall@10 measures *index fidelity* — did we lose anything compared to
exact brute-force scan? It does **not** measure whether the *right memory*
is at rank 1 when an agent asks a question. The 2026-10-25 planner finding
caught a concrete regression (a new release note sat at rank 4 behind older
entries with weaker similarity but higher recency/frequency), and the fix —
Recall scoring v2 — needs a baseline to be judged against.

This script is the baseline. It builds a small synthetic corpus with known
right answers, including a handful of exact-identifier queries (versions,
PR numbers, function names) that embeddings notoriously handle poorly, and
scores three configurations:

- ``default_weights_track_true`` — the current default. Weighted combination
  of similarity, recency, importance and ln(uses+1); every recall mutates
  access_count and accessed_at on the memories it returns. Run as a 5-pass
  simulated-usage loop to catch the feedback drift the planner observed.
- ``similarity_only`` — α=1, β=γ=δ=0. Rebuilt in Python by re-ranking the
  raw ``search()`` output; no engine change required.
- ``default_weights_track_false`` — same formula as the first config, but
  with ``track_access=False`` so the feedback loop is disabled. Lets us
  attribute damage between "the formula" and "the feedback loop".

Each configuration reports hit@1, hit@5, MRR and (for the track-true loop)
drift of the top-1 answer between pass 1 and pass 5.

The output lands under a ``ranking`` key in ``test/scripts/perf_v0.5.0.json``
alongside the speed benchmarks. No engine changes.

Usage
-----
::

    python test/scripts/ranking_harness.py

Dependencies: ``mnemo`` (v0.5.0+) and ``sentence-transformers`` (plus torch),
which the Phase 3 dogfood workflow already installs.
"""

from __future__ import annotations

import json
import os
import platform
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import mnemo
from sentence_transformers import SentenceTransformer

# ----------------------------------------------------------------------
# Configuration
# ----------------------------------------------------------------------

MODEL_NAME = "sentence-transformers/all-MiniLM-L6-v2"
DIMENSIONS = 384
NORMALIZE = True

# How many times to replay the labelled query set under the current-default
# configuration. A single pass with track_access=true is enough to prime
# access_count; by pass 5 the loop should be fully in effect.
USAGE_LOOP_PASSES = 5

# Default weights as recorded in mnemo/src/memory.rs::ScoreWeights::default.
DEFAULT_WEIGHTS = {
    "alpha": 1.0,
    "beta": 0.3,
    "gamma": 0.2,
    "delta": 0.1,
    "half_life_days": 7.0,
}

# Where to write the results. Loads any existing file so the ranking block
# can slot in next to speed-benchmark blocks the Rust side produces.
PERF_PATH = Path(__file__).parent / "perf_v0.5.0.json"


# ----------------------------------------------------------------------
# Corpus
# ----------------------------------------------------------------------
# Each entry: (label, content, importance). `label` is a stable key the
# query set references; it never leaves the harness. The ordering is fixed
# so the Ulids assigned by remember_text are deterministic across runs
# on the same machine (useful when comparing two harness runs).


CORPUS: list[tuple[str, str, float]] = [
    # Release notes — one per shipped version, phrased the way a changelog
    # or git tag message would read. Each contains the version identifier
    # verbatim so exact-identifier queries at least have a chance.
    ("release:v0.1.0",
     "v0.1.0 release notes. Initial public release of mnemo, the encrypted "
     "single-file agent memory engine. Includes the four memory types, "
     "multi-signal recall, Python bindings, CLI binary with init/info/import/"
     "index/search/verify/rekey/compact/snapshots/restore/demo subcommands.",
     0.9),
    ("release:v0.2.0",
     "v0.2.0 release notes. Added track_access on Memory for recency/"
     "frequency recall signals. Python package renamed to mnemo. "
     "Catalog cap at 1M memories to keep manifest decode bounded.",
     0.7),
    ("release:v0.3.0",
     "v0.3.0 release notes. File locking (mandatory on Windows, advisory "
     "elsewhere) and read-only opens. One writer at a time, many readers "
     "concurrently. Mnemo::open_read_only is the new API.",
     0.75),
    ("release:v0.3.1",
     "v0.3.1 release notes. Interactive passphrase prompts in the CLI; "
     "query-file input on recall; manifest cap and security-model notes.",
     0.5),
    ("release:v0.3.2",
     "v0.3.2 release notes. Documentation pass only — no engine changes. "
     "Clarified the security model, passphrase handling and backup story.",
     0.5),
    ("release:v0.4.0",
     "v0.4.0 release notes. Format v8 bump: catalog-side access stats + "
     "result cache. MCP server (mnemo serve --mcp) with the first set of "
     "JSON-RPC tools. Phase 10.1 / 10.3 / 10.4 shipped.",
     0.85),
    ("release:v0.4.1",
     "v0.4.1 release notes. Python binding parity — set_cache_flush_policy, "
     "set_cache_budget, rekey, compact_file, memories, dimensions all "
     "exposed. PyPI Trusted Publishing wired up in the release workflow.",
     0.75),
    ("release:v0.5.0",
     "v0.5.0 release notes. Phase 3 bring-your-own-embedder complete: "
     "Embedder trait, remember_text, EmbedCmd persistent-child adapter, "
     "CLI remember/recall --text, MCP --embed-cmd. All-zero vectors are "
     "now refused (breaking signal).",
     0.95),

    # PR descriptions — one per notable PR, each with its PR number
    # verbatim in the body.
    ("pr:PR-3",
     "PR 3 shipped format v8 and the exact-key result_cache module. "
     "Every mutation flushes before returning so a cache hit is as durable "
     "as a direct catalog read.",
     0.6),
    ("pr:PR-4",
     "PR 4 added the Phase 10.2 semantic cache — vector-keyed cache with "
     "cosine similarity threshold. Model name defaults to the embedder's "
     "identity when text is used.",
     0.55),
    ("pr:PR-5",
     "PR 5 added the Phase 10.4 live surfaces: MCP tool wiring, Python "
     "binding exposure, and a decorator on the Python Mnemo class for "
     "transparent caching.",
     0.5),
    ("pr:PR-7",
     "PR 7 added the fuzzing scaffold: cargo-fuzz crate under mnemo/fuzz, "
     "five fuzz_targets, scheduled weekly runs, panic hardening in format/"
     "wal/pager.",
     0.65),
    ("pr:PR-8",
     "PR 8 added the proptest state machine with a crash-injection hook "
     "under the __failpoints feature. Shipped in four follow-ups (8/8.1/"
     "8.2/8.3).",
     0.65),
    ("pr:PR-9",
     "PR 9 added cargo-deny gates (advisories/bans/licenses/sources), "
     "a cargo fmt --check CI job, and switched maturin publishing to "
     "PyPI Trusted Publishing.",
     0.55),
    ("pr:PR-BP",
     "PR BP (binding parity) exposed set_cache_flush_policy, "
     "set_cache_budget, rekey, compact_file, memories, dimensions on the "
     "Python Mnemo class — one method per missing Rust API.",
     0.6),
    ("pr:PR-10a",
     "PR 10a introduced the Embedder trait and EmbedderIdentity, Mnemo::"
     "remember_text, Memory::from_text, the manifest identity check on "
     "set_embedder, and all-zero vector rejection. Phase 3 part 1.",
     0.8),
    ("pr:PR-10b",
     "PR 10b built the EmbedCmd persistent-child adapter and wired "
     "--embed-cmd/--embed-name into CLI remember/recall and MCP serve. "
     "Phase 3 part 2.",
     0.8),
    ("pr:PR-10b.2",
     "PR 10b.2 fixed EmbedCmd correctness. Reader thread + mpsc channel "
     "so recv_timeout actually fires; kill child on any protocol error "
     "and respawn on the next call; --embed-timeout flag.",
     0.85),

    # Function / API references — exact-identifier queries live here.
    ("api:Mnemo::open",
     "Mnemo::open(path, passphrase) opens an existing encrypted memory "
     "file for read-write access. The file lock is exclusive; "
     "open_read_only is the shared-reader alternative.",
     0.55),
    ("api:Mnemo::recall",
     "fn recall(req: &RecallRequest) -> Result<Vec<RecallResult>>. "
     "Multi-signal recall: top_k by α·similarity + β·recency + "
     "γ·importance + δ·ln(uses+1).",
     0.7),
    ("api:Mnemo::remember",
     "fn remember(m: Memory) -> Result<Ulid>. Writes one memory into the "
     "store; returns its Ulid. All-zero vectors are refused since v0.5.0.",
     0.65),
    ("api:Mnemo::flush",
     "fn flush(&mut self) -> Result<()>. One atomic fsync transaction "
     "across the WAL. Crash-recovery replays the WAL on the next open.",
     0.6),
    ("api:EmbedCmd",
     "EmbedCmd — persistent-child Embedder adapter. Spawns the user's "
     "embedder command once per Mnemo handle, speaks line-delimited JSON, "
     "and respawns on any protocol error.",
     0.65),
    ("api:Session",
     "Session — conversation wrapper that consolidates its turns into "
     "episodic memory on close. One agent_id per session; recall is "
     "automatically scoped to that agent.",
     0.5),
    ("api:ScoreWeights",
     "ScoreWeights {alpha, beta, gamma, delta, half_life_secs} — the "
     "four coefficients and the half-life that combine the recall "
     "signals into a single score. Defaults tuned for conversational "
     "agents.",
     0.6),
    ("api:Metric",
     "Metric — recall distance function. Cosine (the default, used by "
     "the result cache's semantic variant too) and Dot are supported.",
     0.45),

    # Decision notes — architecture decisions worth remembering.
    ("decision:ivf-vs-hnsw",
     "Decision: IVF+PQ (not HNSW) for the Phase 2 approximate index. "
     "IVF is linear in extra space and reconstructs cleanly from the "
     "catalog; HNSW's hierarchical links would need their own durability "
     "story.",
     0.65),
    ("decision:format-v8",
     "Decision: format v8 moves access-stats catalog-side so flushes can "
     "be lock-free on the hot path. Readers see the same stats they'd "
     "see from a catalog scan.",
     0.6),
    ("decision:all-zero-reject",
     "Decision: refuse all-zero embedding vectors at remember time. They "
     "score exactly 0 on every cosine query, so they can't contribute "
     "signal — but recency and importance can still pull them into top-k "
     "as noise.",
     0.7),
    ("decision:phase2-moved-up",
     "Decision: Phase 2 (hybrid BM25+vector recall) moved up in the "
     "roadmap, right after Phase 6 and Recall scoring v2. User priority: "
     "correct answers before faster answers.",
     0.75),
    ("decision:no-shell-embed-cmd",
     "Decision: --embed-cmd is parsed with a Rust shlex_split; no shell "
     "is invoked. No $VAR or ~ expansion. Keeps the shell-injection "
     "surface at zero.",
     0.5),
    ("decision:stdout-replies-only",
     "Decision: an embed-cmd child's stdout is reserved for replies "
     "only. Any other line (banner, log, warning) is a protocol "
     "violation and triggers a kill-and-respawn. Log to stderr.",
     0.65),
    ("decision:windows-stack",
     "Decision: run the CLI dispatch on an 8 MiB worker thread. "
     "Windows' default main-thread stack is 1 MiB and the ~600-line "
     "match in run() can overflow it in debug builds.",
     0.45),
    ("decision:pypi-trusted",
     "Decision: PyPI Trusted Publishing (OIDC from GitHub Actions) "
     "instead of long-lived PyPI tokens. One fewer secret in the repo "
     "settings.",
     0.5),
    ("decision:format-v9-combine",
     "Decision: fold the Phase 11.2 hot-field inline optimization into "
     "the same v9 format bump that Phase 2 will need, so users pay the "
     "migration cost once.",
     0.5),

    # Noise — plausible but off-topic content so the easy queries have
    # distractors with real similarity. Padded enough that top-5 isn't
    # trivially the full relevant set.
    ("noise:1",
     "Project onboarding checklist: clone the repo, install Rust 1.75+, "
     "run cargo test, then run the dogfood script test/scripts/"
     "project_memory.py --about to see every onboarding note.",
     0.4),
    ("noise:2",
     "The MCP server reads MNEMO_PASSPHRASE from the environment; a "
     "--passphrase CLI arg would land in process listings and shell "
     "history, which is why it's not supported.",
     0.4),
    ("noise:3",
     "Agent memory should be local-first: one encrypted file per agent, "
     "copyable and backupable without a database. That is the whole "
     "point of the single-file format.",
     0.35),
    ("noise:4",
     "The CLI mnemo info prints file size, encrypted bool, WAL region "
     "size, created_at, agents, snapshot count, cache summary, and ANN "
     "index info.",
     0.35),
    ("noise:5",
     "mnemo compact collapses soft-deleted memories and old snapshot "
     "pages into a fresh catalog. Call it periodically on long-lived "
     "files to recover disk space.",
     0.4),
    ("noise:6",
     "Snapshots are point-in-time recovery markers; every flush creates "
     "one, mnemo snapshots lists them, mnemo restore <id> rolls back "
     "to one.",
     0.4),
    ("noise:7",
     "The write-ahead log region is fixed at 64 pages (512 KiB) today. "
     "At small N it dominates file size — the baseline perf file "
     "reported 62% WAL share on a 28-memory dogfood file.",
     0.35),
    ("noise:8",
     "KDF defaults to Argon2id with memory=96MB and iterations=3 at "
     "open time. Faster for tests, production KDF for real files; "
     "configurable via KdfParams::fast / ::secure.",
     0.4),
    ("noise:9",
     "The Phase 1.3 proptest state machine runs on every PR and under "
     "MSRV. Crash injection is behind the __failpoints feature so the "
     "non-test build doesn't carry the hook.",
     0.4),
    ("noise:10",
     "Python's with mnemo.open(...) context manager calls close() on "
     "scope exit. On read-only handles this now short-circuits through "
     "the mode-aware Rust close() instead of calling flush() (which "
     "would raise ReadOnly).",
     0.4),
    ("noise:11",
     "The deny.toml policy lists allowed licenses (Apache-2.0, MIT, "
     "BSD-3-Clause, ISC, Unicode-DFS-2016) and bans unmaintained or "
     "security-advisory-flagged crates.",
     0.35),
    ("noise:12",
     "Node bindings via napi-rs are on the roadmap (Phase 6 of the "
     "original plan). Not shipped yet; the Rust core they'd wrap is "
     "complete.",
     0.35),
    ("noise:13",
     "The AGENTS.md workflow calls db.about() at the start of every "
     "session, then recalls the three most relevant onboarding memories "
     "before touching code.",
     0.35),
    ("noise:14",
     "Test corpus memories are generated by test/scripts/seed.json, "
     "a 31-entry list the dogfood file is initialized from. The first "
     "entry is the manifest.",
     0.35),
    ("noise:15",
     "Phase 5 added the Session type. A session owns one agent_id and "
     "automatically scopes recall and remember to that agent.",
     0.4),
    ("noise:16",
     "The result cache has a semantic variant: cache_put_semantic takes "
     "a vector or text, cache_get_semantic returns the nearest hit above "
     "a cosine threshold. Model name tracked separately.",
     0.4),
    ("noise:17",
     "The embedder for the dogfood file is sentence-transformers/all-"
     "MiniLM-L6-v2 (384 dimensions, normalized output). Recorded on the "
     "scaffold manifest.",
     0.5),
    ("noise:18",
     "File lock contention surfaces as MnemoError::Locked. The Python "
     "binding converts it to RuntimeError('file already locked ...'); "
     "the Rust binary exits non-zero with that message on stderr.",
     0.35),
    ("noise:19",
     "The MCP server deliberately runs recall with track_access=false so "
     "an exploratory 'what do you know about X' call doesn't bump every "
     "memory's access_count.",
     0.5),
    ("noise:20",
     "WAL replay on open re-applies any un-fsynced pages from the last "
     "transaction. Idempotent, bounded by WAL region size, and runs "
     "before any catalog read.",
     0.4),
]


# ----------------------------------------------------------------------
# Labelled queries
# ----------------------------------------------------------------------
# Each row is (query_text, expected_label, category). The harness resolves
# expected_label → expected_id after the corpus is inserted.
#
# Categories are kept simple: `*_semantic` queries use natural language, so
# good embeddings should handle them; `*_exact` queries are the version /
# PR / function-name identifiers that embeddings notoriously botch.

QUERIES: list[tuple[str, str, str]] = [
    # Release — semantic
    ("what shipped in v0.5.0?", "release:v0.5.0", "release_semantic"),
    ("what's new in the latest release?", "release:v0.5.0", "release_semantic"),
    ("the first public release", "release:v0.1.0", "release_semantic"),
    ("MCP server added in which version?", "release:v0.4.0", "release_semantic"),
    ("file locking release", "release:v0.3.0", "release_semantic"),
    # Release — exact identifier
    ("v0.5.0", "release:v0.5.0", "release_exact"),
    ("v0.4.0", "release:v0.4.0", "release_exact"),
    ("v0.3.0", "release:v0.3.0", "release_exact"),
    ("v0.1.0", "release:v0.1.0", "release_exact"),
    ("v0.3.2", "release:v0.3.2", "release_exact"),

    # PR — semantic
    ("which PR added fuzzing?", "pr:PR-7", "pr_semantic"),
    ("the proptest state machine PR", "pr:PR-8", "pr_semantic"),
    ("cargo-deny was added by which PR?", "pr:PR-9", "pr_semantic"),
    ("PR that fixed EmbedCmd timeouts", "pr:PR-10b.2", "pr_semantic"),
    ("Python binding parity PR", "pr:PR-BP", "pr_semantic"),
    # PR — exact identifier
    ("PR 7", "pr:PR-7", "pr_exact"),
    ("PR 8", "pr:PR-8", "pr_exact"),
    ("PR 10a", "pr:PR-10a", "pr_exact"),
    ("PR 10b.2", "pr:PR-10b.2", "pr_exact"),
    ("PR BP", "pr:PR-BP", "pr_exact"),

    # API / functions — semantic
    ("how do I open a database?", "api:Mnemo::open", "function_semantic"),
    ("flushing changes to disk", "api:Mnemo::flush", "function_semantic"),
    ("conversation lifecycle wrapper", "api:Session", "function_semantic"),
    ("the recall scoring coefficients", "api:ScoreWeights", "function_semantic"),
    # API / functions — exact identifier
    ("fn recall", "api:Mnemo::recall", "function_exact"),
    ("fn flush", "api:Mnemo::flush", "function_exact"),
    ("EmbedCmd", "api:EmbedCmd", "function_exact"),
    ("ScoreWeights", "api:ScoreWeights", "function_exact"),

    # Decision — mixed
    ("why IVF+PQ and not HNSW?", "decision:ivf-vs-hnsw", "decision_semantic"),
    ("all-zero vector", "decision:all-zero-reject", "decision_exact"),
]


# ----------------------------------------------------------------------
# Helpers
# ----------------------------------------------------------------------

@dataclass
class QueryResult:
    """One query's outcome under one configuration."""

    query: str
    category: str
    expected_id: str
    expected_label: str
    top1_id: str | None
    top1_label: str | None
    top1_similarity: float | None
    rank: int | None  # 1-based; None means not in top_k
    top_k_labels: list[str] = field(default_factory=list)


def _machine() -> dict[str, Any]:
    """Snapshot of the host — written next to the ranking numbers so a
    later reader can tell which laptop they were produced on. Rust and
    Python versions are picked up when available; errors become empty
    strings rather than aborting the harness."""
    cpu = ""
    try:
        if platform.system() == "Darwin":
            cpu = subprocess.check_output(
                ["sysctl", "-n", "machdep.cpu.brand_string"],
                text=True,
                stderr=subprocess.DEVNULL,
            ).strip()
    except Exception:
        pass
    rustc = ""
    try:
        rustc = subprocess.check_output(
            ["rustc", "--version"], text=True, stderr=subprocess.DEVNULL
        ).strip()
    except Exception:
        pass
    return {
        "cpu": cpu or platform.processor() or platform.machine(),
        "os": f"{platform.system()} {platform.release()}",
        "python": platform.python_version(),
        "rustc": rustc,
        "mnemo": getattr(mnemo, "__version__", "unknown"),
    }


def _ensure_corpus(db: mnemo.Mnemo, model: SentenceTransformer) -> dict[str, str]:
    """Insert the full corpus and return label → ulid-string mapping.

    Importance is written through the Memory metadata; mnemo-python's
    remember_text takes an ``importance`` kwarg, so this is one call per
    entry rather than a build-and-remember pair.
    """
    label_to_id: dict[str, str] = {}
    for label, content, importance in CORPUS:
        mid = db.remember_text(
            content,
            memory_type="semantic",
            importance=importance,
            metadata={"label": label},
        )
        label_to_id[label] = mid
    db.flush()
    return label_to_id


def _score(expected_id: str, hits: list[dict], k_top_at: list[int]) -> dict[str, Any]:
    """Compute rank / hit@k / RR for one query result list.

    ``hits`` is already in rank order. If the expected id is not present
    at all, rank is reported as None and RR as 0."""
    rank = None
    for i, h in enumerate(hits, start=1):
        if h["id"] == expected_id:
            rank = i
            break
    out: dict[str, Any] = {"rank": rank, "rr": 0.0 if rank is None else 1.0 / rank}
    for k in k_top_at:
        out[f"hit_at_{k}"] = bool(rank is not None and rank <= k)
    return out


def _summarize(per_query: list[QueryResult], scores: list[dict[str, Any]]) -> dict[str, Any]:
    """Aggregate the per-query numbers into hit@1, hit@5, MRR, and keep
    the misses (rank > 1 or missing entirely) around so the DONE write-up
    can quote specific regressions rather than averages."""
    n = len(scores)
    hit1 = sum(1 for s in scores if s["hit_at_1"]) / n if n else 0.0
    hit5 = sum(1 for s in scores if s["hit_at_5"]) / n if n else 0.0
    mrr = sum(s["rr"] for s in scores) / n if n else 0.0
    misses = []
    for q, s in zip(per_query, scores):
        if s["rank"] != 1:
            misses.append({
                "query": q.query,
                "category": q.category,
                "expected_label": q.expected_label,
                "top1_label": q.top1_label,
                "top1_similarity": q.top1_similarity,
                "rank_of_expected": s["rank"],
                "top_k_labels": q.top_k_labels,
            })
    return {
        "hit_at_1": round(hit1, 4),
        "hit_at_5": round(hit5, 4),
        "mrr": round(mrr, 4),
        "misses": misses,
    }


# ----------------------------------------------------------------------
# Runners for each configuration
# ----------------------------------------------------------------------

def _config_default_track_true(
    db: mnemo.Mnemo,
    resolved_queries: list[tuple[str, str, str, str]],
    label_by_id: dict[str, str],
) -> dict[str, Any]:
    """Config A — the current default. Runs the whole query set
    USAGE_LOOP_PASSES times with track_access=True so the feedback loop
    actually fires. Pass 1 and pass USAGE_LOOP_PASSES both get full
    hit@k / MRR; the drift block names queries whose top-1 answer
    changed between the two."""
    passes_out: list[dict[str, Any]] = []
    first_pass_top1: dict[str, str | None] = {}
    last_pass_top1: dict[str, str | None] = {}

    for pass_idx in range(1, USAGE_LOOP_PASSES + 1):
        per_query: list[QueryResult] = []
        scores: list[dict[str, Any]] = []
        for query, expected_id, expected_label, category in resolved_queries:
            hits = db.recall_text(query, top_k=10, track_access=True)
            top_k_labels = [label_by_id.get(h["id"], "?") for h in hits]
            top1 = hits[0] if hits else None
            qr = QueryResult(
                query=query,
                category=category,
                expected_id=expected_id,
                expected_label=expected_label,
                top1_id=top1["id"] if top1 else None,
                top1_label=label_by_id.get(top1["id"]) if top1 else None,
                top1_similarity=top1.get("similarity") if top1 else None,
                rank=None,
                top_k_labels=top_k_labels,
            )
            s = _score(expected_id, hits, k_top_at=[1, 5])
            qr.rank = s["rank"]
            per_query.append(qr)
            scores.append(s)
            if pass_idx == 1:
                first_pass_top1[query] = qr.top1_label
            if pass_idx == USAGE_LOOP_PASSES:
                last_pass_top1[query] = qr.top1_label
        summary = _summarize(per_query, scores)
        summary["pass"] = pass_idx
        passes_out.append(summary)

    drift_queries = [
        {
            "query": q,
            "pass1_top1": first_pass_top1[q],
            "last_pass_top1": last_pass_top1[q],
        }
        for q in first_pass_top1
        if first_pass_top1[q] != last_pass_top1.get(q)
    ]

    return {
        "formula": "alpha*sim + beta*recency + gamma*importance + delta*ln(uses+1)",
        "weights": DEFAULT_WEIGHTS,
        "track_access": True,
        "usage_loop_passes": USAGE_LOOP_PASSES,
        "passes": passes_out,
        "drift_pass1_vs_last": {
            "top1_changed_count": len(drift_queries),
            "top1_changed": drift_queries,
        },
    }


def _config_similarity_only(
    db: mnemo.Mnemo,
    model: SentenceTransformer,
    resolved_queries: list[tuple[str, str, str, str]],
    label_by_id: dict[str, str],
) -> dict[str, Any]:
    """Config B — α=1, β=γ=δ=0. The Python binding doesn't expose custom
    ScoreWeights, and adding that would be an engine change, so instead
    we embed each query ourselves and use ``search()`` — which is pure
    cosine, no scoring formula — as the ranking source. search() does not
    mutate access state."""
    per_query: list[QueryResult] = []
    scores: list[dict[str, Any]] = []
    for query, expected_id, expected_label, category in resolved_queries:
        qvec = model.encode(
            [query], normalize_embeddings=NORMALIZE
        )[0].astype("float32").tolist()
        pairs = db.search(qvec, top_k=10)  # [(memory_dict, similarity)]
        hits = []
        for m, sim in pairs:
            h = dict(m)
            h["similarity"] = sim
            hits.append(h)
        top_k_labels = [label_by_id.get(h["id"], "?") for h in hits]
        top1 = hits[0] if hits else None
        qr = QueryResult(
            query=query,
            category=category,
            expected_id=expected_id,
            expected_label=expected_label,
            top1_id=top1["id"] if top1 else None,
            top1_label=label_by_id.get(top1["id"]) if top1 else None,
            top1_similarity=top1["similarity"] if top1 else None,
            rank=None,
            top_k_labels=top_k_labels,
        )
        s = _score(expected_id, hits, k_top_at=[1, 5])
        qr.rank = s["rank"]
        per_query.append(qr)
        scores.append(s)

    summary = _summarize(per_query, scores)
    summary["formula"] = "sim"
    summary["weights"] = {"alpha": 1.0, "beta": 0.0, "gamma": 0.0, "delta": 0.0}
    summary["track_access"] = False
    return summary


def _config_default_track_false(
    db: mnemo.Mnemo,
    resolved_queries: list[tuple[str, str, str, str]],
    label_by_id: dict[str, str],
) -> dict[str, Any]:
    """Config C — current formula, no feedback loop. One pass only;
    without track_access there's nothing to iterate on."""
    per_query: list[QueryResult] = []
    scores: list[dict[str, Any]] = []
    for query, expected_id, expected_label, category in resolved_queries:
        hits = db.recall_text(query, top_k=10, track_access=False)
        top_k_labels = [label_by_id.get(h["id"], "?") for h in hits]
        top1 = hits[0] if hits else None
        qr = QueryResult(
            query=query,
            category=category,
            expected_id=expected_id,
            expected_label=expected_label,
            top1_id=top1["id"] if top1 else None,
            top1_label=label_by_id.get(top1["id"]) if top1 else None,
            top1_similarity=top1.get("similarity") if top1 else None,
            rank=None,
            top_k_labels=top_k_labels,
        )
        s = _score(expected_id, hits, k_top_at=[1, 5])
        qr.rank = s["rank"]
        per_query.append(qr)
        scores.append(s)

    summary = _summarize(per_query, scores)
    summary["formula"] = "alpha*sim + beta*recency + gamma*importance + delta*ln(uses+1)"
    summary["weights"] = DEFAULT_WEIGHTS
    summary["track_access"] = False
    return summary


# ----------------------------------------------------------------------
# Orchestration
# ----------------------------------------------------------------------

def _per_category_breakdown(summary: dict[str, Any], resolved_queries: list[tuple[str, str, str, str]]) -> dict[str, Any]:
    """A single hit@1/hit@5/MRR per category, computed from `misses` plus
    the known total. Added because the overall average hides the signal
    we care about most: embeddings bomb on exact-identifier queries."""
    per_cat_total: dict[str, int] = {}
    for _query, _expected_id, _expected_label, category in resolved_queries:
        per_cat_total[category] = per_cat_total.get(category, 0) + 1
    per_cat_misses: dict[str, list[dict[str, Any]]] = {k: [] for k in per_cat_total}
    for m in summary["misses"]:
        per_cat_misses[m["category"]].append(m)
    out: dict[str, Any] = {}
    for cat, total in per_cat_total.items():
        misses = per_cat_misses[cat]
        hits_at_1 = total - sum(1 for m in misses if m["rank_of_expected"] != 1)
        hits_at_5 = total - sum(1 for m in misses if m["rank_of_expected"] is None or m["rank_of_expected"] > 5)
        rr_sum = 0.0
        for m in misses:
            r = m["rank_of_expected"]
            if r is not None and r > 1:
                rr_sum += 1.0 / r
        rr_sum += (total - len(misses))  # perfect top-1s contribute 1.0 each
        out[cat] = {
            "n": total,
            "hit_at_1": round(hits_at_1 / total, 4) if total else 0.0,
            "hit_at_5": round(hits_at_5 / total, 4) if total else 0.0,
            "mrr": round(rr_sum / total, 4) if total else 0.0,
        }
    return out


def main() -> int:
    print(f"[ranking-harness] model: {MODEL_NAME} ({DIMENSIONS}-dim)", flush=True)
    model = SentenceTransformer(MODEL_NAME)

    passphrase = os.environ.get("MNEMO_PASSPHRASE", "ranking-harness-pw")
    with tempfile.TemporaryDirectory() as td:
        db_path = Path(td) / "ranking.mnemo"
        print(f"[ranking-harness] creating {db_path}", flush=True)
        # mnemo.open creates if the file is absent (dimensions required
        # on create, ignored on open) — no separate create() entry point.
        db = mnemo.open(str(db_path), passphrase, dimensions=DIMENSIONS)
        # set_embedder is one call: the callable, then the identity
        # kwargs. The identity is persisted on the scaffold manifest
        # and validated against it on reopen. The callback receives a
        # list of strings per batch and must return list[list[float]]
        # — matches the sentence-transformers / OpenAI shape.
        def _embed(texts: list[str]) -> list[list[float]]:
            vectors = model.encode(list(texts), normalize_embeddings=NORMALIZE)
            return vectors.astype("float32").tolist()
        db.set_embedder(
            _embed, name=MODEL_NAME, dimensions=DIMENSIONS, normalize=NORMALIZE
        )

        print(f"[ranking-harness] seeding {len(CORPUS)} memories", flush=True)
        label_to_id = _ensure_corpus(db, model)
        id_to_label = {v: k for k, v in label_to_id.items()}

        resolved_queries: list[tuple[str, str, str, str]] = []
        for q, label, cat in QUERIES:
            mid = label_to_id.get(label)
            if mid is None:
                raise RuntimeError(f"query {q!r} references unknown label {label!r}")
            resolved_queries.append((q, mid, label, cat))

        print(
            f"[ranking-harness] running {len(resolved_queries)} queries x "
            f"3 configurations (A loops {USAGE_LOOP_PASSES}x)",
            flush=True,
        )

        # Order matters: run similarity_only and track_false BEFORE
        # track_true, so the latter's access-state mutations don't bleed
        # into the earlier configs' numbers.
        sim_only = _config_similarity_only(db, model, resolved_queries, id_to_label)
        default_track_false = _config_default_track_false(db, resolved_queries, id_to_label)
        default_track_true = _config_default_track_true(db, resolved_queries, id_to_label)

        # Per-category breakdowns, pinned to pass 1 for the loop config
        # so it's comparable with the one-shot configs.
        sim_only["per_category"] = _per_category_breakdown(sim_only, resolved_queries)
        default_track_false["per_category"] = _per_category_breakdown(
            default_track_false, resolved_queries
        )
        for p in default_track_true["passes"]:
            # Reuse the same breakdown logic — each pass has the same
            # per-query miss shape.
            p["per_category"] = _per_category_breakdown(p, resolved_queries)

        ranking_block = {
            "machine": _machine(),
            "model": MODEL_NAME,
            "dimensions": DIMENSIONS,
            "normalize": NORMALIZE,
            "corpus_size": len(CORPUS),
            "queries": [
                {
                    "text": q,
                    "expected_id": mid,
                    "expected_label": label,
                    "category": cat,
                }
                for q, mid, label, cat in resolved_queries
            ],
            "configurations": {
                "default_weights_track_true": default_track_true,
                "similarity_only": sim_only,
                "default_weights_track_false": default_track_false,
            },
        }

        db.close()

    # Merge into perf_v0.5.0.json. The speed benchmarks live under their
    # own keys at the same top level; the file is a list in v0.1.0 but
    # the Phase 6 plan upgrades it to a dict so multiple metric families
    # share one file. We handle both shapes.
    out: dict[str, Any]
    if PERF_PATH.exists():
        existing = json.loads(PERF_PATH.read_text())
        if isinstance(existing, list):
            # Legacy array format from perf_v0.1.0.json. Wrap it.
            out = {"legacy_rows": existing}
        elif isinstance(existing, dict):
            out = existing
        else:
            out = {}
    else:
        out = {}
    out["ranking"] = ranking_block
    PERF_PATH.write_text(json.dumps(out, indent=2) + "\n")

    # Pretty-print a one-screen summary so a human reading the terminal
    # output doesn't need to open the JSON.
    def _one_line(name: str, block: dict[str, Any]) -> str:
        return (
            f"  {name:34s} hit@1={block['hit_at_1']:.3f}  "
            f"hit@5={block['hit_at_5']:.3f}  MRR={block['mrr']:.3f}"
        )

    print("\n[ranking-harness] results", flush=True)
    print(_one_line("similarity_only",
                    ranking_block["configurations"]["similarity_only"]))
    print(_one_line("default_weights_track_false",
                    ranking_block["configurations"]["default_weights_track_false"]))
    pass1 = ranking_block["configurations"]["default_weights_track_true"]["passes"][0]
    passN = ranking_block["configurations"]["default_weights_track_true"]["passes"][-1]
    print(_one_line(f"default_weights_track_true p1",  pass1))
    print(_one_line(f"default_weights_track_true p{USAGE_LOOP_PASSES}", passN))
    drift = ranking_block["configurations"]["default_weights_track_true"]["drift_pass1_vs_last"]
    print(f"  drift p1 -> p{USAGE_LOOP_PASSES}: {drift['top1_changed_count']} / "
          f"{len(resolved_queries)} top-1 answers changed")
    print(f"[ranking-harness] wrote {PERF_PATH}", flush=True)

    return 0


if __name__ == "__main__":
    sys.exit(main())
