import asyncio
import json
import logging
import os
import re
import shlex
import subprocess
import time
from pathlib import Path

from telethon import events, TelegramClient
from telethon.tl.functions.messages import SendReactionRequest
from telethon.tl.types import ReactionEmoji

from bot.config import Config, FILES_DIR
from bot.inference import ask_claude
from bot.memory import (
    append_dialog,
    append_memory,
    clear_memory,
    extract_memory_updates,
    read_memory,
)
from bot.pairing import (
    add_paired,
    add_pending,
    get_pending,
    is_paired,
    list_pending,
    remove_paired,
    remove_pending,
)

logger = logging.getLogger(__name__)

# Per-user locks for dialog writes
_user_locks: dict[int, asyncio.Lock] = {}

# Dangerous shell patterns
_SHELL_CHAIN = re.compile(r"[;|&`$()]")

# Max file size to download (20 MB)
_MAX_FILE_SIZE = 20 * 1024 * 1024

# Supported media extensions for Claude CLI Read tool
_IMAGE_EXTS = {".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp", ".svg"}
_DOC_EXTS = {".txt", ".md", ".json", ".csv", ".xml", ".yaml", ".yml",
             ".py", ".js", ".ts", ".rs", ".go", ".java", ".c", ".cpp", ".h",
             ".html", ".css", ".sh", ".toml", ".ini", ".cfg", ".log", ".pdf"}
_SUPPORTED_EXTS = _IMAGE_EXTS | _DOC_EXTS


async def _download_media(client: TelegramClient, event) -> list[str]:
    """Download media/documents from a Telegram message. Returns list of file paths."""
    if not event.media:
        return []

    # Get file info
    file = event.file
    if file is None:
        return []

    # Check file size
    size = file.size or 0
    if size > _MAX_FILE_SIZE:
        logger.warning("File too large (%d bytes), skipping", size)
        return []

    # Determine filename
    name = file.name
    if not name:
        ext = file.ext or ""
        name = f"file_{int(time.time())}_{event.id}{ext}"

    # Check extension
    ext = Path(name).suffix.lower()
    if ext not in _SUPPORTED_EXTS and ext:
        logger.info("Unsupported file extension %s, will still try", ext)

    # Ensure download directory exists
    user_dir = FILES_DIR / str(event.sender_id)
    user_dir.mkdir(parents=True, exist_ok=True)

    # Download
    dest = user_dir / f"{event.id}_{name}"
    try:
        path = await client.download_media(event.message, file=str(dest))
        if path:
            logger.info("Downloaded file: %s (%d bytes)", path, size)
            return [str(path)]
    except Exception as e:
        logger.error("Failed to download media: %s", e)

    return []


async def _react(client: TelegramClient, event, emoji: str):
    """Set a reaction on a message."""
    try:
        await client(SendReactionRequest(
            peer=event.chat_id,
            msg_id=event.id,
            reaction=[ReactionEmoji(emoticon=emoji)],
        ))
    except Exception as e:
        logger.debug("Failed to set reaction %s: %s", emoji, e)


def _get_lock(user_id: int) -> asyncio.Lock:
    if user_id not in _user_locks:
        _user_locks[user_id] = asyncio.Lock()
    return _user_locks[user_id]


async def _send_long(event, text: str):
    """Send message, splitting at 4096 char Telegram limit."""
    MAX = 4096
    while text:
        if len(text) <= MAX:
            await event.respond(text)
            break
        # Find a newline to split at
        split_at = text.rfind("\n", 0, MAX)
        if split_at == -1:
            split_at = MAX
        await event.respond(text[:split_at])
        text = text[split_at:].lstrip("\n")


def register_handlers(client: TelegramClient, config: Config, me_id: int):
    """Register all event handlers on the client."""

    @client.on(events.NewMessage(incoming=True, func=lambda e: e.is_private))
    async def on_private_message(event):
        sender_id = event.sender_id
        if sender_id == me_id:
            return

        text = event.raw_text.strip()
        has_media = event.media is not None

        # Skip if no text and no media
        if not text and not has_media:
            return

        is_owner = sender_id == config.owner_id

        # --- Command dispatch ---
        if text and text.startswith("/"):
            parts = text.split(maxsplit=1)
            cmd = parts[0].lower()
            arg = parts[1].strip() if len(parts) > 1 else ""

            if cmd == "/pair":
                await _handle_pair(event, sender_id, is_owner, config, client)
                return
            if cmd == "/approve" and is_owner:
                await _handle_approve(event, arg, client)
                return
            if cmd == "/deny" and is_owner:
                await _handle_deny(event, arg, client)
                return
            if cmd == "/unpair" and is_owner:
                await _handle_unpair(event, arg)
                return
            if cmd == "/exec" and is_owner:
                await _handle_exec(event, arg, config)
                return
            if cmd == "/memory" and is_owner:
                await _handle_memory(event, arg)
                return
            if cmd == "/help":
                if is_owner or is_paired(sender_id):
                    await _handle_help(event, is_owner)
                    return

        # --- Access check for non-command messages ---
        if not is_owner and not is_paired(sender_id):
            await event.respond(
                "I don't know you yet. Send /pair to request access."
            )
            return

        # --- Download attached files ---
        file_paths: list[str] = []
        if has_media:
            file_paths = await _download_media(client, event)
            if not text and file_paths:
                text = "Пользователь отправил файл. Прочитай и опиши его содержимое."

        if not text:
            return

        # --- Process message inline ---
        await _react(client, event, "🫡")
        logger.info("Message from %d, processing inline", sender_id)

        # Run inference in background task so the event loop stays responsive
        asyncio.create_task(
            _process_message(client, config, event, sender_id, text, file_paths),
            name=f"msg-{sender_id}-{event.id}",
        )

    # --- Command handlers (module-level functions) ---


async def _handle_pair(event, sender_id: int, is_owner: bool, config: Config, client: TelegramClient):
    if is_owner:
        await event.respond("You're the owner — no pairing needed.")
        return
    if is_paired(sender_id):
        await event.respond("You're already paired!")
        return

    sender = await event.get_sender()
    name = _get_display_name(sender)
    add_pending(sender_id, name)
    await event.respond(
        "Pairing request sent to the owner. Please wait for approval."
    )

    try:
        await client.send_message(
            config.owner_id,
            f"Pairing request from **{name}** (ID: `{sender_id}`).\n"
            f"Use `/approve {sender_id}` or `/deny {sender_id}`.",
        )
    except Exception as e:
        logger.error("Failed to notify owner: %s", e)


async def _handle_approve(event, arg: str, client: TelegramClient):
    if not arg.isdigit():
        await event.respond("Usage: /approve <user_id>")
        return
    uid = int(arg)
    pending = get_pending(uid)
    if not pending:
        await event.respond(f"No pending request from ID {uid}.")
        return
    add_paired(uid, pending["name"])
    remove_pending(uid)
    await event.respond(f"Approved **{pending['name']}** ({uid}).")
    try:
        await client.send_message(uid, "You've been approved! You can now chat with me.")
    except Exception:
        pass


async def _handle_deny(event, arg: str, client: TelegramClient):
    if not arg.isdigit():
        await event.respond("Usage: /deny <user_id>")
        return
    uid = int(arg)
    pending = get_pending(uid)
    if not pending:
        await event.respond(f"No pending request from ID {uid}.")
        return
    remove_pending(uid)
    await event.respond(f"Denied **{pending['name']}** ({uid}).")
    try:
        await client.send_message(uid, "Your pairing request was denied.")
    except Exception:
        pass


async def _handle_unpair(event, arg: str):
    if not arg.isdigit():
        await event.respond("Usage: /unpair <user_id>")
        return
    uid = int(arg)
    if remove_paired(uid):
        await event.respond(f"Unpaired user {uid}.")
    else:
        await event.respond(f"User {uid} was not paired.")


async def _handle_exec(event, arg: str, config: Config):
    if not config.exec_enabled:
        await event.respond("Command execution is disabled.")
        return
    if not arg:
        await event.respond("Usage: /exec <command>")
        return

    if _SHELL_CHAIN.search(arg):
        await event.respond("Blocked: shell chaining/subshells not allowed.")
        return

    try:
        parts = shlex.split(arg, posix=True)
    except ValueError as e:
        await event.respond(f"Parse error: {e}")
        return

    if not parts:
        await event.respond("Empty command.")
        return

    if parts[0] not in config.exec_allowed_commands:
        allowed = ", ".join(config.exec_allowed_commands)
        await event.respond(
            f"Command `{parts[0]}` not allowed.\nAllowed: `{allowed}`"
        )
        return

    try:
        proc = await asyncio.to_thread(
            subprocess.run,
            parts,
            capture_output=True,
            text=True,
            timeout=30,
        )
    except subprocess.TimeoutExpired:
        await event.respond("Command timed out (30s limit).")
        return
    except Exception as e:
        await event.respond(f"Exec error: {e}")
        return

    output = proc.stdout or ""
    if proc.stderr:
        output += "\n" + proc.stderr

    output = output.strip()
    if not output:
        output = "(no output)"
    if len(output) > 4000:
        output = output[:4000] + "\n... (truncated)"

    await _send_long(event, f"```\n{output}\n```")


async def _handle_memory(event, arg: str):
    if not arg:
        mem = read_memory()
        if not mem.strip():
            mem = "(empty)"
        await _send_long(event, f"**Memory:**\n{mem}")
        return

    if arg.startswith("add "):
        text = arg[4:].strip()
        if text:
            append_memory(text)
            await event.respond("Memory updated.")
        else:
            await event.respond("Usage: /memory add <text>")
        return

    if arg == "clear":
        clear_memory()
        await event.respond("Memory cleared.")
        return

    await event.respond("Usage: /memory [add <text> | clear]")


async def _handle_help(event, is_owner: bool):
    lines = [
        "**Commands:**",
        "`/pair` — Request access",
        "`/help` — Show this help",
    ]
    if is_owner:
        lines.extend([
            "",
            "**Owner commands:**",
            "`/approve <id>` — Approve pairing",
            "`/deny <id>` — Deny pairing",
            "`/unpair <id>` — Remove paired user",
            "`/exec <cmd>` — Run OS command",
            "`/memory` — View memory",
            "`/memory add <text>` — Add to memory",
            "`/memory clear` — Clear memory",
        ])
    await event.respond("\n".join(lines))


async def _process_message(
    client: TelegramClient, config: Config, event,
    sender_id: int, text: str, file_paths: list[str],
):
    """Process a single message: inference + reply. Runs as a background task."""
    chat_id = event.chat_id

    lock = _get_lock(sender_id)
    async with lock:
        append_dialog(sender_id, "User", text)

    # 🤔 Thinking
    await _react(client, event, "🤔")

    response = None
    cost = None
    try:
        async with client.action(chat_id, "typing"):
            response, cost = await ask_claude(
                sender_id, text, config,
                file_paths=file_paths if file_paths else None,
            )
    except Exception as e:
        await _react(client, event, "🥶")
        logger.error("Inference failed for msg %d: %s", event.id, e)
        await event.respond("Произошла ошибка при обработке запроса.")
        return

    # 🧑‍💻 Composing
    await _react(client, event, "🧑‍💻")

    # Extract and save memory updates
    cleaned, updates = extract_memory_updates(response)
    for update in updates:
        append_memory(update)

    if cost:
        logger.info(
            "Inference cost for %d: in=%s out=%s usd=%s",
            sender_id,
            cost.get("input_tokens"),
            cost.get("output_tokens"),
            cost.get("cost_usd"),
        )

    async with lock:
        append_dialog(sender_id, "Sophia", cleaned)

    # Send response
    await _send_long(event, cleaned)

    # 👌 Done
    await _react(client, event, "👌")


def _get_display_name(sender) -> str:
    parts = []
    if getattr(sender, "first_name", None):
        parts.append(sender.first_name)
    if getattr(sender, "last_name", None):
        parts.append(sender.last_name)
    return " ".join(parts) if parts else f"User {sender.id}"
