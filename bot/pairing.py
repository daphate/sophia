import json
import logging
from pathlib import Path

from bot.config import OWNER_FILE, PAIRED_FILE, USERS_DIR

logger = logging.getLogger(__name__)


def _ensure_dir():
    USERS_DIR.mkdir(parents=True, exist_ok=True)


def load_owner() -> dict:
    if OWNER_FILE.exists():
        return json.loads(OWNER_FILE.read_text(encoding="utf-8"))
    return {}


def save_owner(info: dict):
    _ensure_dir()
    OWNER_FILE.write_text(json.dumps(info, indent=2), encoding="utf-8")


def load_paired() -> dict[str, dict]:
    """Returns {str(user_id): {name, paired_at, ...}}"""
    if PAIRED_FILE.exists():
        return json.loads(PAIRED_FILE.read_text(encoding="utf-8"))
    return {}


def save_paired(paired: dict[str, dict]):
    _ensure_dir()
    PAIRED_FILE.write_text(json.dumps(paired, indent=2), encoding="utf-8")


def is_paired(user_id: int) -> bool:
    paired = load_paired()
    return str(user_id) in paired


def add_paired(user_id: int, name: str):
    from datetime import datetime, timezone

    paired = load_paired()
    paired[str(user_id)] = {
        "name": name,
        "paired_at": datetime.now(timezone.utc).isoformat(),
    }
    save_paired(paired)
    logger.info("Paired user %s (%s)", user_id, name)


def remove_paired(user_id: int) -> bool:
    paired = load_paired()
    key = str(user_id)
    if key in paired:
        del paired[key]
        save_paired(paired)
        logger.info("Unpaired user %s", user_id)
        return True
    return False


# Pending pair requests: {user_id: {name, requested_at}}
_pending: dict[int, dict] = {}


def add_pending(user_id: int, name: str):
    from datetime import datetime, timezone

    _pending[user_id] = {
        "name": name,
        "requested_at": datetime.now(timezone.utc).isoformat(),
    }


def get_pending(user_id: int) -> dict | None:
    return _pending.get(user_id)


def remove_pending(user_id: int):
    _pending.pop(user_id, None)


def list_pending() -> dict[int, dict]:
    return dict(_pending)
