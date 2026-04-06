# Sophia v1.0-beta (2026-04-08)

Telegram userbot with Claude CLI backend + Anthropic-compatible HTTP proxy.

> **Disclaimer:** This project is provided strictly for educational and research purposes only. It demonstrates how, in theory, one could build an Anthropic-compatible proxy over Claude CLI. The authors do not encourage using this in production or in any way that may violate Anthropic's Terms of Service. Use at your own risk and responsibility.

## Why

Use [OpenClaw](https://openclaw.dev) (or any Anthropic-compatible client) with your own Claude Pro/Max subscription — no API keys needed. Sophia Proxy wraps Claude CLI into a standard `/v1/messages` endpoint (Anthropic Messages API), so you can run a self-hosted AI stack on a small VPS or locally on your Mac/PC, powered by the same Claude you already pay for.

## Components

This repo contains two independent components. Install either or both:

| Component | Directory | What it does |
|---|---|---|
| **Sophia Proxy** | `proxy/` | Anthropic-compatible HTTP proxy over Claude CLI |
| **Sophia Bot** | `bot/` | Telegram userbot with memory, pairing, command execution |

---

# Sophia Proxy

Translates Anthropic Messages API calls (`/v1/messages`) into Claude CLI subprocess invocations. Supports streaming (SSE with incremental deltas) and non-streaming modes. Passes images inline as base64. Stateless — no sessions, no files on disk.

## Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [Claude CLI](https://docs.anthropic.com/en/docs/claude-code) — installed and authenticated (`claude login`)

## Quick install

**Linux** (builds + systemd service):
```bash
curl -sSL https://raw.githubusercontent.com/daphate/sophia-proxy/main/proxy/install-linux.sh | bash
```

**macOS** (builds + LaunchAgent):
```bash
curl -sSL https://raw.githubusercontent.com/daphate/sophia-proxy/main/proxy/install-mac.sh | bash
```

**Windows** (PowerShell, builds + optional service):
```powershell
irm https://raw.githubusercontent.com/daphate/sophia-proxy/main/proxy/install-windows.ps1 | iex
```

## Manual install

```bash
git clone https://github.com/daphate/sophia-proxy.git
cd sophia-proxy/proxy
cargo build --release
cp .env.example .env   # edit as needed
./target/release/sophia-proxy
```

Test:
```bash
curl http://127.0.0.1:8080/v1/models
curl http://127.0.0.1:8080/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: sk-sophia-local" \
  -d '{"model":"claude-opus-4-6","max_tokens":1024,"messages":[{"role":"user","content":"Hello"}]}'
```

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `PROXY_HOST` | `127.0.0.1` | Bind address |
| `PROXY_PORT` | `8080` | Bind port |
| `CLAUDE_CLI` | `claude` | Path to Claude CLI binary |
| `MODEL_NAME` | `claude-opus-4-6` | Model name reported to clients |
| `INFERENCE_TIMEOUT` | _(none)_ | Request timeout in seconds |
| `MAX_TURNS` | _(none)_ | Max conversation turns per request |
| `RUST_LOG` | `info` | Log level (`debug`, `info`, `error`) |

## Service management

**Linux (systemd):**
```bash
sudo systemctl start sophia-proxy
sudo systemctl stop sophia-proxy
sudo systemctl restart sophia-proxy
journalctl -u sophia-proxy -f
```

**macOS (LaunchAgent):**
```bash
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.sophia.proxy.plist    # stop
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.sophia.proxy.plist  # start
tail -f /tmp/sophia-proxy.log
```

**Windows (Admin):**
```powershell
sc.exe stop SophiaProxy
sc.exe start SophiaProxy
sc.exe delete SophiaProxy
```

## OpenClaw Integration

Configure [OpenClaw](https://openclaw.dev) to route all LLM requests through sophia-proxy using the `secondf8n/sophia` model.

**Linux / macOS:**
```bash
cd sophia-proxy/proxy && ./setup-openclaw.sh
```

**Windows:**
```powershell
cd sophia-proxy\proxy; powershell -ExecutionPolicy Bypass -File setup-openclaw.ps1
```

The script backs up your config, adds `sophia-proxy` as a provider, and sets it as the primary model. To configure manually, add to `~/.openclaw/openclaw.json`:

```json
{
  "models": {
    "providers": {
      "sophia-proxy": {
        "baseUrl": "http://127.0.0.1:8080/v1",
        "apiKey": "sk-sophia-local",
        "api": "anthropic-messages",
        "models": [
          {
            "id": "secondf8n/sophia",
            "name": "Sophia (Claude via CLI proxy)",
            "contextWindow": 1000000,
            "maxTokens": 64000
          }
        ]
      }
    }
  },
  "agents": {
    "defaults": {
      "model": {
        "primary": "sophia-proxy/secondf8n/sophia"
      }
    }
  }
}
```

For remote servers, set `SOPHIA_PROXY_HOST` and `SOPHIA_PROXY_PORT` before running the setup script.

---

# Sophia Bot (Telegram)

Telegram userbot powered by Claude CLI. Features: message queue with batching, persistent memory, user pairing, OS command execution.

## Prerequisites

- Python 3.11+
- Telegram API credentials (`api_id`, `api_hash` from https://my.telegram.org)
- Claude CLI installed and authenticated

## Install

```bash
git clone https://github.com/daphate/sophia-proxy.git
cd sophia-proxy
pip install -r requirements.txt

cat > .env <<'EOF'
API_ID=your_api_id
API_HASH=your_api_hash
PHONE_NUMBER=+1234567890
OWNER_ID=your_telegram_id
CLAUDE_CLI=claude
EOF

python3 start.py
```

**Systemd service (Linux):**
```bash
sudo cp sophia-bot.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now sophia-bot
```

## Commands

| Command | Access | Description |
|---|---|---|
| `/pair` | Anyone | Request access |
| `/approve <id>` | Owner | Approve pairing |
| `/deny <id>` | Owner | Deny pairing |
| `/unpair <id>` | Owner | Remove user |
| `/exec <cmd>` | Owner | Run OS command |
| `/memory` | Owner | View memory |
| `/memory add <text>` | Owner | Add to memory |
| `/memory clear` | Owner | Clear memory |
| `/queue` | Owner | Queue status |
| `/help` | Paired | Show help |
