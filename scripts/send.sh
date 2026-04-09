#!/bin/bash
# Send a message via sophia-bot outbox.
# Usage: send.sh <chat_id> "message text"
set -e

OUTBOX_DIR="$(cd "$(dirname "$0")/.." && pwd)/data/outbox"
mkdir -p "$OUTBOX_DIR"

CHAT_ID="$1"
TEXT="$2"

if [ -z "$CHAT_ID" ] || [ -z "$TEXT" ]; then
    echo "Usage: $0 <chat_id> \"message text\""
    exit 1
fi

FILE="$OUTBOX_DIR/$(date +%s%N).json"
cat > "$FILE" <<EOF
{"chat_id": $CHAT_ID, "text": $(printf '%s' "$TEXT" | python3 -c 'import sys,json; print(json.dumps(sys.stdin.read()))')}
EOF

echo "Queued message to $CHAT_ID → $FILE"
