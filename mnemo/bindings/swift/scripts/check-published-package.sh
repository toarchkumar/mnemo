#!/usr/bin/env bash
# The release workflow's last Swift check (MOBILE-2a): a throwaway package
# that depends on a published mnemo-swift version, resolved from GitHub (the
# real tag, the real zip url and checksum), then built for the iOS
# simulator. It runs in two phases, so a hang shows which one it was:
#
#   scripts/check-published-package.sh resolve <version> <work dir>
#   scripts/check-published-package.sh build   <work dir>
#
# `resolve` writes the package, resolves it and picks the scheme from
# `xcodebuild -list -json` (SwiftPM names it "<package>-Package" when the
# package graph has more than one target, else after the package), and
# saves it in <work dir>/scheme. `build` builds that scheme.
#
#   scripts/check-published-package.sh latest
#
# prints the newest MAJOR.MINOR.PATCH tag mnemo-swift has (nothing if it
# has none). The dry run checks that version, so the step runs before
# every tag, not first on one.
#
# Nothing here may wait for a person:
# - git is the command-line git (-scmProvider system) with
#   GIT_TERMINAL_PROMPT=0;
# - package credentials come from netrc (-packageAuthorizationProvider
#   netrc), not the Keychain, which can block on a CI runner. v0.12.0's
#   check hung right after "Checking out", where Xcode downloads the
#   binary target;
# - the zip is first downloaded with curl and checked against the
#   checksum in the published Package.swift, so a download problem shows
#   up as one;
# - every xcodebuild runs under a watchdog: on a hang it prints the live
#   processes and a stack sample of xcodebuild, then fails.
set -euo pipefail

REPO=https://github.com/toarchkumar/mnemo-swift
export GIT_TERMINAL_PROMPT=0
XCB_PACKAGES=(-scmProvider system -packageAuthorizationProvider netrc)

# run_watched <seconds> <label> <command...>
run_watched() {
  local limit="$1" label="$2"; shift 2
  "$@" &
  local pid=$! waited=0
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$waited" -ge "$limit" ]; then
      echo "::error::$label is still running after ${limit}s; the live processes:" >&2
      ps -A -o pid,ppid,stat,etime,command | grep -i -E 'xcodebuild|swift|git|curl|ssh|security' | grep -v grep >&2 || true
      for p in $(pgrep -x xcodebuild || true); do
        echo "--- stack sample of xcodebuild $p ---" >&2
        sample "$p" 3 2>/dev/null | sed -n '1,80p' >&2 || true
      done
      kill "$pid" 2>/dev/null || true
      pkill -x xcodebuild 2>/dev/null || true
      return 124
    fi
    sleep 5
    waited=$((waited + 5))
  done
  wait "$pid"
}

case "${1:-}" in
  latest)
    git ls-remote --tags --refs "$REPO" \
      | sed -n 's|.*refs/tags/\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)$|\1|p' \
      | sort -t. -k1,1n -k2,2n -k3,3n | tail -1
    ;;
  resolve)
    VERSION="${2:?version}"
    D="${3:?work dir}"
    if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
      echo "version must be MAJOR.MINOR.PATCH, got \"$VERSION\"" >&2
      exit 2
    fi
    rm -rf "$D"
    mkdir -p "$D/Sources/Check"
    cat > "$D/Package.swift" <<SWIFT
// swift-tools-version:6.0
import PackageDescription
let package = Package(
    name: "Check",
    platforms: [.iOS(.v15), .macOS(.v12)],
    dependencies: [.package(url: "$REPO", exact: "$VERSION")],
    targets: [.target(name: "Check", dependencies: [.product(name: "Mnemo", package: "mnemo-swift")])]
)
SWIFT
    cat > "$D/Sources/Check/Check.swift" <<'SWIFT'
import Mnemo
public func check() -> Int { generateKey().count }
SWIFT
    cd "$D"
    echo "==> the published Package.swift at $VERSION"
    git clone -q --depth 1 --branch "$VERSION" "$REPO" "$D/mirror"
    URL=$(sed -n 's/.*url: "\(https:[^"]*\.zip\)".*/\1/p' "$D/mirror/Package.swift")
    WANT=$(sed -n 's/.*checksum: "\([0-9a-f]*\)".*/\1/p' "$D/mirror/Package.swift")
    echo "    binary target $URL"
    echo "    checksum $WANT"
    echo "==> downloading the zip with curl"
    curl -fsSL --retry 3 --connect-timeout 20 --max-time 180 -o "$D/zip" "$URL"
    HAVE=$(shasum -a 256 "$D/zip" | cut -d' ' -f1)
    if [ "$HAVE" != "$WANT" ]; then
      echo "checksum mismatch: Package.swift has $WANT, the download is $HAVE" >&2
      exit 1
    fi
    echo "    $(stat -f %z "$D/zip") bytes, checksum matches"
    echo "==> resolving mnemo-swift $VERSION with xcodebuild"
    run_watched 300 "xcodebuild -resolvePackageDependencies" \
      xcodebuild -resolvePackageDependencies "${XCB_PACKAGES[@]}" \
      -clonedSourcePackagesDirPath "$D/spm" -packageCachePath "$D/cache"
    SCHEME=$(xcodebuild -list -json "${XCB_PACKAGES[@]}" \
      -clonedSourcePackagesDirPath "$D/spm" -packageCachePath "$D/cache" 2>/dev/null | python3 -c '
import json, sys
schemes = json.load(sys.stdin)["workspace"]["schemes"]
print("Check-Package" if "Check-Package" in schemes else ("Check" if "Check" in schemes else ""))')
    if [ -z "$SCHEME" ]; then
      echo "no Check or Check-Package scheme:" >&2
      xcodebuild -list "${XCB_PACKAGES[@]}" -clonedSourcePackagesDirPath "$D/spm" -packageCachePath "$D/cache" >&2 || true
      exit 1
    fi
    echo "$SCHEME" > "$D/scheme"
    echo "$VERSION" > "$D/version"
    echo "==> resolved; scheme $SCHEME"
    ;;
  build)
    D="${2:?work dir}"
    SCHEME=$(cat "$D/scheme")
    VERSION=$(cat "$D/version")
    cd "$D"
    echo "==> building scheme $SCHEME for the iOS simulator"
    run_watched 600 "xcodebuild build" \
      xcodebuild build -scheme "$SCHEME" -destination 'generic/platform=iOS Simulator' \
      "${XCB_PACKAGES[@]}" -disableAutomaticPackageResolution \
      -clonedSourcePackagesDirPath "$D/spm" -packageCachePath "$D/cache" -derivedDataPath "$D/dd"
    echo "==> resolved and built mnemo-swift $VERSION from GitHub"
    ;;
  *)
    echo "usage: $0 latest | resolve <version> <work dir> | build <work dir>" >&2
    exit 2
    ;;
esac
