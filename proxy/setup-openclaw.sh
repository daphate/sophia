#!/bin/bash
set -e

# Sophia Proxy — OpenClaw integration setup (Anthropic Messages API)
# Configures OpenClaw to use sophia-proxy as the default model provider.

PROXY_HOST="${SOPHIA_PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${SOPHIA_PROXY_PORT:-8080}"
PROXY_URL="http://${PROXY_HOST}:${PROXY_PORT}/v1"
MODEL_ID="secondf8n/sophia"
PROVIDER_NAME="sophia-proxy"

echo "=== Sophia Proxy — OpenClaw setup ==="
echo "Proxy URL: $PROXY_URL"
echo "Model:     $PROVIDER_NAME/$MODEL_ID"
echo ""

# Check proxy is reachable
if ! curl -sf "$PROXY_URL/models" >/dev/null 2>&1; then
    echo "ERROR: Sophia proxy is not reachable at $PROXY_URL"
    echo "Make sure sophia-proxy is running first."
    exit 1
fi
echo "Proxy is reachable."

# Find openclaw config
OPENCLAW_CONFIG="${OPENCLAW_CONFIG:-$HOME/.openclaw/openclaw.json}"
if [ ! -f "$OPENCLAW_CONFIG" ]; then
    echo "ERROR: OpenClaw config not found at $OPENCLAW_CONFIG"
    echo "Set OPENCLAW_CONFIG env var to specify a different path."
    exit 1
fi
echo "OpenClaw config: $OPENCLAW_CONFIG"

# Backup
BACKUP="${OPENCLAW_CONFIG}.backup.$(date +%Y%m%d%H%M%S)"
cp "$OPENCLAW_CONFIG" "$BACKUP"
echo "Backup: $BACKUP"

# Check for jq
if ! command -v jq &>/dev/null; then
    echo "ERROR: jq is required. Install it:"
    echo "  macOS:   brew install jq"
    echo "  Ubuntu:  sudo apt install jq"
    echo "  Windows: choco install jq"
    exit 1
fi

# Patch config: add provider and set as primary model
PATCHED=$(jq \
    --arg url "$PROXY_URL" \
    --arg model_id "$MODEL_ID" \
    --arg provider "$PROVIDER_NAME" \
    '
    # Add sophia-proxy provider under models.providers
    .models.providers[$provider] = {
        "baseUrl": $url,
        "apiKey": "sk-sophia-local",
        "api": "anthropic-messages",
        "models": [
            {
                "id": $model_id,
                "name": "Sophia (Claude via CLI proxy)",
                "contextWindow": 1000000,
                "maxTokens": 64000
            }
        ]
    }
    # Set as primary model
    | .agents.defaults.model.primary = ($provider + "/" + $model_id)
    ' "$OPENCLAW_CONFIG")

echo "$PATCHED" > "$OPENCLAW_CONFIG"

echo ""
echo "=== Done! ==="
echo ""
echo "OpenClaw is now configured to use sophia-proxy."
echo "Primary model: $PROVIDER_NAME/$MODEL_ID"
echo ""
echo "Restart OpenClaw to apply:"
echo "  systemctl --user restart openclaw-gateway"
echo "  # or: openclaw gateway restart"
echo ""
echo "To revert:"
echo "  cp $BACKUP $OPENCLAW_CONFIG"
