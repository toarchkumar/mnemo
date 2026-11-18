#!/bin/sh
# Retake docs/img/inspect-findings.png: the Findings tab of `mnemo inspect`
# on a synthetic demo brain (mnemo/examples/inspect_demo_brain.rs), light
# theme, with headless Chrome. Nothing here reads the project brain or the
# keychain: the demo brain lives in a temp directory under a throwaway
# passphrase. macOS or Linux; set CHROME to the browser's binary if it isn't
# found.
#
#   sh test/scripts/inspect_screenshot.sh [out.png]
set -eu

main() {
    root=$(cd "$(dirname "$0")/../.." && pwd)
    out=${1:-"$root/docs/img/inspect-findings.png"}
    chrome=${CHROME:-}
    if [ -z "$chrome" ]; then
        for c in "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
            google-chrome chromium chromium-browser; do
            if command -v "$c" >/dev/null 2>&1 || [ -x "$c" ]; then
                chrome=$c
                break
            fi
        done
    fi
    [ -n "$chrome" ] || { echo "no Chrome found; set CHROME" >&2; exit 1; }

    work=$(mktemp -d)
    pid=""
    trap 'if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || true; fi; rm -rf "$work"' EXIT
    pass="demo-screenshot-passphrase"
    (cd "$root/mnemo" && cargo build -q --release --example inspect_demo_brain --bin mnemo)
    "$root/mnemo/target/release/examples/inspect_demo_brain" "$work/demo.mnemo" "$pass" >/dev/null

    # From inside the temp directory, so the page's header shows only
    # `demo.mnemo`, not a path on this machine.
    (cd "$work" && MNEMO_PASSPHRASE=$pass exec "$root/mnemo/target/release/mnemo" inspect \
        demo.mnemo --idle-timeout 2m >"$work/inspect.out" 2>&1) &
    pid=$!
    url=""
    i=0
    while [ $i -lt 50 ]; do
        url=$(grep -m1 '^http://' "$work/inspect.out" 2>/dev/null || true)
        [ -n "$url" ] && break
        sleep 0.2
        i=$((i + 1))
    done
    [ -n "$url" ] || { cat "$work/inspect.out" >&2; exit 1; }

    mkdir -p "$(dirname "$out")"
    "$chrome" --headless=new --disable-gpu --no-first-run --no-default-browser-check \
        --user-data-dir="$work/profile" --hide-scrollbars --force-color-profile=srgb \
        --blink-settings=preferredColorScheme=1 --window-size=1200,540 \
        --virtual-time-budget=5000 --screenshot="$out" "$url#findings" >/dev/null 2>&1
    bytes=$(wc -c <"$out" | tr -d ' ')
    echo "$out: $bytes bytes"
    [ "$bytes" -lt 307200 ] || { echo "over 300 KB" >&2; exit 1; }
}

main "$@"
