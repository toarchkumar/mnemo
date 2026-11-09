#!/usr/bin/env bash
# Builds the native half of the MNemo Android library (MOBILE-1c):
#
#   generated/host/     libmnemo_uniffi for this machine (the JVM tests load it)
#   generated/kotlin/   the UniFFI Kotlin bindings, generated from that library
#   generated/jniLibs/  libmnemo_uniffi.so per Android ABI, through cargo-ndk,
#                       linked for 16 KB pages and stripped
#
# Needs: Rust with the Android targets, cargo-ndk, and an NDK r28 or later
# (ANDROID_NDK_HOME, or the newest NDK under ANDROID_HOME).
#
# Environment:
#   MNEMO_ABIS    the Android ABIs (default "arm64-v8a x86_64"); empty builds
#                 only the host library and the bindings (enough for the JVM
#                 tests)
#   ANDROID_API   the minimum API level the .so is linked for (default 24)
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"   # mnemo/bindings/kotlin
CRATE="$(cd "$HERE/../.." && pwd)"                        # mnemo/ (the workspace)
OUT="$HERE/generated"
ABIS="${MNEMO_ABIS-arm64-v8a x86_64}"
API="${ANDROID_API:-24}"

cd "$CRATE"
TARGET_DIR="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"

case "$(uname -s)" in
  Darwin) HOST_LIB="libmnemo_uniffi.dylib" ;;
  *)      HOST_LIB="libmnemo_uniffi.so" ;;
esac

echo "==> host library (release, for bindgen and the JVM tests)"
cargo build -p mnemo-uniffi --lib --release
rm -rf "$OUT/host"
mkdir -p "$OUT/host"
cp "$TARGET_DIR/release/$HOST_LIB" "$OUT/host/"

echo "==> Kotlin bindings"
rm -rf "$OUT/kotlin"
cargo run -q -p mnemo-uniffi --features cli --bin uniffi-bindgen -- \
  generate --library "$OUT/host/$HOST_LIB" --language kotlin \
  --config "$HERE/uniffi.toml" --out-dir "$OUT/kotlin" --no-format

if [ -n "$ABIS" ]; then
  echo "==> Android: $ABIS (API $API, 16 KB pages)"
  TARGETS=()
  for abi in $ABIS; do TARGETS+=(-t "$abi"); done
  # An explicit RUSTFLAGS replaces .cargo/config.toml's, so the ARMv8 AES
  # cfgs are repeated here (they're no-ops on x86_64). NDK r28+ already
  # aligns to 16 KB; the flags make it explicit for any NDK.
  RUSTFLAGS="${RUSTFLAGS:-} --cfg aes_armv8 --cfg polyval_armv8 \
-C link-arg=-Wl,-z,max-page-size=16384 -C link-arg=-Wl,-z,common-page-size=16384" \
    cargo ndk --platform "$API" "${TARGETS[@]}" -o "$OUT/jniLibs" \
      build -p mnemo-uniffi --lib --release
  # Only the listed ABIs, nothing left over from an earlier run.
  for dir in "$OUT/jniLibs"/*; do
    case " $ABIS " in *" $(basename "$dir") "*) ;; *) rm -rf "$dir" ;; esac
  done
  # Strip with the NDK's llvm-strip (whatever cargo-ndk did), so the AAR
  # ships the size the gate measures.
  NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
  if [ -z "$NDK" ] && [ -d "${ANDROID_HOME:-}/ndk" ]; then
    NDK="$(ls -d "$ANDROID_HOME"/ndk/* | sort -V | tail -1)"
  fi
  STRIP="$(ls "$NDK"/toolchains/llvm/prebuilt/*/bin/llvm-strip 2>/dev/null | head -1 || true)"
  if [ -z "$STRIP" ]; then
    echo "error: no llvm-strip under the NDK ($NDK); set ANDROID_NDK_HOME" >&2
    exit 1
  fi
  for so in "$OUT"/jniLibs/*/libmnemo_uniffi.so; do
    "$STRIP" --strip-all "$so"
  done
fi

echo "==> sizes (bytes)"
# Only the directories this run made (jniLibs is absent with MNEMO_ABIS="").
DIRS=()
for d in "$OUT/host" "$OUT/jniLibs"; do [ -d "$d" ] && DIRS+=("$d"); done
find "${DIRS[@]}" -name 'libmnemo_uniffi.*' | sort | while read -r f; do
  printf '%s %s\n' "${f#"$OUT"/}" "$(wc -c < "$f" | tr -d ' ')"
done
