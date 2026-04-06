import re
import logging
from datetime import datetime, timezone
from pathlib import Path

from bot.config import MEMORY_FILE, DIALOGS_DIR, AGENTS_FILE, SOUL_FILE, USER_FILE, MEMORY_DIR

logger = logging.getLogger(__name__)

MEMORY_UPDATE_PATTERN = re.compile(
    r"\[MEMORY_UPDATE\](.*?)\[/MEMORY_UPDATE\]", re.DOTALL
)


def _ensure_dirs():
    MEMORY_DIR.mkdir(parents=True, exist_ok=True)
    DIALOGS_DIR.mkdir(parents=True, exist_ok=True)


def read_file_safe(path: Path) -> str:
    if path.exists():
        return path.read_text(encoding="utf-8")
    return ""


def read_memory() -> str:
    raw = read_file_safe(MEMORY_FILE)
    return _deduplicate_memory(raw)


def _deduplicate_memory(text: str) -> str:
    """Remove duplicate memory entries, keeping the last occurrence of each."""
    lines = text.splitlines()
    non_entry_lines: list[str] = []
    entries: list[str] = []

    for line in lines:
        stripped = line.strip()
        if stripped.startswith("- "):
            entries.append(line)
        else:
            # Flush collected entries before a non-entry line
            if entries:
                entries = _keep_last_unique(entries)
                non_entry_lines.extend(entries)
                entries = []
            non_entry_lines.append(line)

    # Flush remaining entries
    if entries:
        entries = _keep_last_unique(entries)
        non_entry_lines.extend(entries)

    return "\n".join(non_entry_lines)


def _keep_last_unique(entries: list[str]) -> list[str]:
    """Given a list of '- ...' lines, keep only the last occurrence of each (by normalized content)."""
    seen: dict[str, int] = {}
    for i, line in enumerate(entries):
        fact = line.strip()[2:]  # strip "- "
        norm = _normalize_fact(fact)
        if norm:
            seen[norm] = i  # overwrite → keeps last index
    # Keep entries whose index matches the last occurrence, preserving order
    last_indices = set(seen.values())
    return [line for i, line in enumerate(entries) if i in last_indices]


def _normalize_fact(text: str) -> str:
    """Normalize a memory fact for deduplication comparison."""
    import re as _re
    # Strip timestamps like [2026-04-07 09:14 UTC]
    s = _re.sub(r"\[\d{4}-\d{2}-\d{2}\s+\d{2}:\d{2}\s*\w*\]\s*", "", text)
    return s.strip().lower()


def append_memory(text: str):
    _ensure_dirs()
    current = read_memory()
    # Remove the "No memories" placeholder if present
    if "No memories stored yet." in current:
        current = "# Memory\n\n"

    # Deduplicate: skip if a similar fact already exists
    new_norm = _normalize_fact(text)
    for line in current.splitlines():
        line_stripped = line.strip()
        if line_stripped.startswith("- "):
            existing_norm = _normalize_fact(line_stripped[2:])
            if existing_norm and new_norm and existing_norm == new_norm:
                logger.info("Skipping duplicate memory entry: %s", text.strip()[:80])
                return

    timestamp = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M UTC")
    entry = f"- [{timestamp}] {text.strip()}\n"
    MEMORY_FILE.write_text(current.rstrip() + "\n" + entry, encoding="utf-8")
    logger.info("Memory updated: %s", text.strip()[:80])


def clear_memory():
    _ensure_dirs()
    MEMORY_FILE.write_text("# Memory\n\nNo memories stored yet.\n", encoding="utf-8")


def extract_memory_updates(response: str) -> tuple[str, list[str]]:
    """Extract [MEMORY_UPDATE] blocks from response. Returns (cleaned_response, updates)."""
    updates = MEMORY_UPDATE_PATTERN.findall(response)
    cleaned = MEMORY_UPDATE_PATTERN.sub("", response).strip()
    return cleaned, [u.strip() for u in updates if u.strip()]


def build_system_prompt(recent_dialog: str) -> str:
    agents = read_file_safe(AGENTS_FILE)
    soul = read_file_safe(SOUL_FILE)
    user_ctx = read_file_safe(USER_FILE)
    memory = read_memory()
    # Truncate memory to last 1200 chars if too long
    if len(memory) > 1200:
        memory = "# Memory\n…(older entries truncated)\n" + memory[-1200:]

    parts = []
    if agents:
        parts.append(agents.strip())
    if soul:
        parts.append(soul.strip())
    if user_ctx and "No specific user context" not in user_ctx:
        parts.append(user_ctx.strip())
    if memory and "No memories stored yet." not in memory:
        parts.append(memory.strip())
    if recent_dialog:
        parts.append(f"# Recent Conversation\n{recent_dialog}")

    parts.append(
        "To save important facts, append at end of response: "
        "[MEMORY_UPDATE]fact[/MEMORY_UPDATE] (hidden from user)."
    )

    return "\n\n".join(parts)


# --- Dialog persistence ---

def _dialog_path(user_id: int) -> Path:
    today = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    user_dir = DIALOGS_DIR / str(user_id)
    user_dir.mkdir(parents=True, exist_ok=True)
    return user_dir / f"{today}.md"


def append_dialog(user_id: int, role: str, text: str):
    path = _dialog_path(user_id)
    timestamp = datetime.now(timezone.utc).strftime("%H:%M:%S")
    entry = f"**{role}** [{timestamp}]: {text}\n\n"
    with open(path, "a", encoding="utf-8") as f:
        f.write(entry)


def load_recent_dialog(user_id: int, max_turns: int = 15, max_total_chars: int = 3000) -> str:
    path = _dialog_path(user_id)
    if not path.exists():
        return ""
    content = path.read_text(encoding="utf-8")
    # Each turn is separated by double newline, starts with **role**
    turns = [t.strip() for t in content.split("\n\n") if t.strip()]
    recent = turns[-max_turns:]
    # Truncate long individual turns
    truncated = []
    for t in recent:
        if len(t) > 300:
            t = t[:300] + "…(truncated)"
        truncated.append(t)
    # Enforce total size cap: drop oldest turns until within budget
    result = "\n\n".join(truncated)
    while len(result) > max_total_chars and len(truncated) > 4:
        truncated.pop(0)
        result = "\n\n".join(truncated)
    return result
