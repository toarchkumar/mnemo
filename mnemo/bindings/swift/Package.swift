// swift-tools-version:6.0
//
// The development manifest: the binary target is the XCFramework that
// scripts/build-xcframework.sh builds locally. The release manifest (MOBILE-1e,
// published to toarchkumar/mnemo-swift) points the same target at the
// release zip and its checksum instead.
import PackageDescription

let package = Package(
    name: "Mnemo",
    platforms: [.iOS(.v15), .macOS(.v12)],
    products: [
        .library(name: "Mnemo", targets: ["Mnemo"]),
    ],
    targets: [
        .binaryTarget(name: "MnemoFFI", path: "build/MnemoFFI.xcframework"),
        .target(
            name: "Mnemo",
            dependencies: ["MnemoFFI"],
            path: "Sources/Mnemo",
            linkerSettings: [.linkedFramework("Security")]
        ),
        .testTarget(
            name: "MnemoTests",
            dependencies: ["Mnemo"],
            path: "Tests/MnemoTests"
        ),
    ],
    // The UniFFI-generated Swift predates Swift 6's strict concurrency checks.
    swiftLanguageModes: [.v5]
)
