#!/usr/bin/env bash
# The release dry run's SPM consumer (MOBILE-1e): a throwaway package that
# depends on the mnemo-swift release tree, with its binary target pointed
# at the local XCFramework zip, builds on the iOS simulator and runs one
# store round trip.
#
#   scripts/check-release-package.sh <release tree> <zip> [simulator udid]
#
# The release tree is make-release-package.sh's output. The zip's checksum
# is checked against the one in its Package.swift first, so the url and
# checksum the release publishes are the ones tested here (only the source
# of the zip differs: a local path instead of the GitHub Release).
set -euo pipefail

TREE="$(cd "$1" && pwd)"
ZIP="$2"
UDID="${3:-}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

want=$(sed -n 's/.*checksum: "\([0-9a-f]*\)".*/\1/p' "$TREE/Package.swift")
have=$(shasum -a 256 "$ZIP" | cut -d' ' -f1)
if [ -z "$want" ] || [ "$want" != "$have" ]; then
  echo "checksum mismatch: Package.swift has \"$want\", the zip is $have" >&2
  exit 1
fi
echo "==> checksum matches: $have"

# The package under test: the release tree with a local binary target
# (SwiftPM wants a binary target's path inside the package).
cp -R "$TREE" "$WORK/MnemoSwift"
cp "$ZIP" "$WORK/MnemoSwift/MnemoFFI.xcframework.zip"
python3 - "$WORK/MnemoSwift/Package.swift" <<'PY'
import re, sys
p = sys.argv[1]
s = open(p).read()
s, n = re.subn(r'url: "[^"]+",\s*checksum: "[0-9a-f]+"', 'path: "MnemoFFI.xcframework.zip"', s)
assert n == 1, "binaryTarget not found"
open(p, "w").write(s)
PY

mkdir -p "$WORK/Consumer/Sources/Consumer" "$WORK/Consumer/Tests/ConsumerTests"
cat > "$WORK/Consumer/Package.swift" <<'SWIFT'
// swift-tools-version:6.0
import PackageDescription

let package = Package(
    name: "Consumer",
    platforms: [.iOS(.v15), .macOS(.v12)],
    dependencies: [.package(path: "../MnemoSwift")],
    targets: [
        .target(name: "Consumer", dependencies: [.product(name: "Mnemo", package: "MnemoSwift")]),
        .testTarget(name: "ConsumerTests", dependencies: ["Consumer"]),
    ],
    swiftLanguageModes: [.v5]
)
SWIFT
cat > "$WORK/Consumer/Sources/Consumer/Consumer.swift" <<'SWIFT'
import Foundation
import Mnemo

/// Keys in memory: the bare test runner has no Keychain entitlement.
public final class MemoryKeys: MnemoKeyStore, @unchecked Sendable {
    private let lock = NSLock()
    private var keys: [String: Data] = [:]
    public init() {}
    public func storeKey(_ key: Data, id: String, policy: KeyPolicy) throws {
        lock.lock(); defer { lock.unlock() }; keys[id] = key
    }
    public func key(id: String) throws -> Data {
        lock.lock(); defer { lock.unlock() }
        guard let k = keys[id] else { throw MnemoStoreError.keyMissing(keyID: id, path: nil) }
        return k
    }
    public func keyIDs() throws -> [String] { lock.lock(); defer { lock.unlock() }; return Array(keys.keys) }
    public func deleteKey(id: String) throws { lock.lock(); defer { lock.unlock() }; keys[id] = nil }
}

/// One round trip: write, recall, an essence item, close, reopen.
public func roundTrip(in directory: URL) async throws -> (count: UInt64, top: String, essence: String) {
    let keys = MemoryKeys()
    let config = MnemoStoreConfiguration(name: "consumer", dimensions: 4, directory: directory)
    let store = try MnemoStore(configuration: config, keyStore: keys)
    try await store.remember("the user prefers green tea", vector: [1, 0, 0, 0])
    try await store.remember("flight UA 931 on 12 March", vector: [0, 1, 0, 0])
    try await store.essenceAdd(.decisions, "keep the sample small")
    let top = try await store.recall(vector: [0.9, 0.1, 0, 0], options: RecallOptions(topK: 1)).hits.first?.memory.content ?? ""
    try await store.close()
    let again = try MnemoStore(configuration: config, keyStore: keys)
    let count = try await again.count()
    let essence = try await again.essence()
    try await again.close()
    return (count, top, essence)
}
SWIFT
cat > "$WORK/Consumer/Tests/ConsumerTests/ConsumerTests.swift" <<'SWIFT'
import Foundation
import XCTest
@testable import Consumer

final class ConsumerTests: XCTestCase {
    func testRoundTrip() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let result = try await roundTrip(in: dir)
        XCTAssertGreaterThanOrEqual(result.count, 2)
        XCTAssertEqual(result.top, "the user prefers green tea")
        XCTAssertTrue(result.essence.contains("keep the sample small"), result.essence)
    }
}
SWIFT

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
echo "==> xcodebuild test on simulator $UDID"
cd "$WORK/Consumer"
set -o pipefail
# SwiftPM names the test scheme "<package>-Package" when the package has
# more than one target, else after the package.
SCHEME=$(xcodebuild -list -json 2>/dev/null | python3 -c '
import json, sys
schemes = json.load(sys.stdin)["workspace"]["schemes"]
print("Consumer-Package" if "Consumer-Package" in schemes else schemes[0])')
echo "==> scheme $SCHEME"
xcodebuild test -scheme "$SCHEME" -destination "id=$UDID" -derivedDataPath "$WORK/dd" 2>&1 \
  | tee "$WORK/test.log" | grep -E "Test Case .*(passed|failed)|error:|\*\* TEST" || true
grep -q "TEST SUCCEEDED" "$WORK/test.log"
echo "==> the SPM consumer resolved the release package, built on the simulator and passed"
