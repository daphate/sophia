#!/usr/bin/env bash
set -euo pipefail

REPO="https://github.com/daphate/sophia.git"
DIR="sophia"

echo "=== Sophia Bot Installer ==="

# Check Rust
if ! command -v cargo &>/dev/null; then
    echo "Rust not found. Installing via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
fi

# Clone or update
if [ -d "$DIR" ]; then
    echo "Directory '$DIR' exists, pulling latest..."
    cd "$DIR" && git pull
else
    echo "Cloning repository..."
    git clone "$REPO"
    cd "$DIR"
fi

# Build
echo "Building (release)..."
cargo build --release

# Config
if [ ! -f .env ]; then
    cp .env.example .env
    echo ""
    echo "Created .env from template. Edit it with your credentials:"
    echo "  nano $PWD/.env"
fi

echo ""
echo "Done! Run with:"
echo "  cd $PWD && ./target/release/sophia"
