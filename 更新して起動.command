#!/bin/bash
set -euo pipefail
cd "$HOME/.local/share/vector-remote-cli/source"
bash scripts/update-and-start.sh
status=$?
echo "Exited: $status"
read -r -p "Enter to close: " _
