#!/bin/bash
# Opt-in macOS user LaunchAgent management. No root, no secrets in GitHub/plist.
set -euo pipefail
umask 077

LABEL="com.badjoke-lab.vector-remote-cli"
ROOT="$HOME/.local/share/vector-remote-cli"
SRC="$ROOT/source"
BIN="$HOME/.local/bin/vector-remote-cli"
OWNER="$HOME/.config/vector-remote-cli/owner.npub"
DATA="$ROOT/data"
PLIST_DIR="$HOME/Library/LaunchAgents"
PLIST="$PLIST_DIR/$LABEL.plist"
LOG_DIR="$ROOT/logs"
DOMAIN="gui/$(id -u)"
TARGET="$DOMAIN/$LABEL"
MODE="${1:-}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS only"; exit 1
fi

escape_xml() {
  local s="$1"
  s="${s//&/&amp;}"
  s="${s//</&lt;}"
  s="${s//>/&gt;}"
  s="${s//\"/&quot;}"
  printf '%s' "$s"
}

is_loaded() { launchctl print "$TARGET" >/dev/null 2>&1; }
check_existing() {
  if [[ ! -s "$OWNER" || ! -s "$DATA/identity.nsec" || ! -x "$BIN" ]]; then
    echo "Existing bot owner settings, identity or binary missing; refusing to continue."
    exit 1
  fi
}
ensure_paths() {
  mkdir -p "$PLIST_DIR" "$LOG_DIR"
  chmod 700 "$LOG_DIR"
  touch "$LOG_DIR/bot.stdout.log" "$LOG_DIR/bot.stderr.log"
  chmod 600 "$LOG_DIR/bot.stdout.log" "$LOG_DIR/bot.stderr.log"
}
make_plist() {
  ensure_paths
  local script_path
  script_path="$(escape_xml "$SRC/scripts/run-bot.sh")"
  local work_dir
  work_dir="$(escape_xml "$HOME")"
  local stdout_path
  stdout_path="$(escape_xml "$LOG_DIR/bot.stdout.log")"
  local stderr_path
  stderr_path="$(escape_xml "$LOG_DIR/bot.stderr.log")"
  cat > "$PLIST" <<PLIST_XML
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key>
  <array><string>/bin/bash</string><string>$script_path</string></array>
  <key>WorkingDirectory</key><string>$work_dir</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>30</integer>
  <key>StandardOutPath</key><string>$stdout_path</string>
  <key>StandardErrorPath</key><string>$stderr_path</string>
</dict>
</plist>
PLIST_XML
  chmod 600 "$PLIST"
  plutil -lint "$PLIST" >/dev/null
}
bootout_if_loaded() {
  if is_loaded; then
    launchctl bootout "$DOMAIN" "$PLIST"
  fi
}
bootstrap() {
  launchctl bootstrap "$DOMAIN" "$PLIST"
  echo "Bot service enabled for this Mac user (login and automatic restart)."
  echo "The Mac must remain powered on and awake for remote commands."
}
case "$MODE" in
  enable)
    check_existing
    if [[ ! -d "$SRC/.git" ]]; then echo "GitHub source missing: $SRC"; exit 1; fi
    echo "Stop any existing foreground Bot with Control+C before enabling."
    if is_loaded; then echo "Already enabled. Use update or status."; exit 0; fi
    make_plist
    bootstrap
    ;;
  disable)
    bootout_if_loaded
    if [[ -f "$PLIST" ]]; then rm "$PLIST"; fi
    echo "Bot LaunchAgent disabled; owner settings and identity preserved."
    ;;
  update)
    if [[ ! -f "$PLIST" ]]; then
      echo "Not enabled. Use: bash scripts/manage-mac.sh enable"
      exit 1
    fi
    check_existing
    bootout_if_loaded
    # Restart the previous executable even when GitHub fetch/build fails.
    on_exit() {
      local code=$?
      if [[ -f "$PLIST" ]]; then
        if launchctl bootstrap "$DOMAIN" "$PLIST"; then
          echo "Bot service restarted by launchd."
        else
          echo "Unable to restart Bot service; run manage-mac.sh status" >&2
        fi
      fi
      exit "$code"
    }
    trap on_exit EXIT
    bash "$SRC/scripts/update-and-start.sh" --build-only
    echo "GitHub update installed; restarting Bot service..."
    ;;
  status)
    if is_loaded; then
      echo "Service loaded: $LABEL"
      launchctl print "$TARGET" | grep -E 'state =|pid =|last exit code =' || true
    else
      echo "Bot service is not loaded."
    fi
    echo "Private service logs: $LOG_DIR"
    ;;
  *)
    echo "Usage: bash scripts/manage-mac.sh {enable|disable|update|status}"
    exit 2
    ;;
esac
