#!/bin/bash
# One-time setup after git clone. Never copies, commits or uploads local credentials.
set -euo pipefail
ROOT="$HOME/.local/share/vector-remote-cli"
SRC="$ROOT/source"
OWNER="$HOME/.config/vector-remote-cli/owner.npub"
DATA="$ROOT/data"
LAUNCHER="$HOME/Desktop/Vector Remote CLI.command"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This helper is for macOS."; exit 1
fi
if [[ ! -d "$SRC/.git" ]]; then
  echo "GitHub source not found: $SRC"; exit 1
fi
if [[ ! -s "$OWNER" || ! -s "$DATA/identity.nsec" ]]; then
  echo "Existing Bot settings are missing; stopping to avoid replacing its identity."
  exit 1
fi
if [[ -e "$LAUNCHER" ]]; then
  echo "Launcher already exists: $LAUNCHER"
  echo "It has not been overwritten."
else
  cp "$SRC/更新して起動.command" "$LAUNCHER"
  chmod 700 "$LAUNCHER"
  echo "Created Desktop launcher: $LAUNCHER"
fi
echo "Existing owner settings and identity.nsec stay on your Mac."
echo "Updating and starting your Bot now..."
exec bash "$SRC/scripts/update-and-start.sh"
