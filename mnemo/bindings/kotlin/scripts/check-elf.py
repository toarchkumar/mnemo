#!/usr/bin/env python3
"""Checks Android shared libraries for 16 KB page support, and their size.

Usage: check-elf.py <llvm-readelf> [--max-bytes N] [--report-only] <lib.so>...

For each library, from `llvm-readelf -lW`:
  - every LOAD segment is aligned to at least 0x4000 (16 KB), and its file
    offset and address agree modulo 16 KB;
  - the GNU_RELRO segment ends on a 16 KB boundary, or ends where its LOAD
    segment ends (so the next LOAD, itself 16 KB-aligned, starts a fresh
    page), as developer.android.com/guide/practices/page-sizes asks;
  - with --max-bytes, the file is at most N bytes.

Exits non-zero on any failure, unless --report-only.
"""
import os
import subprocess
import sys

PAGE = 0x4000


def segments(readelf, path):
    out = subprocess.run([readelf, "-lW", path], check=True, capture_output=True, text=True).stdout
    segs = []
    for line in out.splitlines():
        parts = line.split()
        if not parts or parts[0] not in ("LOAD", "GNU_RELRO"):
            continue
        # Type Offset VirtAddr PhysAddr FileSiz MemSiz Flg... Align
        offset, vaddr, _paddr, filesz, memsz = (int(p, 16) for p in parts[1:6])
        segs.append({"type": parts[0], "offset": offset, "vaddr": vaddr,
                     "filesz": filesz, "memsz": memsz, "align": int(parts[-1], 16)})
    return segs


def check(readelf, path, max_bytes):
    problems = []
    segs = segments(readelf, path)
    loads = [s for s in segs if s["type"] == "LOAD"]
    if not loads:
        problems.append("no LOAD segments")
    for s in loads:
        if s["align"] < PAGE:
            problems.append(f"LOAD at 0x{s['vaddr']:x} aligned 0x{s['align']:x} (< 0x4000)")
        if s["offset"] % PAGE != s["vaddr"] % PAGE:
            problems.append(f"LOAD at 0x{s['vaddr']:x}: offset and address differ modulo 16 KB")
    for r in (s for s in segs if s["type"] == "GNU_RELRO"):
        end = r["vaddr"] + r["memsz"]
        owner = next((l for l in loads if l["vaddr"] <= r["vaddr"] < l["vaddr"] + l["memsz"]), None)
        if end % PAGE == 0:
            rule = "ends 16 KB-aligned"
        elif owner is not None and end == owner["vaddr"] + owner["memsz"]:
            rule = "ends with its LOAD"
        else:
            rule = None
            problems.append(f"GNU_RELRO ends at 0x{end:x}: not 16 KB-aligned and not the end of its LOAD")
        if rule:
            print(f"  GNU_RELRO 0x{r['vaddr']:x}..0x{end:x} {rule}")
    size = os.path.getsize(path)
    aligns = ", ".join(f"0x{s['align']:x}" for s in loads)
    print(f"{path}: {size} bytes; LOAD aligns {aligns}")
    if max_bytes is not None and size > max_bytes:
        problems.append(f"{size} bytes > {max_bytes}")
    for p in problems:
        print(f"  FAIL {p}")
    return not problems


def main(argv):
    readelf, rest = argv[1], argv[2:]
    max_bytes, report_only = None, False
    libs = []
    i = 0
    while i < len(rest):
        if rest[i] == "--max-bytes":
            max_bytes = int(rest[i + 1])
            i += 2
        elif rest[i] == "--report-only":
            report_only = True
            i += 1
        else:
            libs.append(rest[i])
            i += 1
    if not libs:
        print("no libraries given", file=sys.stderr)
        return 2
    ok = all([check(readelf, lib, max_bytes) for lib in libs])
    print("16 KB check:", "pass" if ok else "FAIL")
    return 0 if ok or report_only else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
