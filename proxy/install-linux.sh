#!/bin/bash
set -e

REPO_URL="https://github.com/daphate/sophia-proxy.git"
INSTALL_DIR="${SOPHIA_INSTALL_DIR:-$HOME/sophia-proxy}"

echo "=== Sophia Proxy — Linux installer ==="

# Check Rust
if ! command -v cargo &>/dev/null; then
    echo "Rust not found. Installing via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
fi

# Check Claude CLI
if ! command -v claude &>/dev/null; then
    echo "Claude CLI not found."
    if command -v npm &>/dev/null; then
        echo "Installing via npm..."
        npm install -g @anthropic-ai/claude-code
    else
        echo "ERROR: Install Claude CLI manually: npm install -g @anthropic-ai/claude-code"
        exit 1
    fi
fi

# Clone or update repo
if [ -d "$INSTALL_DIR/proxy/src" ]; then
    echo "Updating existing installation at $INSTALL_DIR..."
    git -C "$INSTALL_DIR" pull --ff-only
else
    echo "Cloning repository to $INSTALL_DIR..."
    git clone "$REPO_URL" "$INSTALL_DIR"
fi

# Build
PROXY_DIR="$INSTALL_DIR/proxy"
echo "Building sophia-proxy..."
cd "$PROXY_DIR"
cargo build --release

BINARY="$PROXY_DIR/target/release/sophia-proxy"
echo "Built: $BINARY"

# Create .env if missing
CLAUDE_PATH=$(which claude 2>/dev/null || echo "claude")
if [ ! -f "$PROXY_DIR/.env" ]; then
    cat > "$PROXY_DIR/.env" <<ENVEOF
PROXY_HOST=127.0.0.1
PROXY_PORT=8080
CLAUDE_CLI=$CLAUDE_PATH
MODEL_NAME=claude-opus-4-6
INFERENCE_TIMEOUT=300
ENVEOF
    echo "Created .env (CLAUDE_CLI=$CLAUDE_PATH). Edit as needed."
fi

# Install systemd service
SERVICE_FILE="/etc/systemd/system/sophia-proxy.service"
CLAUDE_PATH=$(which claude 2>/dev/null || echo "/usr/local/bin/claude")

cat > /tmp/sophia-proxy.service <<SVCEOF
[Unit]
Description=Sophia Anthropic-compatible proxy (Claude CLI)
After=network.target

[Service]
Type=simple
User=$USER
WorkingDirectory=$PROXY_DIR
Environment=PATH=$(dirname "$CLAUDE_PATH"):/usr/local/bin:/usr/bin:/bin
ExecStart=$BINARY
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
SVCEOF

if command -v sudo &>/dev/null; then
    sudo cp /tmp/sophia-proxy.service "$SERVICE_FILE"
    sudo systemctl daemon-reload
    sudo systemctl enable sophia-proxy
    sudo systemctl start sophia-proxy
    echo "Systemd service installed and started."
else
    echo "sudo not available — service file written to /tmp/sophia-proxy.service"
    echo "Copy it to /etc/systemd/system/ manually."
fi

rm -f /tmp/sophia-proxy.service

echo ""
echo "=== Done! ==="
echo "Sophia proxy is running on http://127.0.0.1:8080"
echo ""
echo "Commands:"
echo "  Stop:    sudo systemctl stop sophia-proxy"
echo "  Start:   sudo systemctl start sophia-proxy"
echo "  Restart: sudo systemctl restart sophia-proxy"
echo "  Logs:    journalctl -u sophia-proxy -f"
echo "  Test:    curl http://127.0.0.1:8080/v1/models"
