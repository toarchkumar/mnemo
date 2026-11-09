#!/usr/bin/env bash
# Builds build/MnemoFFI.xcframework from the mnemo-uniffi crate and
# generates the Swift bindings into Sources/Mnemo/Generated/.
#
# Slices: ios-arm64, ios-arm64_x86_64-simulator, macos-arm64.
#
#   scripts/build-xcframework.sh            # all three slices
#   MNEMO_SLICES=macos scripts/build-xcframework.sh   # only macOS (swift test)
#
# Needs the Rust targets aarch64-apple-ios, aarch64-apple-ios-sim,
# x86_64-apple-ios and aarch64-apple-darwin, and Xcode (or, for the macOS
# slice alone, the Command Line Tools).
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"        # mnemo/bindings/swift
WORKSPACE="$(cd "$HERE/../.." && pwd)"           # mnemo/ (the Cargo workspace)
BUILD="$HERE/build"
GENERATED="$HERE/Sources/Mnemo/Generated"
SLICES="${MNEMO_SLICES:-ios ios-sim macos}"
MODULE="mnemo_uniffiFFI"
LIB="libmnemo_uniffi.a"

export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-15.0}"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-12.0}"

# The SDK for an `xcrun --sdk` name. When xcrun can't look it up (for
# example the Command Line Tools are selected), falls back to the SDK in
# DEVELOPER_DIR or /Applications/Xcode.app.
sdk_path() {
  local sdk="$1" platform="$2" dev
  if xcrun --sdk "$sdk" --show-sdk-path 2>/dev/null; then return; fi
  for dev in "${DEVELOPER_DIR:-}" "$(xcode-select -p 2>/dev/null)" /Applications/Xcode.app/Contents/Developer; do
    local path="$dev/Platforms/$platform.platform/Developer/SDKs/$platform.sdk"
    if [ -n "$dev" ] && [ -d "$path" ]; then echo "$path"; return; fi
  done
  echo "no $sdk SDK found" >&2
  exit 1
}

# Builds the static library for one Rust target.
build_static() {
  local target="$1" sdk="$2" platform="$3"
  echo "==> cargo: $target"
  (cd "$WORKSPACE" && SDKROOT="$(sdk_path "$sdk" "$platform")" \
    cargo rustc -p mnemo-uniffi --lib --release --target "$target" --crate-type staticlib)
}

rm -rf "$BUILD/MnemoFFI.xcframework" "$BUILD/MnemoFFI.xcframework.zip" "$BUILD/headers" "$BUILD/gen"
mkdir -p "$BUILD" "$GENERATED"

# The bindings are generated from the macOS dylib, so the macOS build always
# runs. Each crate type is built on its own: with `lib` among them, the
# static library would skip fat LTO and come out about 4x larger.
MACOS_SDK="${MACOS_SDKROOT:-$(sdk_path macosx MacOSX)}"
echo "==> cargo: aarch64-apple-darwin (cdylib for bindgen)"
(cd "$WORKSPACE" && SDKROOT="$MACOS_SDK" \
  cargo rustc -p mnemo-uniffi --lib --release --target aarch64-apple-darwin --crate-type cdylib)
DARWIN="$WORKSPACE/target/aarch64-apple-darwin/release"

echo "==> uniffi-bindgen: swift"
(cd "$WORKSPACE" && SDKROOT="$MACOS_SDK" \
  cargo run -q -p mnemo-uniffi --features cli --bin uniffi-bindgen -- \
  generate --library "$DARWIN/libmnemo_uniffi.dylib" --language swift --out-dir "$BUILD/gen")
cp "$BUILD/gen/mnemo_uniffi.swift" "$GENERATED/mnemo_uniffi.swift"

# One headers directory per slice; the module map sits in a subdirectory named
# after the module so it can't collide with another XCFramework's.
mkdir -p "$BUILD/headers/$MODULE"
cp "$BUILD/gen/$MODULE.h" "$BUILD/headers/$MODULE/"
cp "$BUILD/gen/$MODULE.modulemap" "$BUILD/headers/$MODULE/module.modulemap"

ARGS=()
PLIST_LIBS=""
add_slice() {   # dir, identifier, platform, variant (or ""), archs...
  local dir="$1" id="$2" platform="$3" variant="$4"; shift 4
  ARGS+=(-library "$dir/$LIB" -headers "$BUILD/headers")
  local archs="" a
  for a in "$@"; do archs="$archs<string>$a</string>"; done
  local variant_xml=""
  [ -n "$variant" ] && variant_xml="<key>SupportedPlatformVariant</key><string>$variant</string>"
  PLIST_LIBS="$PLIST_LIBS<dict><key>HeadersPath</key><string>Headers</string><key>LibraryIdentifier</key><string>$id</string><key>LibraryPath</key><string>$LIB</string><key>SupportedArchitectures</key><array>$archs</array><key>SupportedPlatform</key><string>$platform</string>$variant_xml</dict>"
  SLICE_DIRS+=("$id:$dir")
}
SLICE_DIRS=()

for slice in $SLICES; do
  case "$slice" in
    ios)
      build_static aarch64-apple-ios iphoneos iPhoneOS
      add_slice "$WORKSPACE/target/aarch64-apple-ios/release" ios-arm64 ios "" arm64 ;;
    ios-sim)
      build_static aarch64-apple-ios-sim iphonesimulator iPhoneSimulator
      build_static x86_64-apple-ios iphonesimulator iPhoneSimulator
      mkdir -p "$BUILD/ios-sim"
      lipo -create \
        "$WORKSPACE/target/aarch64-apple-ios-sim/release/$LIB" \
        "$WORKSPACE/target/x86_64-apple-ios/release/$LIB" \
        -output "$BUILD/ios-sim/$LIB"
      add_slice "$BUILD/ios-sim" ios-arm64_x86_64-simulator ios simulator arm64 x86_64 ;;
    macos)
      (cd "$WORKSPACE" && SDKROOT="$MACOS_SDK" \
        cargo rustc -p mnemo-uniffi --lib --release --target aarch64-apple-darwin --crate-type staticlib)
      add_slice "$DARWIN" macos-arm64 macos "" arm64 ;;
    *) echo "unknown slice: $slice" >&2; exit 1 ;;
  esac
done

OUT="$BUILD/MnemoFFI.xcframework"
if xcodebuild -version >/dev/null 2>&1; then
  echo "==> xcodebuild -create-xcframework"
  xcodebuild -create-xcframework "${ARGS[@]}" -output "$OUT" >/dev/null
else
  # xcodebuild isn't usable (Command Line Tools only, or a broken Xcode):
  # assemble the same layout by hand.
  echo "==> assembling the XCFramework without xcodebuild"
  mkdir -p "$OUT"
  for entry in "${SLICE_DIRS[@]}"; do
    id="${entry%%:*}"; dir="${entry#*:}"
    mkdir -p "$OUT/$id"
    cp "$dir/$LIB" "$OUT/$id/$LIB"
    cp -R "$BUILD/headers" "$OUT/$id/Headers"
  done
  cat > "$OUT/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>AvailableLibraries</key><array>$PLIST_LIBS</array><key>CFBundlePackageType</key><string>XFWK</string><key>XCFrameworkFormatVersion</key><string>1.0</string></dict></plist>
PLIST
fi

(cd "$BUILD" && ditto -c -k --sequesterRsrc --keepParent MnemoFFI.xcframework MnemoFFI.xcframework.zip)

echo "==> sizes (bytes)"
echo "MnemoFFI.xcframework.zip $(stat -f %z "$BUILD/MnemoFFI.xcframework.zip")"
for d in "$OUT"/*/; do
  id="$(basename "$d")"
  echo "$id/$LIB $(stat -f %z "$d/$LIB")"
done
