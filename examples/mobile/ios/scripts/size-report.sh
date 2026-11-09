#!/usr/bin/env bash
# The iOS size report (MOBILE-1d): what MNemo adds to an app's binary.
#
# App Thinning reports need a signed archive and an export, so CI can't make
# one unsigned. This reports the stripped binary instead: SizeProbe (one
# screen that opens a store, writes and recalls) against SizeBaseline (the
# same screen without MNemo), built Release for arm64 devices, unsigned, and
# stripped the way an App Store build is (all symbols). It also reports the
# full sample app.
#
#   scripts/size-report.sh            # after `xcodegen generate`
#
# Prints a Markdown table; CI appends it to the job summary.
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
DERIVED="${DERIVED:-$HERE/build/size}"
cd "$HERE"

build() {
  xcodebuild build -project MnemoSample.xcodeproj -scheme "$1" -configuration Release \
    -destination 'generic/platform=iOS' -derivedDataPath "$DERIVED" CODE_SIGNING_ALLOWED=NO -quiet >&2
}

stripped() {   # target -> bytes of its stripped executable
  local app="$DERIVED/Build/Products/Release-iphoneos/$1.app"
  local copy="$DERIVED/$1.stripped"
  cp "$app/$1" "$copy"
  xcrun strip "$copy"
  stat -f %z "$copy"
}

for t in SizeBaseline SizeProbe MnemoSample; do build "$t"; done
base=$(stripped SizeBaseline)
probe=$(stripped SizeProbe)
sample=$(stripped MnemoSample)
bundle=$(du -sk "$DERIVED/Build/Products/Release-iphoneos/MnemoSample.app" | cut -f1)

echo "### iOS size (Release, arm64, unsigned, stripped binary)"
echo
echo "App Thinning needs a signed export, so this is the stripped binary-size delta."
echo
echo "| binary | bytes |"
echo "|---|---|"
echo "| SizeBaseline (no MNemo) | $base |"
echo "| SizeProbe (MNemo: open, write, recall, history, essence) | $probe |"
echo "| **MNemo's delta** | **$((probe - base))** |"
echo "| MnemoSample (the full sample) | $sample |"
echo "| MnemoSample.app bundle (KB, unstripped) | $bundle |"
