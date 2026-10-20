#!/bin/bash
# GitHub-based updater for an existing Mac installation. Never alters Bot keys.
set -euo pipefail

REPO="$HOME/.local/share/vector-remote-cli/source"
BIN_DIR="$HOME/.local/bin"
BIN="$BIN_DIR/vector-remote-cli"
DATA="$HOME/.local/share/vector-remote-cli/data"
OWNER="$HOME/.config/vector-remote-cli/owner.npub"
CACHE="$HOME/.local/share/vector-remote-cli/target"

echo "=== Vector Remote CLI: update and start ==="
if [[ "$(uname -s)" != "Darwin" && "$(uname -s)" != "Linux" ]]; then
  echo "macOS or Linux only"; exit 1
fi
if [[ ! -s "$OWNER" ]]; then
  echo "Owner configuration missing: $OWNER"; exit 1
fi
if [[ ! -d "$REPO/.git" ]]; then
  echo "Source repository not found: $REPO"; exit 1
fi
mkdir -p "$BIN_DIR" "$DATA" "$CACHE"
chmod 700 "$DATA"
cd "$REPO"

if [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
  echo "Local source changes found: update aborted to avoid overwriting them."; exit 1
fi
echo "Checking GitHub updates..."
git fetch origin main
git checkout main
git merge --ff-only origin/main

export PATH="$HOME/.cargo/bin:$PATH"
if [[ -f "$HOME/.cargo/env" ]]; then
  # shellcheck source=/dev/null
  source "$HOME/.cargo/env"
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust/Cargo missing. Run the original Mac installer once."; exit 1
fi
export RUSTUP_TOOLCHAIN=stable
export CARGO_TARGET_DIR="$CACHE"
export CARGO_BUILD_JOBS=2

echo "Building updated CLI (first time may take a while)..."
cargo build --release

NEW_BIN="$CACHE/release/vector-remote-cli"
if [[ ! -x "$NEW_BIN" ]]; then
  echo "Build finished without executable: $NEW_BIN"; exit 1
fi
install -m 700 "$NEW_BIN" "$BIN.next"
if [[ -f "$BIN" ]]; then
  cp "$BIN" "$BIN.previous"
fi
mv "$BIN.next" "$BIN"

export VECTOR_OWNER_NPUB
VECTOR_OWNER_NPUB="$(cat "$OWNER")"
export VECTOR_DATA_DIR="$DATA"

echo "Build installed. Persistent keys and settings have NOT been changed."
echo "Bot will print [READY] after its subscription is registered."
echo "Send 'connect' (no slash), then 'pwd' in the existing Vector DM."
echo "To stop the Bot: press Control+C in this Terminal."
echo ""
exec "$BIN"
