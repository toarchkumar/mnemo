"""Phase 6 bench-results collector.

Walks `mnemo/target/criterion/**/estimates.json`, extracts the median
and mean timing of every bench criterion ran, and merges the result
into `test/scripts/perf_v0.5.0.json` under a top-level `speed` key
(or whichever key is passed via ``--key``).

The ``--key`` flag lets a given set of criterion runs land under a
non-default top-level block — PR 11.0 uses it to park the hardware-AES
numbers under ``speed_hw_aes`` next to the software-AES ``speed``
block from v0.5.0, so the before/after is reproducible from one file.

Shape of a criterion `estimates.json`:

    {
      "mean":   { "point_estimate": <ns>, "standard_error": <ns>, ... },
      "median": { "point_estimate": <ns>, ... },
      ...
    }

We keep the median (robust) and mean (comparable to criterion's
terminal output), convert nanoseconds to seconds, and record the
`throughput` block when criterion wrote one (needed to express QPS
on the `recall` and `cache_get_*` benches).

Idempotent. Rerun after each `cargo bench` invocation to refresh
the speed block without touching the ranking / ann_recall_grid /
file_size_breakdown blocks the other harnesses write.

Usage::

    python test/scripts/collect_bench_results.py            # merges into 'speed'
    python test/scripts/collect_bench_results.py --key speed_hw_aes
"""

from __future__ import annotations

import json
import platform
import subprocess
from pathlib import Path
from typing import Any

PERF_PATH = Path(__file__).parent / "perf_v0.5.0.json"
CRITERION_ROOT = Path(__file__).parent.parent.parent / "mnemo" / "target" / "criterion"


def _machine() -> dict[str, Any]:
    """Best-effort host snapshot. Mirrors ranking_harness.py so the
    two speed/quality blocks can be checked for 'same laptop'."""
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
        "rustc": rustc,
    }


def _collect_one(estimates_path: Path) -> dict[str, Any]:
    """Pull median/mean and (if present) throughput out of one
    `base/estimates.json`. Returns a plain dict with the fields we
    care about; the raw criterion file has more we deliberately drop
    (bootstrap distributions, confidence intervals) to keep the perf
    file reviewable in a diff."""
    with open(estimates_path) as f:
        est = json.load(f)
    out: dict[str, Any] = {
        "median_secs": est["median"]["point_estimate"] / 1e9,
        "mean_secs": est["mean"]["point_estimate"] / 1e9,
        "stderr_secs": est["mean"].get("standard_error", 0.0) / 1e9,
    }
    # Throughput, if criterion wrote one — only the `recall` /
    # `cache_get_*` / `write` benches set it today.
    tp_path = estimates_path.parent.parent / "benchmark.json"
    if tp_path.exists():
        try:
            with open(tp_path) as f:
                meta = json.load(f)
            if "throughput" in meta and meta["throughput"]:
                # criterion stores { "Elements": N } or { "Bytes": N }
                for k, v in meta["throughput"].items():
                    out[f"throughput_{k.lower()}_per_iter"] = v
        except Exception:
            pass
    return out


def main() -> int:
    import argparse
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument(
        "--key",
        default="speed",
        help="Top-level key in perf_v0.5.0.json to write under "
             "(default: 'speed'). PR 11.0 uses 'speed_hw_aes' for the "
             "hardware-AES rerun.",
    )
    args = ap.parse_args()
    key = args.key

    if not CRITERION_ROOT.exists():
        print(f"[collect] no criterion dir at {CRITERION_ROOT}. "
              "Run `cargo bench --features benches` first.")
        return 1

    speed: dict[str, dict[str, Any]] = {}
    # Criterion layout: target/criterion/<group>/<bench>/base/estimates.json
    for group_dir in sorted(CRITERION_ROOT.iterdir()):
        if not group_dir.is_dir() or group_dir.name == "report":
            continue
        group = group_dir.name
        for bench_dir in sorted(group_dir.iterdir()):
            if not bench_dir.is_dir() or bench_dir.name == "report":
                continue
            est_path = bench_dir / "base" / "estimates.json"
            if not est_path.exists():
                continue
            full_name = f"{group}/{bench_dir.name}"
            speed[full_name] = _collect_one(est_path)

    if not speed:
        print(f"[collect] criterion dir exists at {CRITERION_ROOT} but no "
              "estimates.json found. Did a bench run complete?")
        return 1

    # Merge into perf JSON. Preserve any other top-level keys the
    # ranking / ann_recall_grid / file_size_breakdown harnesses wrote.
    if PERF_PATH.exists():
        try:
            existing = json.loads(PERF_PATH.read_text())
            if isinstance(existing, list):
                out = {"legacy_rows": existing}
            elif isinstance(existing, dict):
                out = existing
            else:
                out = {}
        except Exception:
            out = {}
    else:
        out = {}

    out[key] = {
        "machine": _machine(),
        "benches": speed,
    }
    PERF_PATH.write_text(json.dumps(out, indent=2) + "\n")

    # One-screen summary — median wall time per bench, sorted by group
    # then name. Convenient for diffing against a prior run without
    # opening the JSON.
    print(f"[collect] wrote {len(speed)} bench(es) under '{key}' in {PERF_PATH}")
    for name in sorted(speed):
        t = speed[name]
        med = t["median_secs"]
        unit, scale = ("ns", 1e9) if med < 1e-6 else \
                      ("µs", 1e6) if med < 1e-3 else \
                      ("ms", 1e3) if med < 1.0 else ("s", 1.0)
        print(f"  {name:48s}  median = {med * scale:>8.3f} {unit}")

    return 0


if __name__ == "__main__":
    import sys
    sys.exit(main())
