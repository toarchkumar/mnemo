#!/usr/bin/env bash
# The android-emulator workflow's device steps, run inside the emulator
# runner from mnemo/bindings/kotlin:
#
#   scripts/emulator-tests.sh <expected page size>
#
# 1. The instrumented tests (connectedDebugAndroidTest), with the page size
#    the image should report.
# 2. The Android sample's R8-minified release APK: install it, launch its
#    self-test, and check logcat for "MNEMO_SELFTEST ok" (MOBILE-1d).
#
# Emulators, the 16 KB image especially, can restart the system server
# shortly after boot. An install then fails ("Can't find service:
# package"), and AGP's test runner reports that as a green run with no
# tests. So this waits until the services have stayed up for a while, and
# retries a run that executed no tests.
set -euo pipefail

PAGE="$1"
SAMPLE=../../../examples/mobile/android
RESULTS=build/outputs/androidTest-results/connected

# Boot completed, and the package and activity services answering on
# `stable` consecutive checks, 3 s apart.
wait_for_system() {
  local stable=0 tries=0
  while [ "$stable" -lt 5 ]; do
    tries=$((tries + 1))
    if [ "$tries" -gt 120 ]; then
      echo "the emulator's system services never settled" >&2
      exit 1
    fi
    if [ "$(adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ] &&
      adb shell cmd package list packages >/dev/null 2>&1 &&
      adb shell cmd activity get-current-user >/dev/null 2>&1; then
      stable=$((stable + 1))
    else
      stable=0
    fi
    sleep 3
  done
}

# The number of tests in the last connected run's XML.
tests_run() {
  python3 - "$RESULTS" <<'PY'
import glob, sys, xml.etree.ElementTree as ET
print(sum(int(ET.parse(p).getroot().get("tests") or 0)
          for p in glob.glob(sys.argv[1] + "/**/TEST-*.xml", recursive=True)))
PY
}

wait_for_system
echo "page size: $(adb shell getconf PAGE_SIZE | tr -d '\r') (expected $PAGE)"

for attempt in 1 2 3; do
  rm -rf "$RESULTS"
  ./gradlew --no-daemon --stacktrace connectedDebugAndroidTest \
    -Pandroid.testInstrumentationRunnerArguments.expectPageSize="$PAGE"
  count=$(tests_run)
  echo "attempt $attempt: $count instrumented tests ran"
  [ "$count" -gt 0 ] && break
  [ "$attempt" -eq 3 ] && { echo "no instrumented tests ran in 3 attempts" >&2; exit 1; }
  wait_for_system
done

"$SAMPLE/gradlew" --no-daemon --stacktrace -p "$SAMPLE" assembleRelease
wait_for_system
for attempt in 1 2 3; do
  adb install -r "$SAMPLE/app/build/outputs/apk/release/app-release.apk" && break
  [ "$attempt" -eq 3 ] && exit 1
  wait_for_system
done
adb logcat -c
adb shell am start -W -n io.github.toarchkumar.mnemo.sample/.MainActivity --ez selftest true
for _ in $(seq 1 60); do
  adb logcat -d -s MnemoSelfTest:I | grep -q MNEMO_SELFTEST && break
  sleep 1
done
adb logcat -d -s MnemoSelfTest:I AndroidRuntime:E | tee selftest.log
grep -q "MNEMO_SELFTEST ok" selftest.log
