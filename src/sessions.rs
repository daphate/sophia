use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

use crate::config::BotRole;

/// Session status in the database.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SessionStatus {
    Active,
    Expired,
    Failed,
}

impl SessionStatus {
    #[allow(dead_code)]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Expired => "expired",
            Self::Failed => "failed",
        }
    }
}

/// Per-bot, per-user Claude CLI session tracker backed by SQLite.
#[derive(Clone)]
pub struct SessionStore {
    conn: Arc<Mutex<Connection>>,
}

impl SessionStore {
    /// Open (or create) the sessions database at the given path.
    pub fn new(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open sessions DB at {}", db_path.display()))?;

        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;
             CREATE TABLE IF NOT EXISTS sessions (
                 id          INTEGER PRIMARY KEY AUTOINCREMENT,
                 bot_role    TEXT    NOT NULL,
                 user_id     INTEGER NOT NULL,
                 session_id  TEXT    NOT NULL,
                 status      TEXT    NOT NULL DEFAULT 'active',
                 created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
                 last_used   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
                 msg_count   INTEGER NOT NULL DEFAULT 0,
                 UNIQUE(bot_role, user_id, status)
             );
             CREATE INDEX IF NOT EXISTS idx_sessions_lookup
                 ON sessions(bot_role, user_id, status);",
        )?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Get the active session for (bot_role, user_id).
    /// Returns `(session_id, is_new)`.
    /// If no active session exists, creates one.
    pub fn get_or_create(&self, role: BotRole, user_id: i64) -> Result<(String, bool)> {
        let role_str = role_to_str(role);
        let conn = self.conn.lock().unwrap();

        // Try to find an active session
        let existing: Option<String> = conn
            .query_row(
                "SELECT session_id FROM sessions
                 WHERE bot_role = ?1 AND user_id = ?2 AND status = 'active'",
                rusqlite::params![role_str, user_id],
                |row| row.get(0),
            )
            .ok();

        if let Some(id) = existing {
            // Touch last_used
            conn.execute(
                "UPDATE sessions SET last_used = strftime('%Y-%m-%dT%H:%M:%SZ','now'),
                                     msg_count = msg_count + 1
                 WHERE bot_role = ?1 AND user_id = ?2 AND status = 'active'",
                rusqlite::params![role_str, user_id],
            )?;
            return Ok((id, false));
        }

        // Create new session
        let session_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO sessions (bot_role, user_id, session_id, status, msg_count)
             VALUES (?1, ?2, ?3, 'active', 1)",
            rusqlite::params![role_str, user_id, &session_id],
        )?;
        info!(
            "New CLI session {} for {}:{}",
            session_id, role_str, user_id
        );
        Ok((session_id, true))
    }

    /// Mark the active session for (role, user_id) as failed and remove it
    /// so the next call starts fresh.
    pub fn invalidate(&self, role: BotRole, user_id: i64) {
        let role_str = role_to_str(role);
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE sessions SET status = 'failed'
             WHERE bot_role = ?1 AND user_id = ?2 AND status = 'active'",
            rusqlite::params![role_str, user_id],
        );
        warn!("Invalidated CLI session for {}:{}", role_str, user_id);
    }

    /// Expire all active sessions older than `ttl_hours`.
    pub fn expire_stale(&self, ttl_hours: u64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE sessions SET status = 'expired'
             WHERE status = 'active'
               AND last_used < strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?1)",
            rusqlite::params![format!("-{ttl_hours} hours")],
        )?;
        if n > 0 {
            info!("Expired {} stale CLI sessions (TTL={}h)", n, ttl_hours);
        }
        Ok(n)
    }

    /// Delete non-active sessions older than `days`.
    pub fn cleanup(&self, days: u64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "DELETE FROM sessions
             WHERE status != 'active'
               AND last_used < strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?1)",
            rusqlite::params![format!("-{days} days")],
        )?;
        if n > 0 {
            info!("Cleaned up {} old CLI session records", n);
        }
        Ok(n)
    }
}

fn role_to_str(role: BotRole) -> &'static str {
    match role {
        BotRole::Main => "main",
        BotRole::Rescue => "rescue",
    }
}
