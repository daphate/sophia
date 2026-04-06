#!/bin/bash
set -e

REPO_URL="https://github.com/daphate/sophia-proxy.git"
INSTALL_DIR="${SOPHIA_INSTALL_DIR:-$HOME/sophia-proxy}"

echo "=== Sophia Proxy — macOS installer ==="

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
    elif command -v brew &>/dev/null; then
        echo "Installing via brew..."
        brew install claude-code
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

# Install LaunchAgent
PLIST="$HOME/Library/LaunchAgents/com.sophia.proxy.plist"
mkdir -p "$HOME/Library/LaunchAgents"

cat > "$PLIST" <<PLISTEOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.sophia.proxy</string>
    <key>ProgramArguments</key>
    <array>
        <string>$BINARY</string>
    </array>
    <key>WorkingDirectory</key>
    <string>$PROXY_DIR</string>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/sophia-proxy.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/sophia-proxy.err</string>
</dict>
</plist>
PLISTEOF

echo "Installed LaunchAgent: $PLIST"

# Load service
launchctl bootout gui/$(id -u) "$PLIST" 2>/dev/null || true
launchctl bootstrap gui/$(id -u) "$PLIST"

echo ""
echo "=== Done! ==="
echo "Sophia proxy is running on http://127.0.0.1:8080"
echo ""
echo "Commands:"
echo "  Stop:    launchctl bootout gui/\$(id -u) $PLIST"
echo "  Start:   launchctl bootstrap gui/\$(id -u) $PLIST"
echo "  Logs:    tail -f /tmp/sophia-proxy.log"
echo "  Test:    curl http://127.0.0.1:8080/v1/models"
