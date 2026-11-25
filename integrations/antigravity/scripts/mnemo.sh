#!/bin/sh
# The plugin's launcher: it runs the mnemo you installed, and carries no
# binary of its own. It tries $MNEMO_BIN, then mnemo on the PATH, then the
# install script's and cargo's directories, which an app started from the
# Dock or a desktop menu may not have on its PATH. Without one it says how
# to install mnemo; for a hook it then exits 0, so a session never fails
# because mnemo is missing.
#
# The same file is in each plugin (integrations/*/scripts/mnemo.sh); a test
# checks the copies match.
for m in "${MNEMO_BIN:-}" "$(command -v mnemo 2>/dev/null)" \
  "$HOME/.local/bin/mnemo" "$HOME/.cargo/bin/mnemo"; do
  if [ -n "$m" ] && [ -x "$m" ] && [ ! -d "$m" ]; then
    exec "$m" "$@"
  fi
done
cat >&2 <<'EOF'
mnemo: the mnemo command isn't installed, so this plugin can't run it.
Install it, then restart the session:
  curl -fsSL https://toarchkumar.github.io/mnemo/install.sh | sh
or from source: cargo install mnemo-engine --features keychain,embed-candle
More: https://toarchkumar.github.io/mnemo/
EOF
if [ "${1:-}" = hook ]; then
  exit 0
fi
exit 127
