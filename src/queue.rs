#![allow(dead_code)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::Connection;
use tracing::info;

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

#[derive(Debug, Clone)]
pub struct QueuedMessage {
    pub id: i64,
    pub sender_id: i64,
    pub chat_id: i64,
    pub msg_id: i32,
    pub text: String,
    pub status: String,
    pub created_at: f64,
    pub file_paths: String,
}

#[derive(Clone)]
pub struct MessageQueue {
    conn: Arc<Mutex<Connection>>,
}

impl MessageQueue {
    pub fn new(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path).context("Failed to open queue database")?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS messages (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                sender_id   INTEGER NOT NULL,
                chat_id     INTEGER NOT NULL,
                msg_id      INTEGER NOT NULL,
                text        TEXT    NOT NULL,
                status      TEXT    NOT NULL DEFAULT 'pending',
                created_at  REAL    NOT NULL,
                updated_at  REAL    NOT NULL,
                file_paths  TEXT    NOT NULL DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS idx_messages_status
            ON messages (status, created_at);
            CREATE INDEX IF NOT EXISTS idx_messages_chat_msg
            ON messages (chat_id, msg_id);",
        )?;

        // Migration: add file_paths if missing
        let has_file_paths = conn
            .prepare("PRAGMA table_info(messages)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .any(|name| name == "file_paths");
        if !has_file_paths {
            conn.execute(
                "ALTER TABLE messages ADD COLUMN file_paths TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn recover(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let t = now();
        let count = conn.execute(
            "UPDATE messages SET status = 'pending', updated_at = ?1 WHERE status = 'processing'",
            [t],
        )?;
        if count > 0 {
            info!("Recovered {} messages stuck in 'processing'", count);
        }
        Ok(count)
    }

    pub fn enqueue(
        &self,
        sender_id: i64,
        chat_id: i64,
        msg_id: i32,
        text: &str,
        file_paths: &str,
    ) -> Result<(i64, bool)> {
        let conn = self.conn.lock().unwrap();
        let t = now();
        let cutoff = t - 60.0;

        // Dedup by msg_id: skip if this Telegram message exists in ANY status
        // (pending, processing, or done) — prevents re-processing after restart + catch_up
        let already_handled: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE chat_id = ?1 AND msg_id = ?2",
                rusqlite::params![chat_id, msg_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            > 0;

        if already_handled {
            info!("Skipping duplicate msg_id={} (already handled)", msg_id);
            return Ok((0, true));
        }

        // Dedup by text content (for batching window)
        // Check both 'pending' AND 'processing' — prevents re-enqueue while Claude is still thinking
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM messages WHERE sender_id = ?1 AND chat_id = ?2 AND text = ?3 \
                 AND status IN ('pending', 'processing') AND created_at > ?4 ORDER BY created_at DESC LIMIT 1",
                rusqlite::params![sender_id, chat_id, text, cutoff],
                |row| row.get(0),
            )
            .ok();

        if let Some(id) = existing {
            return Ok((id, true));
        }

        conn.execute(
            "INSERT INTO messages (sender_id, chat_id, msg_id, text, status, created_at, updated_at, file_paths) \
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?5, ?6)",
            rusqlite::params![sender_id, chat_id, msg_id, text, t, file_paths],
        )?;
        let id = conn.last_insert_rowid();
        Ok((id, false))
    }

    pub fn take_batch(&self, sender_id: i64, chat_id: i64) -> Result<Vec<QueuedMessage>> {
        let conn = self.conn.lock().unwrap();
        let t = now();

        let mut stmt = conn.prepare(
            "SELECT id, sender_id, chat_id, msg_id, text, status, created_at, file_paths \
             FROM messages WHERE status = 'pending' AND sender_id = ?1 AND chat_id = ?2 \
             ORDER BY created_at ASC",
        )?;
        let msgs: Vec<QueuedMessage> = stmt
            .query_map(rusqlite::params![sender_id, chat_id], |row| {
                Ok(QueuedMessage {
                    id: row.get(0)?,
                    sender_id: row.get(1)?,
                    chat_id: row.get(2)?,
                    msg_id: row.get(3)?,
                    text: row.get(4)?,
                    status: row.get(5)?,
                    created_at: row.get(6)?,
                    file_paths: row.get(7)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        if !msgs.is_empty() {
            let ids: Vec<String> = msgs.iter().map(|m| m.id.to_string()).collect();
            let placeholders = ids.join(",");
            conn.execute(
                &format!(
                    "UPDATE messages SET status = 'processing', updated_at = {} WHERE id IN ({})",
                    t, placeholders
                ),
                [],
            )?;
        }

        Ok(msgs)
    }

    pub fn mark_done(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE messages SET status = 'done', updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now(), id],
        )?;
        Ok(())
    }

    pub fn mark_failed(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE messages SET status = 'failed', updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now(), id],
        )?;
        Ok(())
    }

    pub fn cleanup(&self, older_than_hours: u64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let cutoff = now() - (older_than_hours as f64 * 3600.0);
        let count = conn.execute(
            "DELETE FROM messages WHERE status = 'done' AND updated_at < ?1",
            [cutoff],
        )?;
        if count > 0 {
            info!("Cleaned up {} old messages from queue", count);
        }
        Ok(count)
    }
}
