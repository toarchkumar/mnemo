#!/bin/sh
# Build the MCP bundle and the registry's server.json for one release.
#
#   integrations/mcpb/build.sh <version> <dir with the five CLI binaries> <out dir>
#
# Run from the repository root. Writes <out>/mnemo-v<version>.mcpb (a zip:
# manifest.json with the version filled in, the launcher and the five
# binaries) and <out>/server.json (the root server.json with the bundle's
# SHA-256). The root server.json must already name <version>: it's bumped
# with the manifests, and release.yml's version check reads it.
set -eu

main() {
  version=$1
  bins=$2
  out=$3
  case "$version" in
    *[!0-9.]* | "") echo "build.sh: the version must be x.y.z, got '$version'" >&2; exit 1 ;;
  esac
  root_version=$(jq -r .version server.json)
  if [ "$root_version" != "$version" ]; then
    echo "build.sh: server.json names $root_version, not $version" >&2
    exit 1
  fi

  stage=$(mktemp -d)
  mkdir -p "$stage/server" "$out"
  jq --arg v "$version" '. + {version: $v}' integrations/mcpb/manifest.json > "$stage/manifest.json"
  cp integrations/mcpb/mnemo.sh "$stage/server/mnemo.sh"
  for target in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu \
    aarch64-unknown-linux-gnu x86_64-pc-windows-msvc; do
    ext=
    [ "$target" = x86_64-pc-windows-msvc ] && ext=.exe
    src="$bins/mnemo-v$version-$target$ext"
    [ -f "$src" ] || { echo "build.sh: missing $src" >&2; exit 1; }
    cp "$src" "$stage/server/mnemo-$target$ext"
  done
  chmod 755 "$stage/server/"*

  bundle="mnemo-v$version.mcpb"
  rm -f "$out/$bundle"
  # -X leaves out extra attributes; the Unix modes (the executable bits)
  # stay in the zip.
  (cd "$stage" && zip -q -X -r - manifest.json server) > "$out/$bundle"
  sha=$(sha256 "$out/$bundle")
  url="https://github.com/toarchkumar/mnemo/releases/download/v$version/$bundle"
  jq --arg u "$url" --arg s "$sha" \
    '.packages |= map(if .registryType == "mcpb" then . + {identifier: $u, fileSha256: $s} else . end)' \
    server.json > "$out/server.json"
  cp "$stage/manifest.json" "$out/manifest.json"
  rm -rf "$stage"
  echo "$out/$bundle $sha"
}

sha256() {
  if command -v sha256sum > /dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

main "$@"
