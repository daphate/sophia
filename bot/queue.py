"""
Persistent message queue backed by SQLite.

Messages are enqueued as 'pending', picked up by a worker,
moved to 'processing', then marked 'done' or 'failed'.
On startup, any 'processing' messages are reset to 'pending'
(they crashed mid-flight).
"""

import asyncio
import logging
import sqlite3
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

from bot.config import PROJECT_ROOT

logger = logging.getLogger(__name__)

DB_PATH = PROJECT_ROOT / "data" / "queue.db"

# Statuses
PENDING = "pending"
PROCESSING = "processing"
DONE = "done"
FAILED = "failed"


@dataclass
class QueuedMessage:
    id: int
    sender_id: int
    chat_id: int
    msg_id: int
    text: str
    status: str
    created_at: float
    file_paths: str = ""  # JSON array of file paths, e.g. '["/tmp/sophia/img.png"]'


class MessageQueue:
    """SQLite-backed persistent message queue."""

    def __init__(self, db_path: Path = DB_PATH):
        self._db_path = db_path
        self._db_path.parent.mkdir(parents=True, exist_ok=True)
        self._conn = sqlite3.connect(str(db_path))
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA synchronous=NORMAL")
        self._init_schema()
        self._event = asyncio.Event()

    def _init_schema(self):
        self._conn.execute("""
            CREATE TABLE IF NOT EXISTS messages (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                sender_id   INTEGER NOT NULL,
                chat_id     INTEGER NOT NULL,
                msg_id      INTEGER NOT NULL,
                text        TEXT    NOT NULL,
                status      TEXT    NOT NULL DEFAULT 'pending',
                created_at  REAL    NOT NULL,
                updated_at  REAL    NOT NULL,
                file_paths  TEXT    NOT NULL DEFAULT ''
            )
        """)
        self._conn.execute("""
            CREATE INDEX IF NOT EXISTS idx_messages_status
            ON messages (status, created_at)
        """)
        self._conn.commit()
        # Migration: add file_paths column if missing
        cols = {r[1] for r in self._conn.execute("PRAGMA table_info(messages)").fetchall()}
        if "file_paths" not in cols:
            self._conn.execute("ALTER TABLE messages ADD COLUMN file_paths TEXT NOT NULL DEFAULT ''")
            self._conn.commit()

    def recover(self) -> int:
        """Reset 'processing' messages back to 'pending' after a crash.
        Returns number of recovered messages."""
        now = time.time()
        cur = self._conn.execute(
            "UPDATE messages SET status = ?, updated_at = ? WHERE status = ?",
            (PENDING, now, PROCESSING),
        )
        self._conn.commit()
        count = cur.rowcount
        if count:
            logger.info("Recovered %d messages stuck in 'processing'", count)
        return count

    def enqueue(self, sender_id: int, chat_id: int, msg_id: int, text: str, file_paths: str = "") -> tuple[int, bool]:
        """Add a message to the queue. Returns (queue_row_id, is_duplicate).
        Deduplicates: if the same sender+chat+text was enqueued within the last
        60 seconds and is still pending, returns the existing row instead.
        file_paths: JSON array string of local file paths, e.g. '["/tmp/sophia/img.png"]'
        """
        now = time.time()

        # Dedup: check for identical pending message from same sender+chat within 60s
        cutoff = now - 60
        existing = self._conn.execute(
            "SELECT id FROM messages WHERE sender_id = ? AND chat_id = ? AND text = ? "
            "AND status = ? AND created_at > ? ORDER BY created_at DESC LIMIT 1",
            (sender_id, chat_id, text, PENDING, cutoff),
        ).fetchone()
        if existing:
            logger.info("Dedup: skipping duplicate message from %d (existing queue id %d)", sender_id, existing[0])
            return existing[0], True

        cur = self._conn.execute(
            "INSERT INTO messages (sender_id, chat_id, msg_id, text, status, created_at, updated_at, file_paths) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            (sender_id, chat_id, msg_id, text, PENDING, now, now, file_paths),
        )
        self._conn.commit()
        row_id = cur.lastrowid
        logger.debug("Enqueued message %d from %d (files: %s)", row_id, sender_id, bool(file_paths))
        self._event.set()
        return row_id, False

    def take_next(self) -> Optional[QueuedMessage]:
        """Atomically take the next pending message (FIFO).
        Returns None if queue is empty."""
        now = time.time()
        row = self._conn.execute(
            "SELECT id, sender_id, chat_id, msg_id, text, status, created_at, file_paths "
            "FROM messages WHERE status = ? ORDER BY created_at ASC LIMIT 1",
            (PENDING,),
        ).fetchone()
        if not row:
            return None
        msg = QueuedMessage(*row)
        self._conn.execute(
            "UPDATE messages SET status = ?, updated_at = ? WHERE id = ?",
            (PROCESSING, now, msg.id),
        )
        self._conn.commit()
        return msg

    def peek_next(self) -> Optional[QueuedMessage]:
        """Peek at the next pending message without changing its status."""
        row = self._conn.execute(
            "SELECT id, sender_id, chat_id, msg_id, text, status, created_at, file_paths "
            "FROM messages WHERE status = ? ORDER BY created_at ASC LIMIT 1",
            (PENDING,),
        ).fetchone()
        if not row:
            return None
        return QueuedMessage(*row)

    def take_batch(self, sender_id: int, chat_id: int) -> list[QueuedMessage]:
        """Atomically take ALL pending messages from a given sender+chat.
        Returns them in FIFO order."""
        now = time.time()
        rows = self._conn.execute(
            "SELECT id, sender_id, chat_id, msg_id, text, status, created_at, file_paths "
            "FROM messages WHERE status = ? AND sender_id = ? AND chat_id = ? "
            "ORDER BY created_at ASC",
            (PENDING, sender_id, chat_id),
        ).fetchall()
        if not rows:
            return []
        msgs = [QueuedMessage(*r) for r in rows]
        ids = [m.id for m in msgs]
        placeholders = ",".join("?" * len(ids))
        self._conn.execute(
            f"UPDATE messages SET status = ?, updated_at = ? WHERE id IN ({placeholders})",
            [PROCESSING, now] + ids,
        )
        self._conn.commit()
        return msgs

    def latest_pending_time(self, sender_id: int, chat_id: int) -> Optional[float]:
        """Return created_at of the most recent pending message from sender+chat."""
        row = self._conn.execute(
            "SELECT created_at FROM messages WHERE status = ? AND sender_id = ? AND chat_id = ? "
            "ORDER BY created_at DESC LIMIT 1",
            (PENDING, sender_id, chat_id),
        ).fetchone()
        return row[0] if row else None

    def mark_done(self, msg_id: int):
        self._conn.execute(
            "UPDATE messages SET status = ?, updated_at = ? WHERE id = ?",
            (DONE, time.time(), msg_id),
        )
        self._conn.commit()

    def mark_failed(self, msg_id: int):
        self._conn.execute(
            "UPDATE messages SET status = ?, updated_at = ? WHERE id = ?",
            (FAILED, time.time(), msg_id),
        )
        self._conn.commit()

    def pending_count(self) -> int:
        row = self._conn.execute(
            "SELECT COUNT(*) FROM messages WHERE status IN (?, ?)",
            (PENDING, PROCESSING),
        ).fetchone()
        return row[0]

    def cleanup(self, older_than_hours: int = 24):
        """Remove completed messages older than N hours."""
        cutoff = time.time() - older_than_hours * 3600
        cur = self._conn.execute(
            "DELETE FROM messages WHERE status = ? AND updated_at < ?",
            (DONE, cutoff),
        )
        self._conn.commit()
        if cur.rowcount:
            logger.info("Cleaned up %d old messages from queue", cur.rowcount)

    async def wait_for_messages(self, timeout: float = 2.0):
        """Wait until a new message is enqueued or timeout."""
        self._event.clear()
        try:
            await asyncio.wait_for(self._event.wait(), timeout=timeout)
        except asyncio.TimeoutError:
            pass

    def close(self):
        self._conn.close()
