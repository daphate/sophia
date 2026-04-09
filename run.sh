#!/usr/bin/env bash
# Wrapper script: runs Sophia and restarts on exit code 42 (auto-update).
set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
BIN="$DIR/target/release/sophia"

while true; do
    "$BIN" "$@"
    CODE=$?
    if [ "$CODE" -eq 42 ]; then
        echo "[run.sh] Auto-update applied, restarting..."
        sleep 1
    else
        echo "[run.sh] Sophia exited with code $CODE"
        exit "$CODE"
    fi
done
