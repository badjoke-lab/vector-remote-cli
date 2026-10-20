#!/bin/bash
# launchd entrypoint. Does not update GitHub, compile, or print credentials.
set -euo pipefail
umask 077

ROOT="$HOME/.local/share/vector-remote-cli"
OWNER="$HOME/.config/vector-remote-cli/owner.npub"
DATA="$ROOT/data"
BIN="$HOME/.local/bin/vector-remote-cli"

if [[ ! -s "$OWNER" || ! -s "$DATA/identity.nsec" || ! -x "$BIN" ]]; then
  echo "[START ERROR] Existing owner, bot identity or binary is missing" >&2
  exit 1
fi

export VECTOR_OWNER_NPUB
VECTOR_OWNER_NPUB="$(cat "$OWNER")"
export VECTOR_DATA_DIR="$DATA"
cd "$HOME"
exec "$BIN"
