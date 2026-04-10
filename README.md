# Sophia Bot v1.0-beta

> Release date: 2026-04-09

Telegram bot powered by Claude CLI. Written in Rust. Ships two binaries (main + rescue) that share a common library crate.

Features: persistent memory, semantic search over conversation history, user pairing, OS command execution, per-user dialog history, proactive messaging, automatic update notifications.

## Quick install

**Linux / macOS:**

```bash
curl -fsSL https://raw.githubusercontent.com/daphate/sophia/main/install.sh | bash
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/daphate/sophia/main/install.ps1 | iex
```

### Prerequisites

- [Rust](https://rustup.rs/) (stable, 1.75+)
- Telegram API credentials (`api_id`, `api_hash` from https://my.telegram.org)
- [Claude CLI](https://docs.anthropic.com/en/docs/claude-code) installed and authenticated (`claude login`)

## Manual install

### 1. Clone and build

```bash
git clone https://github.com/daphate/sophia.git
cd sophia
cargo build --release
```

### 2. Configure

```bash
cp .env.example .env
```

Edit `.env` with your values:

```env
API_ID=your_api_id
API_HASH=your_api_hash
# Use BOT_TOKEN for regular bot mode, or PHONE_NUMBER for userbot mode
BOT_TOKEN=123456:ABC-DEF
# PHONE_NUMBER=+1234567890
OWNER_ID=your_telegram_id
CLAUDE_CLI=claude
INFERENCE_TIMEOUT=120
SESSION_NAME=sophia
EXEC_ENABLED=true
EXEC_ALLOWED_COMMANDS=cat,echo,ls,pwd,date,whoami,uname,head,tail,wc,df,free,uptime,tee
UPDATE_CHECK_HOURS=12
```

| Variable | Description |
|---|---|
| `API_ID` | Telegram API ID from https://my.telegram.org |
| `API_HASH` | Telegram API hash |
| `BOT_TOKEN` | Bot token from [@BotFather](https://t.me/BotFather) (bot mode) |
| `PHONE_NUMBER` | Phone number (userbot mode). Either `BOT_TOKEN` or `PHONE_NUMBER` is required |
| `OWNER_ID` | Your Telegram user ID (numeric). The bot treats this user as admin |
| `CLAUDE_CLI` | Path to Claude CLI binary (default: `claude`) |
| `INFERENCE_TIMEOUT` | Max seconds to wait for Claude response (default: `120`) |
| `SESSION_NAME` | Telegram session file name (default: `sophia`) |
| `EXEC_ENABLED` | Enable `/exec` command (default: `true`) |
| `EXEC_ALLOWED_COMMANDS` | Comma-separated whitelist of allowed OS commands |
| `UPDATE_CHECK_HOURS` | How often to check for updates, in hours. `0` = disabled (default: `12`) |
| `AUTO_UPDATE` | Auto-pull, rebuild and restart on new version (default: `false`) |
| `RESCUE_BOT_TOKEN` | Bot token for the rescue/watchdog bot (see [Rescue Bot](#rescue-bot)) |

### 3. First run

```bash
./target/release/sophia
```

On first run you will be prompted for:
1. Telegram login code (sent to your Telegram app)
2. 2FA password (if enabled)

After authentication the session is saved and subsequent runs won't ask again.

### 4. Update

Sophia checks for updates automatically (every 12 hours by default).

**Manual update:**

```bash
cd sophia
git pull
cargo build --release
```

Then restart the bot (see below).

**Automatic update:** set `AUTO_UPDATE=true` in `.env`. When a new release is found, the bot will pull, rebuild and restart itself (exit code 42 triggers restart by the service manager).

### 5. Restart

**systemd (Linux):**

```bash
sudo systemctl restart sophia-bot
```

**launchd (macOS):**

```bash
# Restart main bot
launchctl kickstart -k gui/$(id -u)/com.sophia.bot
# Restart rescue bot
launchctl kickstart -k gui/$(id -u)/com.sophia.rescue
```

**Windows (NSSM):**

```powershell
nssm restart Sophia
```

**Standalone (no service manager):** use the wrapper script:

```bash
./run.sh
```

### 6. Debug mode

```bash
./target/release/sophia --debug
```

Logs all raw Telegram updates for troubleshooting.

## Platform-specific setup

### Linux (systemd)

```bash
sudo cp sophia-bot.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now sophia-bot
```

Edit `sophia-bot.service` to match your paths:

```ini
[Service]
WorkingDirectory=/path/to/sophia
ExecStart=/path/to/sophia/target/release/sophia
User=your_user
```

### macOS (launchd)

Create `~/Library/LaunchAgents/com.sophia.bot.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.sophia.bot</string>
    <key>ProgramArguments</key>
    <array>
        <string>/path/to/sophia/target/release/sophia</string>
    </array>
    <key>WorkingDirectory</key>
    <string>/path/to/sophia</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/sophia.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/sophia.err</string>
</dict>
</plist>
```

```bash
launchctl load ~/Library/LaunchAgents/com.sophia.bot.plist
```

For the rescue bot, create `~/Library/LaunchAgents/com.sophia.rescue.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.sophia.rescue</string>
    <key>ProgramArguments</key>
    <array>
        <string>/path/to/sophia/target/release/sophia-rescue</string>
    </array>
    <key>WorkingDirectory</key>
    <string>/path/to/sophia</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/sophia-rescue.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/sophia-rescue.err</string>
</dict>
</plist>
```

```bash
launchctl load ~/Library/LaunchAgents/com.sophia.rescue.plist
```

### Windows

Run in PowerShell:

```powershell
git clone https://github.com/daphate/sophia.git
cd sophia
cargo build --release
copy .env.example .env
# Edit .env with your values
.\target\release\sophia.exe
```

To run as a background service, use [NSSM](https://nssm.cc/):

```powershell
nssm install Sophia "C:\path\to\sophia\target\release\sophia.exe"
nssm set Sophia AppDirectory "C:\path\to\sophia"
nssm start Sophia
```

## Commands

Both bots (main and rescue) support the same command set:

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
| `/update` | Owner | Check and install updates |
| `/search <query>` | Owner | Search conversation history |
| `/reindex` | Owner | Rebuild semantic search index |
| `/status` | Owner | Check peer bot status |
| `/restart` | Owner | Restart peer bot |
| `/logs` | Owner | View recent log files |
| `/ping` | Paired | Alive check with uptime |
| `/help` | Paired | Show help |

## Rescue Bot

`sophia-rescue` is a companion bot that shares the same library crate (`src/lib.rs`) as the main bot. Both binaries have full conversational capability and the same command set; the difference is which peer they monitor and restart.

The rescue bot runs as its own launchd/systemd service and acts as a watchdog for the main bot (and vice versa).

**Setup:**

1. Create a second bot via [@BotFather](https://t.me/BotFather).
2. Set `RESCUE_BOT_TOKEN` in your `.env`.
3. Build: `cargo build --release` (builds both binaries).
4. Set up BotFather commands for both bots: `./scripts/set-commands.sh`
5. Install the launchd service (see [macOS (launchd)](#macos-launchd)).

## Vector Store

The bot uses [fastembed](https://github.com/Anush008/fastembed-rs) with the `multilingual-e5-small` model (384 dimensions) and [usearch](https://github.com/unum-cloud/usearch) for semantic search over conversation history.

Each incoming message is embedded and stored in `data/vecstore.usearch`. When building context for a response, the bot retrieves the most relevant past conversations, enabling context-aware replies even across long time spans.

## Message Queue

A SQLite-based message queue (`queue.db`) handles deduplication and reliable message processing. Incoming Telegram updates are enqueued before processing, which prevents duplicate handling on restarts and ensures no messages are lost.

## Outbox

Proactive messaging is supported via the outbox mechanism. Drop a JSON file into `data/outbox/` (or use `scripts/send.sh`) to send a message from the bot without an incoming trigger.

```bash
# Send a message to a specific chat
./scripts/send.sh <chat_id> "Hello from the outbox"
```

The bot watches the `data/outbox/` directory and processes any `*.json` files it finds.

## Architecture

The project is a Cargo workspace with a shared library crate (`src/lib.rs`) and two binary crates (`src/main.rs` for the main bot, `sophia-rescue/` for the rescue bot). Both binaries link against the same library, so all core logic (handlers, inference, memory, pairing, etc.) is shared.

```
src/
  lib.rs          — Shared library crate (re-exports all modules below)
  main.rs         — Entry point for main bot
  config.rs       — Config struct, env loading, path constants
  format.rs       — Markdown → Telegram HTML converter + safe message splitting
  handlers.rs     — Command dispatch, message processing
  inference.rs    — Claude CLI subprocess, JSON parsing
  memory.rs       — Memory, dialogs, system prompt builder
  pairing.rs      — Paired/pending users (both persistent)
  queue.rs        — SQLite message queue (dedup, reliable delivery)
  telegram.rs     — Reactions, send_long, download_media
  update_check.rs — Periodic GitHub release checker
  outbox.rs       — Proactive message sending via outbox files
  vecstore.rs     — Embedding + semantic search (fastembed + usearch)
  watchdog.rs     — Peer bot health monitoring (launchd service checker)

sophia-rescue/
  src/
    main.rs       — Entry point for rescue bot (uses shared lib crate)

sophia-nexus/
  src/
    main.rs       — MCP server for Claude Code integration

data/
  instructions/  — System prompt files (see below)
  memory/        — Runtime memory (auto-managed via [MEMORY_UPDATE] tags)
  dialogs/       — Per-user per-day conversation logs
  users/         — Pairing data (paired.json, pending.json, owner.json)
  files/         — Downloaded media files
  outbox/        — Proactive message JSON files
  vecstore.usearch — Semantic search index
```

### Instruction files

All files in `data/instructions/` are loaded into the system prompt:

| File | Purpose | In repo |
|---|---|---|
| `AGENTS.md` | Bootstrap: startup rules, memory protocol, red lines | Yes (template) |
| `IDENTITY.md` | Bot identity: name, role, emoji, backstory | Yes (template) |
| `SOUL.md` | Personality: thinking style, communication, boundaries | Yes (template) |
| `USER.md` | Owner info: name, timezone, preferences | Yes (template) |
| `TOOLS.md` | Environment-specific notes (SSH, TTS, APIs) | No (gitignored) |
| `MEMORY.md` | Curated long-term memory | No (gitignored) |

Copy `.example` files to get started:

```bash
cp data/instructions/TOOLS.md.example data/instructions/TOOLS.md
cp data/instructions/MEMORY.md.example data/instructions/MEMORY.md
```

## Sophia NEXUS (MCP Server)

`sophia-nexus` is a separate workspace crate that implements a [Model Context Protocol](https://modelcontextprotocol.io/) server. It gives Claude Code (or any MCP-compatible client) direct access to Sophia's data: personality files, memory, dialog history, Telegram messaging, and semantic search.

Configured in `.mcp.json` at the project root. Build with `cargo build --release` (workspace builds all crates).

## License

MIT
