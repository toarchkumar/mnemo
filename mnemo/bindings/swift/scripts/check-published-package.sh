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
# Resolution uses the command-line git (-scmProvider system) with
# GIT_TERMINAL_PROMPT=0, so anything that would wait for credentials fails
# at once instead.
set -euo pipefail

REPO=https://github.com/toarchkumar/mnemo-swift
export GIT_TERMINAL_PROMPT=0

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
    echo "==> resolving mnemo-swift $VERSION from $REPO"
    xcodebuild -resolvePackageDependencies -scmProvider system \
      -clonedSourcePackagesDirPath "$D/spm"
    SCHEME=$(xcodebuild -list -json -scmProvider system \
      -clonedSourcePackagesDirPath "$D/spm" 2>/dev/null | python3 -c '
import json, sys
schemes = json.load(sys.stdin)["workspace"]["schemes"]
print("Check-Package" if "Check-Package" in schemes else ("Check" if "Check" in schemes else ""))')
    if [ -z "$SCHEME" ]; then
      echo "no Check or Check-Package scheme:" >&2
      xcodebuild -list -scmProvider system -clonedSourcePackagesDirPath "$D/spm" >&2 || true
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
    xcodebuild build -scheme "$SCHEME" -destination 'generic/platform=iOS Simulator' \
      -scmProvider system -disableAutomaticPackageResolution \
      -clonedSourcePackagesDirPath "$D/spm" -derivedDataPath "$D/dd"
    echo "==> resolved and built mnemo-swift $VERSION from GitHub"
    ;;
  *)
    echo "usage: $0 latest | resolve <version> <work dir> | build <work dir>" >&2
    exit 2
    ;;
esac
