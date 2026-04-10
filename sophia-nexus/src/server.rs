use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use regex::Regex;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    ErrorData as McpError,
};
use serde_json::json;

use crate::paths;

// ---------------------------------------------------------------------------
// Parameter structs
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadInstructionsParams {
    /// Which instruction file: IDENTITY, SOUL, USER, AGENTS, TOOLS, or MEMORY
    pub file: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AppendMemoryParams {
    /// The fact to remember (timestamp added automatically)
    pub fact: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListDialogsParams {
    /// Filter to a specific user ID (omit for all users)
    #[serde(default)]
    pub user_id: Option<i64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadDialogParams {
    /// Telegram user ID
    pub user_id: i64,
    /// Date in YYYY-MM-DD format (defaults to today)
    #[serde(default)]
    pub date: Option<String>,
    /// Maximum number of turns to return (default 50)
    #[serde(default)]
    pub max_turns: Option<u32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AppendDialogParams {
    /// Telegram user ID
    pub user_id: i64,
    /// Role name (e.g. "User", "Claude Code", "Sophia")
    pub role: String,
    /// Message text
    pub text: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SendTelegramParams {
    /// Telegram chat ID to send to
    pub chat_id: i64,
    /// Message text
    pub text: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchVecstoreParams {
    /// Search query (substring match)
    pub query: String,
    /// Max results (default 10)
    #[serde(default)]
    pub limit: Option<u32>,
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SophiaNexus {
    data_dir: PathBuf,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl SophiaNexus {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            tool_router: Self::tool_router(),
        }
    }

    fn read_file_safe(&self, path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_default()
    }
}

#[tool_handler]
impl ServerHandler for SophiaNexus {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Sophia NEXUS — MCP server providing access to Sophia bot's data: \
                 personality, memory, dialogs, Telegram messaging, and conversation search.",
            )
    }
}

#[tool_router]
impl SophiaNexus {
    /// Read one of Sophia's personality/instruction files
    #[tool(description = "Read Sophia's personality/instruction files (IDENTITY, SOUL, USER, AGENTS, TOOLS, MEMORY)")]
    fn read_instructions(
        &self,
        Parameters(p): Parameters<ReadInstructionsParams>,
    ) -> Result<CallToolResult, McpError> {
        let dir = paths::instructions_dir(&self.data_dir);
        let filename = match p.file.to_uppercase().as_str() {
            "IDENTITY" => "IDENTITY.md",
            "SOUL" => "SOUL.md",
            "USER" => "USER.md",
            "AGENTS" => "AGENTS.md",
            "TOOLS" => "TOOLS.md",
            "MEMORY" => "MEMORY.md",
            other => {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Unknown file: {}. Valid: IDENTITY, SOUL, USER, AGENTS, TOOLS, MEMORY",
                    other
                ))]));
            }
        };
        let content = self.read_file_safe(&dir.join(filename));
        if content.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(format!(
                "(file {} is empty or missing)",
                filename
            ))]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(content)]))
        }
    }

    /// Read Sophia's long-term memory
    #[tool(description = "Read Sophia's long-term memory (shared with Telegram bot)")]
    fn read_memory(&self) -> Result<CallToolResult, McpError> {
        let path = paths::memory_file(&self.data_dir);
        let raw = self.read_file_safe(&path);
        let content = deduplicate_memory(&raw);
        Ok(CallToolResult::success(vec![Content::text(content)]))
    }

    /// Append a fact to long-term memory
    #[tool(description = "Append a fact to Sophia's long-term memory (shared with Telegram bot). Timestamp auto-added. Duplicates skipped.")]
    fn append_memory(
        &self,
        Parameters(p): Parameters<AppendMemoryParams>,
    ) -> Result<CallToolResult, McpError> {
        let path = paths::memory_file(&self.data_dir);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let current = self.read_file_safe(&path);
        let current = if current.contains("No memories stored yet.") {
            "# Memory\n\n".to_string()
        } else {
            current
        };

        // Dedup check
        let new_norm = normalize_fact(&p.fact);
        for line in current.lines() {
            let stripped = line.trim();
            if let Some(fact) = stripped.strip_prefix("- ") {
                let existing_norm = normalize_fact(fact);
                if !existing_norm.is_empty() && !new_norm.is_empty() && existing_norm == new_norm {
                    return Ok(CallToolResult::success(vec![Content::text(
                        "Duplicate fact, skipped.",
                    )]));
                }
            }
        }

        let timestamp = Utc::now().format("%Y-%m-%d %H:%M UTC");
        let entry = format!("- [{}] {}\n", timestamp, p.fact.trim());
        let new_content = format!("{}\n{}", current.trim_end(), entry);
        fs::write(&path, new_content)
            .map_err(|e| McpError::internal_error(format!("Write failed: {}", e), None))?;

        Ok(CallToolResult::success(vec![Content::text("ok")]))
    }

    /// List available dialog users and dates
    #[tool(description = "List available dialog users and dates")]
    fn list_dialogs(
        &self,
        Parameters(p): Parameters<ListDialogsParams>,
    ) -> Result<CallToolResult, McpError> {
        let dir = paths::dialogs_dir(&self.data_dir);
        if !dir.exists() {
            return Ok(CallToolResult::success(vec![Content::text("[]")]));
        }

        let mut result = Vec::new();
        let entries = fs::read_dir(&dir).map_err(|e| {
            McpError::internal_error(format!("Cannot read dialogs dir: {}", e), None)
        })?;

        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let user_id: i64 = match entry.file_name().to_str().and_then(|s| s.parse().ok()) {
                Some(id) => id,
                None => continue,
            };
            if let Some(filter_id) = p.user_id {
                if user_id != filter_id {
                    continue;
                }
            }

            let mut dates = Vec::new();
            if let Ok(files) = fs::read_dir(entry.path()) {
                for f in files.flatten() {
                    let name = f.file_name();
                    let name = name.to_string_lossy();
                    if name.ends_with(".md") {
                        dates.push(name.trim_end_matches(".md").to_string());
                    }
                }
            }
            dates.sort();
            result.push(json!({"user_id": user_id, "dates": dates}));
        }

        let output = serde_json::to_string_pretty(&result).unwrap_or_else(|_| "[]".into());
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    /// Read dialog history for a user on a given date
    #[tool(description = "Read conversation history for a Telegram user (defaults to today, most recent turns)")]
    fn read_dialog(
        &self,
        Parameters(p): Parameters<ReadDialogParams>,
    ) -> Result<CallToolResult, McpError> {
        let date = p
            .date
            .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string());
        let max_turns = p.max_turns.unwrap_or(50) as usize;

        let path = paths::dialogs_dir(&self.data_dir)
            .join(p.user_id.to_string())
            .join(format!("{}.md", date));

        if !path.exists() {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No dialog found for user {} on {}",
                p.user_id, date
            ))]));
        }

        let content = self.read_file_safe(&path);
        let turns: Vec<&str> = content
            .split("\n\n")
            .filter(|t| !t.trim().is_empty())
            .collect();

        let recent: Vec<String> = turns
            .iter()
            .rev()
            .take(max_turns)
            .rev()
            .map(|t| {
                if t.len() > 500 {
                    let end = t.floor_char_boundary(500);
                    format!("{}...(truncated)", &t[..end])
                } else {
                    t.to_string()
                }
            })
            .collect();

        let output = if recent.is_empty() {
            format!("(empty dialog for user {} on {})", p.user_id, date)
        } else {
            recent.join("\n\n")
        };
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    /// Append a message to the dialog log
    #[tool(description = "Append a message to Sophia's dialog log (log Claude Code conversations to shared history)")]
    fn append_dialog(
        &self,
        Parameters(p): Parameters<AppendDialogParams>,
    ) -> Result<CallToolResult, McpError> {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let user_dir = paths::dialogs_dir(&self.data_dir).join(p.user_id.to_string());
        let _ = fs::create_dir_all(&user_dir);
        let path = user_dir.join(format!("{}.md", today));

        let timestamp = Utc::now().format("%H:%M:%S");
        let entry = format!("**{}** [{}]: {}\n\n", p.role, timestamp, p.text);
        let mut content = self.read_file_safe(&path);
        content.push_str(&entry);
        fs::write(&path, content)
            .map_err(|e| McpError::internal_error(format!("Write failed: {}", e), None))?;

        Ok(CallToolResult::success(vec![Content::text("ok")]))
    }

    /// Send a Telegram message via the outbox
    #[tool(description = "Send a Telegram message via the bot's outbox (bot picks it up in ~2 seconds)")]
    fn send_telegram(
        &self,
        Parameters(p): Parameters<SendTelegramParams>,
    ) -> Result<CallToolResult, McpError> {
        let dir = paths::outbox_dir(&self.data_dir);
        let _ = fs::create_dir_all(&dir);

        let filename = format!("{}.json", uuid::Uuid::new_v4());
        let path = dir.join(&filename);
        let payload = json!({"chat_id": p.chat_id, "text": p.text});

        fs::write(&path, serde_json::to_string_pretty(&payload).unwrap())
            .map_err(|e| McpError::internal_error(format!("Write failed: {}", e), None))?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Queued as {}",
            filename
        ))]))
    }

    /// Read user data (owner and paired users)
    #[tool(description = "Read user data: owner info and paired users")]
    fn read_users(&self) -> Result<CallToolResult, McpError> {
        let dir = paths::users_dir(&self.data_dir);
        let owner = self.read_file_safe(&dir.join("owner.json"));
        let paired = self.read_file_safe(&dir.join("paired.json"));

        let owner_json: serde_json::Value =
            serde_json::from_str(&owner).unwrap_or(json!(null));
        let paired_json: serde_json::Value =
            serde_json::from_str(&paired).unwrap_or(json!({}));

        let result = json!({
            "owner": owner_json,
            "paired": paired_json,
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap(),
        )]))
    }

    /// Search past conversations by text
    #[tool(description = "Search past conversations by text (substring match in semantic memory chunks)")]
    fn search_vecstore(
        &self,
        Parameters(p): Parameters<SearchVecstoreParams>,
    ) -> Result<CallToolResult, McpError> {
        let db_path = paths::vecstore_db(&self.data_dir);
        if !db_path.exists() {
            return Ok(CallToolResult::success(vec![Content::text(
                "Vecstore database not found.",
            )]));
        }

        let limit = p.limit.unwrap_or(10);

        let db = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| McpError::internal_error(format!("DB open failed: {}", e), None))?;

        let mut stmt = db
            .prepare(
                "SELECT text, role, user_id, ts FROM chunks \
                 WHERE text LIKE '%' || ?1 || '%' \
                 ORDER BY ts DESC LIMIT ?2",
            )
            .map_err(|e| McpError::internal_error(format!("Query failed: {}", e), None))?;

        let rows = stmt
            .query_map(rusqlite::params![p.query, limit], |row| {
                Ok(json!({
                    "text": row.get::<_, String>(0)?,
                    "role": row.get::<_, String>(1)?,
                    "user_id": row.get::<_, i64>(2)?,
                    "timestamp": row.get::<_, String>(3)?,
                }))
            })
            .map_err(|e| McpError::internal_error(format!("Query failed: {}", e), None))?;

        let results: Vec<serde_json::Value> = rows.filter_map(|r| r.ok()).collect();
        let output = serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".into());
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }
}

// ---------------------------------------------------------------------------
// Memory helpers (ported from sophia/src/memory.rs)
// ---------------------------------------------------------------------------

fn normalize_fact(text: &str) -> String {
    let re = Regex::new(r"\[\d{4}-\d{2}-\d{2}\s+\d{2}:\d{2}\s*\w*\]\s*").unwrap();
    let s = re.replace_all(text, "");
    s.trim().to_lowercase()
}

fn deduplicate_memory(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut non_entry_lines: Vec<String> = Vec::new();
    let mut entries: Vec<String> = Vec::new();

    for line in lines {
        let stripped = line.trim();
        if stripped.starts_with("- ") {
            entries.push(line.to_string());
        } else {
            if !entries.is_empty() {
                let deduped = keep_last_unique(&entries);
                non_entry_lines.extend(deduped);
                entries.clear();
            }
            non_entry_lines.push(line.to_string());
        }
    }
    if !entries.is_empty() {
        let deduped = keep_last_unique(&entries);
        non_entry_lines.extend(deduped);
    }

    non_entry_lines.join("\n")
}

fn keep_last_unique(entries: &[String]) -> Vec<String> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, line) in entries.iter().enumerate() {
        let fact = line.trim().strip_prefix("- ").unwrap_or("");
        let norm = normalize_fact(fact);
        if !norm.is_empty() {
            seen.insert(norm, i);
        }
    }
    let last_indices: std::collections::HashSet<usize> = seen.into_values().collect();
    entries
        .iter()
        .enumerate()
        .filter(|(i, _)| last_indices.contains(i))
        .map(|(_, line)| line.clone())
        .collect()
}
