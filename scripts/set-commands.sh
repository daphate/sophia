#!/usr/bin/env bash
# set-commands.sh — Set BotFather command descriptions for both bots via Telegram Bot API.
# Reads BOT_TOKEN and RESCUE_BOT_TOKEN from .env in the project root.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
ENV_FILE="$PROJECT_DIR/.env"

if [ ! -f "$ENV_FILE" ]; then
    echo "Error: .env file not found at $ENV_FILE"
    exit 1
fi

# Source .env (handle lines with export prefix and skip comments)
set -a
# shellcheck disable=SC1090
source <(grep -v '^\s*#' "$ENV_FILE" | grep -v '^\s*$' | sed 's/^export //')
set +a

COMMANDS='{"commands":[
  {"command":"pair","description":"Request access to talk with me"},
  {"command":"help","description":"Show available commands"},
  {"command":"memory","description":"View or manage memory"},
  {"command":"search","description":"Search conversation history"},
  {"command":"exec","description":"Run a system command"},
  {"command":"update","description":"Check and install updates"},
  {"command":"status","description":"Check peer bot status"},
  {"command":"restart","description":"Restart peer bot"},
  {"command":"logs","description":"View recent log files"},
  {"command":"ping","description":"Alive check with uptime"}
]}'

set_commands() {
    local token="$1"
    local label="$2"

    if [ -z "$token" ]; then
        echo "[$label] Token not set, skipping."
        return
    fi

    local url="https://api.telegram.org/bot${token}/setMyCommands"
    local response
    response=$(curl -s -X POST "$url" \
        -H "Content-Type: application/json" \
        -d "$COMMANDS")

    if echo "$response" | grep -q '"ok":true'; then
        echo "[$label] Commands set successfully."
    else
        echo "[$label] Failed to set commands: $response"
        return 1
    fi
}

echo "Setting BotFather commands..."
echo

set_commands "${BOT_TOKEN:-}" "Main bot"
set_commands "${RESCUE_BOT_TOKEN:-}" "Rescue bot"

echo
echo "Done."
