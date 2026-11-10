#!/usr/bin/env bash
# The kill test for the default flush policy (MOBILE-2b), on an iOS
# simulator. Builds the TestHost app, launches it to write (some writes
# older than the coalescing window, a few right before the kill), kills it
# with `simctl terminate` (no lifecycle event, no flush), relaunches it to
# count, and checks that every write older than the window survived.
#
#   scripts/kill-test.sh [simulator udid]
#
# Needs XcodeGen. The app's steps are DurabilityTest in
# TestHost/App/MnemoTestHostApp.swift.
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"   # mnemo/bindings/swift
UDID="${1:-}"
BUNDLE=io.github.toarchkumar.mnemo.testhost
SETTLED=20
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [ -z "$UDID" ]; then
  UDID=$(xcrun simctl list devices available --json | python3 -c '
import json, sys
devices = json.load(sys.stdin)["devices"]
for runtime in sorted(devices, reverse=True):
    if "iOS" in runtime:
        for d in devices[runtime]:
            if d["name"].startswith("iPhone"):
                print(d["udid"]); sys.exit()
')
fi
echo "==> simulator $UDID"
xcrun simctl boot "$UDID" 2>/dev/null || true
xcrun simctl bootstatus "$UDID" -b >/dev/null

echo "==> building the TestHost app"
(cd "$HERE/TestHost" && xcodegen generate --quiet)
xcodebuild build-for-testing -project "$HERE/TestHost/MnemoTestHost.xcodeproj" -scheme MnemoHostTests \
  -destination "id=$UDID" -derivedDataPath "$WORK/dd" -quiet
APP=$(find "$WORK/dd/Build/Products" -maxdepth 2 -name MnemoTestHost.app -type d | head -1)
[ -n "$APP" ] || { echo "no MnemoTestHost.app" >&2; exit 1; }
xcrun simctl install "$UDID" "$APP"

# Waits until the app's stdout has a line matching $2.
wait_for() {   # file, pattern, seconds
  for _ in $(seq 1 "$3"); do
    grep -q "$2" "$1" 2>/dev/null && return 0
    sleep 1
  done
  return 1
}

# Launches the app with its console on a file, in the background; the
# console ends when the app does.
launch() {   # mode, log file
  : > "$2"
  xcrun simctl launch --console-pty "$UDID" "$BUNDLE" -mnemoDurability "$1" > "$2" 2>&1 &
}

echo "==> write, then kill"
xcrun simctl terminate "$UDID" "$BUNDLE" 2>/dev/null || true
launch write "$WORK/write.log"
wait_for "$WORK/write.log" "MNEMO_DURABILITY \(late\|failed\)" 60 || { cat "$WORK/write.log" >&2; echo "the writer never finished" >&2; exit 1; }
xcrun simctl terminate "$UDID" "$BUNDLE"
wait
cat "$WORK/write.log"
grep -q "MNEMO_DURABILITY late" "$WORK/write.log"

echo "==> relaunch and count"
launch count "$WORK/count.log"
wait_for "$WORK/count.log" "MNEMO_DURABILITY \(count\|failed\)" 60 || { cat "$WORK/count.log" >&2; echo "the reader never finished" >&2; exit 1; }
xcrun simctl terminate "$UDID" "$BUNDLE" 2>/dev/null || true
wait
cat "$WORK/count.log"
count=$(sed -n 's/.*MNEMO_DURABILITY count=\([0-9]*\).*/\1/p' "$WORK/count.log" | tail -1)
echo "==> after the kill: $count memories (at least $SETTLED expected)"
[ -n "$count" ] && [ "$count" -ge "$SETTLED" ]
echo "==> every write older than the window survived the kill"
