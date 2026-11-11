#!/usr/bin/env python3
"""Validate device-run JSON files against the `device_budget_mobile/1` schema.

    python3 test/scripts/device_runs/validate.py [file.json ...]

With no arguments it checks every `*.json` next to this script. Errors (a
missing or mistyped field, a bad file name) fail with exit code 1. Warnings
(a debug build, a simulator, low-power mode, charging) don't: such a run is
still usable, and the warning says what was different. Standard library only,
so CI and contributors need nothing installed. The schema is documented in
README.md next to this script.
"""

import json
import math
import re
import sys
from pathlib import Path

SCHEMA = "device_budget_mobile/1"
NAME = re.compile(r"^[a-z0-9]+(_[a-z0-9]+)*\.json$")
ISO8601 = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:?\d{2})$")
SEMVER = re.compile(r"^\d+\.\d+\.\d+([-+].*)?$")

# The fields every platform writes, by section: (name, type).
DEVICE = [
    ("platform", str), ("model_identifier", str), ("os", str), ("os_version", str),
    ("ram_bytes", int), ("cpu_count", int), ("page_size", int),
]
BUILD = [("mnemo_version", str), ("configuration", str), ("release_build", bool), ("commit", str)]
CORPUS = [("dimensions", int), ("text_bytes", int)]
# Per result cell (one per corpus size): phase -> the numbers it must carry.
CELL = {
    "file": ["memories", "file_bytes", "bytes_per_memory", "format_version"],
    "handle": ["open_ms", "first_recall_ms", "warm_recall_median_ms", "warm_recall_p95_ms", "handle_rss_kb"],
    "cycles": ["cycles", "open_median_ms", "open_p95_ms", "first_recall_median_ms", "first_recall_p95_ms"],
    "writes": ["single_write_median_ms", "single_write_p95_ms", "batch_size", "batch_write_median_ms",
               "batch_write_p95_ms"],
    "keyopen": ["open_with_key_median_ms", "open_with_key_p95_ms"],
    "background": ["cycles", "close_median_ms", "close_p95_ms", "reopen_median_ms", "reopen_p95_ms"],
}


def is_number(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v)


def check_fields(obj, fields, where, errors):
    for name, kind in fields:
        v = obj.get(name)
        ok = isinstance(v, bool) if kind is bool else (
            isinstance(v, kind) and not isinstance(v, bool))
        if not ok:
            errors.append(f"{where}.{name}: expected {kind.__name__}, got {json.dumps(v)}")


def validate(path):
    """Returns (errors, warnings) for one file."""
    errors, warnings = [], []
    if not NAME.match(path.name):
        errors.append(f"file name {path.name!r}: use lowercase letters, digits and underscores, "
                      "named after the device (iphone16_1.json, pixel8.json)")
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        return [f"not readable JSON: {e}"], warnings
    if not isinstance(doc, dict):
        return ["the top level must be an object"], warnings

    if doc.get("schema") != SCHEMA:
        errors.append(f"schema: expected {SCHEMA!r}, got {json.dumps(doc.get('schema'))}")
    if not (isinstance(doc.get("timestamp"), str) and ISO8601.match(doc["timestamp"])):
        errors.append(f"timestamp: expected an ISO 8601 time, got {json.dumps(doc.get('timestamp'))}")
    if not (is_number(doc.get("elapsed_secs")) and doc["elapsed_secs"] >= 0):
        errors.append("elapsed_secs: expected a non-negative number")

    sections = {}
    for name in ("device", "build", "corpus", "results"):
        v = doc.get(name)
        if not isinstance(v, dict):
            errors.append(f"{name}: expected an object")
            v = {}
        sections[name] = v
    device, build, corpus, results = (sections[k] for k in ("device", "build", "corpus", "results"))

    check_fields(device, DEVICE, "device", errors)
    if device.get("platform") not in ("ios", "android"):
        errors.append(f"device.platform: expected 'ios' or 'android', got {json.dumps(device.get('platform'))}")
    check_fields(build, BUILD, "build", errors)
    if isinstance(build.get("mnemo_version"), str) and not SEMVER.match(build["mnemo_version"]):
        errors.append(f"build.mnemo_version: expected MAJOR.MINOR.PATCH, got {build['mnemo_version']!r}")
    check_fields(corpus, CORPUS, "corpus", errors)

    if not results:
        errors.append("results: expected at least one corpus size")
    for size, cell in results.items():
        where = f"results[{size}]"
        if not (size.isdigit() and int(size) > 0):
            errors.append(f"{where}: keys are corpus sizes (positive integers)")
            continue
        if not isinstance(cell, dict):
            errors.append(f"{where}: expected an object")
            continue
        for phase, numbers in CELL.items():
            obj = cell.get(phase)
            if not isinstance(obj, dict):
                errors.append(f"{where}.{phase}: expected an object")
                continue
            for n in numbers:
                v = obj.get(n)
                if not (is_number(v) and v >= 0):
                    errors.append(f"{where}.{phase}.{n}: expected a non-negative number, got {json.dumps(v)}")
        memories = (cell.get("file") or {}).get("memories")
        if is_number(memories) and memories != int(size):
            errors.append(f"{where}.file.memories: {memories}, but the cell is for {size} memories")

    # Usable, but say what was different.
    if build.get("release_build") is False:
        warnings.append("a debug build: timings run slower than a release build")
    if device.get("simulator") is True or device.get("emulator") is True:
        warnings.append("a simulator or emulator, not a device")
    if device.get("low_power_mode") is True:
        warnings.append("low-power mode was on")
    if device.get("battery_state") in ("charging", "full"):
        warnings.append(f"the battery was {device['battery_state']} (plugged in)")
    if device.get("thermal_state") in ("serious", "critical"):
        warnings.append(f"the thermal state was {device['thermal_state']}")
    if build.get("dirty") is True:
        warnings.append("built from a tree with uncommitted changes")
    return errors, warnings


def main(argv):
    here = Path(__file__).resolve().parent
    files = [Path(a) for a in argv] or sorted(here.glob("*.json"))
    if not files:
        print("no device runs to check")
        return 0
    failed = 0
    for f in files:
        errors, warnings = validate(f)
        status = "FAIL" if errors else "ok"
        print(f"{status:4}  {f.name}")
        for e in errors:
            print(f"      error: {e}")
        for w in warnings:
            print(f"      warning: {w}")
        failed += bool(errors)
    print(f"{len(files) - failed} of {len(files)} device runs valid")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
