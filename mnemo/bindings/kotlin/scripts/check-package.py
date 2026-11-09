#!/usr/bin/env python3
"""Checks what publishToMavenLocal produced, and reports the AAR's sizes.

Usage: check-package.py <llvm-readelf>   (run from mnemo/bindings/kotlin)

- The POM has what Maven Central requires (name, description, URL, a
  licence, a developer, SCM), and JNA as an exact-version `aar` dependency.
- The AAR, sources and javadoc jars are there.
- Reports the AAR's size, its native libraries, and the native size an app
  gets with JNA's dispatch library for the same ABIs; checks JNA's libraries
  for 16 KB pages (reported, not gated: JNA isn't built here).

Writes a Markdown summary to $GITHUB_STEP_SUMMARY when set.
"""
import glob
import os
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
import zipfile

ABIS = ("arm64-v8a", "x86_64")
NS = {"m": "http://maven.apache.org/POM/4.0.0"}


def prop(name):
    with open("gradle.properties") as f:
        for line in f:
            if line.startswith(name + "="):
                return line.split("=", 1)[1].strip()
    raise SystemExit(f"no {name} in gradle.properties")


def jna_version():
    with open("build.gradle.kts") as f:
        for line in f:
            if line.startswith('val jnaVersion = "'):
                return line.split('"')[1]
    raise SystemExit("no jnaVersion in build.gradle.kts")


def main(readelf):
    group, version, artifact = prop("GROUP"), prop("VERSION_NAME"), "mnemo-android"
    base = os.path.expanduser(f"~/.m2/repository/{group.replace('.', '/')}/{artifact}/{version}")
    stem = f"{base}/{artifact}-{version}"
    failures = []
    summary = [f"### {group}:{artifact}:{version} (publishToMavenLocal)"]

    pom = ET.parse(f"{stem}.pom").getroot()
    for path in ("m:name", "m:description", "m:url", "m:licenses/m:license/m:name",
                 "m:licenses/m:license/m:url", "m:developers/m:developer/m:id",
                 "m:developers/m:developer/m:name", "m:scm/m:url", "m:scm/m:connection",
                 "m:scm/m:developerConnection"):
        node = pom.find(path, NS)
        if node is None or not (node.text or "").strip():
            failures.append(f"POM: missing {path}")
    deps = {d.findtext("m:artifactId", namespaces=NS): d for d in pom.findall("m:dependencies/m:dependency", NS)}
    jna = deps.get("jna")
    if jna is None:
        failures.append("POM: no JNA dependency")
    else:
        if jna.findtext("m:version", namespaces=NS) != jna_version():
            failures.append(f"POM: JNA version {jna.findtext('m:version', namespaces=NS)} != {jna_version()}")
        if jna.findtext("m:type", namespaces=NS) != "aar":
            failures.append("POM: JNA isn't the aar artifact")
    for suffix in (".aar", "-sources.jar", "-javadoc.jar", ".module"):
        if not os.path.isfile(stem + suffix):
            failures.append(f"missing {artifact}-{version}{suffix}")
    summary.append("- POM fields, JNA (aar, exact), AAR, sources and javadoc jars: "
                   + ("ok" if not failures else "**FAIL**"))

    aar = f"{stem}.aar"
    if os.path.isfile(aar):
        summary.append(f"- AAR: {os.path.getsize(aar)} bytes")
        ours = {}
        with zipfile.ZipFile(aar) as z:
            for info in z.infolist():
                if info.filename.startswith("jni/") and info.filename.endswith(".so"):
                    ours[info.filename] = info.file_size
        for name, size in sorted(ours.items()):
            summary.append(f"  - `{name}`: {size} bytes")
        if sorted(name.split("/")[1] for name in ours) != sorted(ABIS):
            failures.append(f"AAR native ABIs {sorted(ours)} != {list(ABIS)}")

        jna_aars = glob.glob(os.path.expanduser(
            f"~/.gradle/caches/modules-2/files-2.1/net.java.dev.jna/jna/{jna_version()}/*/jna-{jna_version()}.aar"))
        if jna_aars:
            theirs = {}
            with tempfile.TemporaryDirectory() as tmp, zipfile.ZipFile(jna_aars[0]) as z:
                for abi in ABIS:
                    name = f"jni/{abi}/libjnidispatch.so"
                    theirs[name] = z.getinfo(name).file_size
                    z.extract(name, tmp)
                libs = [os.path.join(tmp, n) for n in theirs]
                out = subprocess.run([sys.executable, os.path.join(os.path.dirname(__file__), "check-elf.py"),
                                      readelf, "--report-only", *libs], capture_output=True, text=True).stdout
                print(out.replace(tmp + "/", ""))
                jna_16k = "16 KB check: pass" in out
            for abi in ABIS:
                total = ours.get(f"jni/{abi}/libmnemo_uniffi.so", 0) + theirs[f"jni/{abi}/libjnidispatch.so"]
                summary.append(f"- native for {abi} with JNA {jna_version()}: {total} bytes "
                               f"(libjnidispatch.so {theirs[f'jni/{abi}/libjnidispatch.so']})")
            summary.append(f"- JNA's libjnidispatch.so, 16 KB pages: {'pass' if jna_16k else '**not aligned**'}")
        else:
            summary.append("- JNA's AAR isn't in the Gradle cache; no total reported")

    for f in failures:
        print("FAIL", f)
        summary.append(f"- **FAIL** {f}")
    text = "\n".join(summary)
    print(text)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as f:
            f.write(text + "\n")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
