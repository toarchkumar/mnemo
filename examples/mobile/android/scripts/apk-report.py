#!/usr/bin/env python3
"""The Android size report (MOBILE-1d): the release APK and what MNemo adds.

    python3 scripts/apk-report.py app/build/outputs/apk/release/app-release.apk

Lists the APK's size, the native libraries per ABI (stored uncompressed, as
AGP packages them for minSdk 24), and the dex. Prints Markdown, and appends
it to $GITHUB_STEP_SUMMARY when that's set.
"""
import os
import sys
import zipfile
from collections import defaultdict


def main(path: str) -> None:
    total = os.path.getsize(path)
    libs = defaultdict(dict)
    dex = 0
    with zipfile.ZipFile(path) as z:
        for info in z.infolist():
            name = info.filename
            if name.startswith("lib/") and name.endswith(".so"):
                _, abi, lib = name.split("/", 2)
                stored = "stored" if info.compress_type == zipfile.ZIP_STORED else "COMPRESSED"
                libs[abi][lib] = (info.compress_size, stored)
            elif name.endswith(".dex"):
                dex += info.compress_size
    lines = [
        "### Android sample APK (release, R8 minified)",
        "",
        f"- APK: {total} bytes ({os.path.basename(path)})",
        f"- dex (compressed): {dex} bytes",
        "",
        "| ABI | library | bytes in the APK | packing |",
        "|---|---|---|---|",
    ]
    for abi in sorted(libs):
        for lib, (size, stored) in sorted(libs[abi].items()):
            lines.append(f"| {abi} | {lib} | {size} | {stored} |")
        mnemo = sum(s for l, (s, _) in libs[abi].items() if l in ("libmnemo_uniffi.so", "libjnidispatch.so"))
        lines.append(f"| {abi} | **MNemo native total (with JNA)** | **{mnemo}** | |")
    text = "\n".join(lines) + "\n"
    print(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as f:
            f.write(text)
    compressed = [f"{abi}/{lib}" for abi in libs for lib, (_, s) in libs[abi].items() if s != "stored"]
    if compressed:
        sys.exit(f"native libraries are compressed (they can't load in place): {compressed}")


if __name__ == "__main__":
    main(sys.argv[1])
