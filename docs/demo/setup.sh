# Sourced by docs/demo/demo.tape before the recording starts (hidden).
#
# A throwaway brain in a temp directory with a test passphrase from the
# environment: never the project brain, never a keychain. Every command in
# the tape then really runs against it, and its output is what the GIF
# shows. The built-in embedder's model is fetched here, off camera.

# Nothing from the recording machine's own mnemo setup.
for v in $(env | sed -n 's/^\(MNEMO_[A-Z_]*\)=.*/\1/p'); do unset "$v"; done
export MNEMO_PASSPHRASE=demo-passphrase-not-a-secret

demo=$(mktemp -d)
cd "$demo" || return 1
mnemo embedder install > /dev/null 2>&1
mnemo init team.mnemo --no-manifest > /dev/null
export PS1='$ '
clear
