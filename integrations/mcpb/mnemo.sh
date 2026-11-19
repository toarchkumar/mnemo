#!/bin/sh
# The MCP bundle's launcher on macOS and Linux: an MCPB manifest can pick a
# command per OS but not per CPU, so this runs the bundled binary built for
# this machine. Windows runs its .exe directly (manifest.json).
dir=$(dirname "$0")
case "$(uname -s) $(uname -m)" in
  "Darwin arm64") target=aarch64-apple-darwin ;;
  "Darwin x86_64") target=x86_64-apple-darwin ;;
  "Linux x86_64") target=x86_64-unknown-linux-gnu ;;
  "Linux aarch64" | "Linux arm64") target=aarch64-unknown-linux-gnu ;;
  *)
    echo "mnemo: this bundle has no build for $(uname -s) $(uname -m)" >&2
    exit 1
    ;;
esac
exec "$dir/mnemo-$target" "$@"
