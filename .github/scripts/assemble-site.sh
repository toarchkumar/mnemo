#!/bin/sh
# The site GitHub Pages publishes (decision.site-publish, site-1), assembled
# from exactly this list and nothing else:
#   - the root index.html (it moves into site/ later, at the same address);
#   - docs/img/ (the page loads docs/img/demo.gif and inspect-findings.png);
#   - the files whose addresses must not change: install.sh, install.ps1,
#     llms.txt, llms-full.txt, .claude-plugin/marketplace.json;
#   - everything in site/, at the site's root (the sitemap, the social card).
# A listed file that's missing, or a site/ file that would replace one of the
# others, fails the run.
#
#   sh .github/scripts/assemble-site.sh <out-dir>          assemble, then check
#   sh .github/scripts/assemble-site.sh --check <out-dir>  check only
#
# Run from the repository root.
set -eu

# Every address that must keep working, relative to the site's root.
PUBLISHED="index.html install.sh install.ps1 llms.txt llms-full.txt
.claude-plugin/marketplace.json docs/img/demo.gif docs/img/inspect-findings.png
sitemap.xml social-card.png"

die() {
  echo "assemble-site: $*" >&2
  exit 1
}

# copy <file> <out-dir>: one listed file, to the same path in the site.
copy() {
  [ -f "$1" ] || die "$1 is listed but missing"
  mkdir -p "$2/$(dirname "$1")"
  cp "$1" "$2/$1"
}

assemble() {
  out=$1
  [ ! -e "$out" ] || die "$out already exists"
  [ -d docs/img ] || die "docs/img is listed but missing"
  [ -d site ] || die "site/ is listed but missing"
  mkdir -p "$out"
  for f in index.html install.sh install.ps1 llms.txt llms-full.txt \
    .claude-plugin/marketplace.json; do
    copy "$f" "$out"
  done
  find docs/img -type f | while IFS= read -r f; do
    copy "$f" "$out"
  done
  # site/ at the root, never over a file placed above.
  (cd site && find . -type f) | while IFS= read -r f; do
    f=${f#./}
    [ ! -e "$out/$f" ] || die "site/$f would replace $f"
    mkdir -p "$out/$(dirname "$f")"
    cp "site/$f" "$out/$f"
  done
}

check() {
  out=$1
  [ -d "$out" ] || die "$out isn't a directory"
  missing=0
  for f in $PUBLISHED; do
    if [ -s "$out/$f" ]; then
      echo "ok  /$f"
    else
      echo "MISSING  /$f" >&2
      missing=1
    fi
  done
  [ "$missing" = 0 ] || die "a published address would stop working"
}

main() {
  case "${1:-}" in
    --check)
      [ $# -eq 2 ] || die "usage: assemble-site.sh --check <out-dir>"
      check "$2"
      ;;
    "" | -*)
      die "usage: assemble-site.sh <out-dir> | --check <out-dir>"
      ;;
    *)
      [ $# -eq 1 ] || die "usage: assemble-site.sh <out-dir>"
      assemble "$1"
      check "$1"
      ;;
  esac
}

main "$@"
