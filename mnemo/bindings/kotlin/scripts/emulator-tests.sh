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
# 3. The kill test: the same APK writes, is force-stopped with no flush,
#    and a relaunch counts what survived (MOBILE-2b).
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

# 3. The kill test for the default flush policy (MOBILE-2b): the sample
#    writes, waits past the coalescing window, writes a few more, and is
#    force-stopped with no lifecycle event. A relaunch must find every write
#    older than the window (see DurabilityTest in the sample).
SETTLED=20
wait_for_line() {   # tag, pattern, seconds
  for _ in $(seq 1 "$3"); do
    adb logcat -d -s "$1:I" | grep -q "$2" && return 0
    sleep 1
  done
  return 1
}
# A fresh process: the self-test's activity is still on top, and an intent
# to it would only reach onNewIntent.
adb shell am force-stop io.github.toarchkumar.mnemo.sample
adb logcat -c
adb shell am start -W -n io.github.toarchkumar.mnemo.sample/.MainActivity --es durability write
wait_for_line MnemoDurability "MNEMO_DURABILITY \(late\|failed\)" 60 || { echo "the durability writer never finished" >&2; exit 1; }
adb shell am force-stop io.github.toarchkumar.mnemo.sample
adb logcat -d -s MnemoDurability:I | tee durability.log
grep -q "MNEMO_DURABILITY late" durability.log
adb logcat -c
adb shell am start -W -n io.github.toarchkumar.mnemo.sample/.MainActivity --es durability count
wait_for_line MnemoDurability "MNEMO_DURABILITY \(count\|failed\)" 60 || { echo "the durability reader never finished" >&2; exit 1; }
adb logcat -d -s MnemoDurability:I AndroidRuntime:E | tee -a durability.log
count=$(sed -n 's/.*MNEMO_DURABILITY count=\([0-9]*\).*/\1/p' durability.log | tail -1)
echo "after am force-stop: $count memories (at least $SETTLED expected)"
[ -n "$count" ] && [ "$count" -ge "$SETTLED" ]
