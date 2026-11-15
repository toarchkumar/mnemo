#!/bin/sh
# Install mnemo, the encrypted single-file memory for AI agents.
#
#   curl -fsSL https://toarchkumar.github.io/mnemo/install.sh | sh
#
# What it does (docs/design/12.2-onboard.md, section 1):
# 1. Works out your platform (Linux or macOS, x86_64 or arm64).
# 2. Finds the version: the latest release, or MNEMO_VERSION=0.16.0.
# 3. Downloads that release's binary and its SHA256SUMS from GitHub, over
#    HTTPS only, into a new temporary directory.
# 4. Checks the binary's SHA-256 against SHA256SUMS. A mismatch, or no tool
#    to check with, stops it, and nothing is installed.
# 5. With the GitHub CLI (`gh`) installed and signed in, it also checks the
#    build's attestation: a signature that ties the binary to this
#    repository's release workflow. Without `gh`, it says only the SHA-256
#    was checked.
# 6. Installs to ~/.local/bin/mnemo (no sudo), replacing an older one in a
#    single move.
# 7. Prints `mnemo --version`, then the next step.
#
# What the checks protect: the SHA-256 catches a corrupt or cut-short
# download, but SHA256SUMS comes from the same release, so it can't catch a
# release whose files were swapped. The attestation can: a swapped file has
# none. This script itself comes from the site over HTTPS; to read it before
# running it, download it first.
#
# MNEMO_INSTALL_BASE (for the release workflow's own check) names a local
# directory holding the binary and SHA256SUMS; then nothing is downloaded,
# and the attestation is skipped.
#
# Everything is in functions, and the last line calls `main`, so a download
# cut short runs nothing.

set -eu

REPO="toarchkumar/mnemo"

say() {
    printf '%s\n' "$*"
}

die() {
    printf 'mnemo install: %s\n' "$*" >&2
    exit 1
}

have() {
    command -v "$1" >/dev/null 2>&1
}

# The release target for this machine.
target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
        Linux)
            case "$arch" in
                x86_64 | amd64) echo "x86_64-unknown-linux-gnu" ;;
                aarch64 | arm64) echo "aarch64-unknown-linux-gnu" ;;
                *) die "no mnemo build for Linux on $arch (there are x86_64 and aarch64)" ;;
            esac
            ;;
        Darwin)
            case "$arch" in
                x86_64) echo "x86_64-apple-darwin" ;;
                arm64) echo "aarch64-apple-darwin" ;;
                *) die "no mnemo build for macOS on $arch (there are x86_64 and arm64)" ;;
            esac
            ;;
        *)
            die "no mnemo build for $os (there are Linux and macOS builds; on Windows, use install.ps1)"
            ;;
    esac
}

# Download $1 to $2, over HTTPS only.
download() {
    if have curl; then
        curl --proto '=https' --tlsv1.2 -fsSL -o "$2" "$1" || die "couldn't download $1"
    elif have wget; then
        wget --https-only -q -O "$2" "$1" || die "couldn't download $1"
    else
        die "this needs curl or wget to download mnemo"
    fi
}

# The latest release's version, from the redirect of /releases/latest (no
# API, no token).
latest() {
    url="https://github.com/$REPO/releases/latest"
    if have curl; then
        final=$(curl --proto '=https' --tlsv1.2 -fsSLI -o /dev/null -w '%{url_effective}' "$url") ||
            die "couldn't reach $url"
    elif have wget; then
        final=$(wget --https-only -q -O /dev/null --server-response "$url" 2>&1 |
            sed -n 's/^ *[Ll]ocation: *//p' | tail -n 1 | tr -d '\r')
    else
        die "this needs curl or wget to download mnemo"
    fi
    tag=${final##*/}
    echo "${tag#v}"
}

# Whether $1 is a version like 0.16.0.
valid_version() {
    printf '%s\n' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'
}

# The SHA-256 of file $1.
sha256() {
    if have sha256sum; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif have shasum; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        die "this needs sha256sum or shasum to check the download; nothing was installed"
    fi
}

# Check the build's attestation, when gh can.
attest() {
    if have gh && gh auth status >/dev/null 2>&1; then
        gh attestation verify "$1" --repo "$REPO" >/dev/null ||
            die "the build's attestation didn't verify; nothing was installed"
        say "checked the SHA-256 and the build's attestation"
    else
        say "checked the SHA-256 only (install and sign in to \`gh\` to also check the build's attestation)"
    fi
}

main() {
    base=${MNEMO_INSTALL_BASE:-}
    if [ -n "$base" ]; then
        case "$base" in
            /*) ;;
            *) die "MNEMO_INSTALL_BASE must be an absolute path to a local directory: $base" ;;
        esac
        [ -d "$base" ] || die "MNEMO_INSTALL_BASE isn't a directory: $base"
    fi

    tgt=$(target)
    version=${MNEMO_VERSION:-}
    if [ -z "$version" ]; then
        [ -z "$base" ] || die "with MNEMO_INSTALL_BASE, set MNEMO_VERSION too"
        version=$(latest)
    fi
    valid_version "$version" || die "not a version: '$version' (want something like 0.16.0)"

    name="mnemo-v$version-$tgt"
    tmp=$(mktemp -d 2>/dev/null || mktemp -d -t mnemo)
    trap 'rm -rf "$tmp"' EXIT INT TERM

    if [ -n "$base" ]; then
        [ -f "$base/$name" ] || die "no $name in $base"
        [ -f "$base/SHA256SUMS" ] || die "no SHA256SUMS in $base"
        cp "$base/$name" "$tmp/$name"
        cp "$base/SHA256SUMS" "$tmp/SHA256SUMS"
    else
        say "downloading mnemo $version for $tgt"
        url="https://github.com/$REPO/releases/download/v$version"
        download "$url/$name" "$tmp/$name"
        download "$url/SHA256SUMS" "$tmp/SHA256SUMS"
    fi

    want=$(awk -v n="$name" '$2 == n || $2 == "*" n { print $1 }' "$tmp/SHA256SUMS")
    [ -n "$want" ] || die "SHA256SUMS doesn't list $name; nothing was installed"
    got=$(sha256 "$tmp/$name")
    [ "$got" = "$want" ] || die "the SHA-256 doesn't match SHA256SUMS ($got, not $want); nothing was installed"

    if [ -n "$base" ]; then
        say "checked the SHA-256 (a local MNEMO_INSTALL_BASE: the attestation was skipped)"
    else
        attest "$tmp/$name"
    fi

    [ -n "${HOME:-}" ] || die "HOME isn't set"
    dir="$HOME/.local/bin"
    mkdir -p "$dir"
    # Copy beside the target, then one move over the old one.
    cp "$tmp/$name" "$dir/.mnemo.new.$$"
    chmod 755 "$dir/.mnemo.new.$$"
    mv -f "$dir/.mnemo.new.$$" "$dir/mnemo"

    "$dir/mnemo" --version || die "the installed mnemo didn't run"
    say "installed $dir/mnemo"
    case ":${PATH:-}:" in
        *":$dir:"*)
            say "Next: mnemo onboard"
            ;;
        *)
            say "$dir isn't on your PATH. Add it with:"
            say "  export PATH=\"\$HOME/.local/bin:\$PATH\""
            say "Next: $dir/mnemo onboard"
            ;;
    esac
}

main "$@"
