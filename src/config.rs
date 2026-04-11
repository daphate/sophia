use std::path::PathBuf;

use anyhow::{Context, Result};

/// Bot operation mode.
#[derive(Debug, Clone, PartialEq)]
pub enum BotMode {
    /// Regular Telegram bot via BOT_TOKEN.
    Bot { token: String },
    /// Userbot via phone number + API credentials.
    Userbot { phone_number: String },
}

/// Which role this binary plays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BotRole {
    Main,
    Rescue,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub api_id: i32,
    pub api_hash: String,
    pub mode: BotMode,
    pub owner_id: i64,
    pub claude_cli: String,
    /// OAuth token read once at startup from ~/.claude/tokens/ (annual or session).
    /// Passed to child `claude -p` via CLAUDE_CODE_OAUTH_TOKEN so they never
    /// touch the token files themselves — no race with interactive CLI.
    pub oauth_token: Option<String>,
    #[allow(dead_code)]
    pub inference_timeout: u64,
    pub session_name: String,
    pub exec_enabled: bool,
    pub exec_allowed_commands: Vec<String>,
    /// Update check interval in hours. 0 = disabled.
    pub update_check_hours: u64,
    /// Automatically pull, rebuild and restart on new version.
    pub auto_update: bool,
    /// Which role this instance plays.
    pub role: BotRole,
    /// launchd service label of the *peer* bot (for /status, /restart, watchdog).
    pub peer_service: String,
}

impl Config {
    pub fn is_bot(&self) -> bool {
        matches!(self.mode, BotMode::Bot { .. })
    }

    /// Load config for the main bot (reads BOT_TOKEN or PHONE_NUMBER).
    pub fn from_env() -> Result<Self> {
        let mut cfg = Self::load_common()?;
        cfg.role = BotRole::Main;
        cfg.peer_service = "com.sophia.rescue".into();

        let bot_token = std::env::var("BOT_TOKEN").ok();
        let phone_number = std::env::var("PHONE_NUMBER").ok();
        cfg.mode = match (bot_token, phone_number) {
            (Some(token), _) => BotMode::Bot { token },
            (None, Some(phone)) => BotMode::Userbot { phone_number: phone },
            (None, None) => anyhow::bail!("Either BOT_TOKEN or PHONE_NUMBER must be set"),
        };

        if cfg.session_name == "sophia" || cfg.session_name.is_empty() {
            cfg.session_name = "sophia".into();
        }

        Ok(cfg)
    }

    /// Load config for the rescue bot (reads RESCUE_BOT_TOKEN).
    pub fn from_env_rescue() -> Result<Self> {
        let mut cfg = Self::load_common()?;
        cfg.role = BotRole::Rescue;
        cfg.peer_service = "com.sophia.bot".into();

        let token = std::env::var("RESCUE_BOT_TOKEN")
            .context("RESCUE_BOT_TOKEN not set")?;
        cfg.mode = BotMode::Bot { token };
        cfg.session_name = "rescue".into();

        Ok(cfg)
    }

    /// Shared config loading (everything except mode/role).
    fn load_common() -> Result<Self> {
        dotenvy::dotenv().ok();

        let api_id: i32 = std::env::var("API_ID")
            .context("API_ID not set")?
            .parse()
            .context("API_ID must be an integer")?;
        let api_hash = std::env::var("API_HASH").context("API_HASH not set")?;
        let owner_id: i64 = std::env::var("OWNER_ID")
            .context("OWNER_ID not set")?
            .parse()
            .context("OWNER_ID must be an integer")?;

        let claude_cli = std::env::var("CLAUDE_CLI").unwrap_or_else(|_| "claude".into());
        let oauth_token = read_oauth_token();
        let inference_timeout: u64 = std::env::var("INFERENCE_TIMEOUT")
            .unwrap_or_else(|_| "150".into())
            .parse()
            .unwrap_or(150);
        let session_name = std::env::var("SESSION_NAME").unwrap_or_else(|_| "sophia".into());
        let exec_enabled = std::env::var("EXEC_ENABLED")
            .unwrap_or_else(|_| "true".into())
            .to_lowercase()
            == "true";
        let exec_allowed_commands: Vec<String> = std::env::var("EXEC_ALLOWED_COMMANDS")
            .unwrap_or_else(|_| {
                "cat,echo,ls,pwd,date,whoami,uname,head,tail,wc,df,free,uptime,tee".into()
            })
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let update_check_hours: u64 = std::env::var("UPDATE_CHECK_HOURS")
            .unwrap_or_else(|_| "12".into())
            .parse()
            .unwrap_or(12);
        let auto_update = std::env::var("AUTO_UPDATE")
            .unwrap_or_else(|_| "false".into())
            .to_lowercase()
            == "true";

        Ok(Self {
            api_id,
            api_hash,
            mode: BotMode::Bot { token: String::new() }, // placeholder, overwritten by caller
            owner_id,
            claude_cli,
            oauth_token,
            inference_timeout,
            session_name,
            exec_enabled,
            exec_allowed_commands,
            update_check_hours,
            auto_update,
            role: BotRole::Main,       // overwritten by caller
            peer_service: String::new(), // overwritten by caller
        })
    }
}

/// Read OAuth token from ~/.claude/tokens/ once at startup.
/// Prefers "annual" (long-lived), falls back to "session".
fn read_oauth_token() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let base = std::path::Path::new(&home).join(".claude/tokens");
    for name in &["annual", "session"] {
        if let Ok(token) = std::fs::read_to_string(base.join(name)) {
            let token = token.trim().to_string();
            if !token.is_empty() {
                return Some(token);
            }
        }
    }
    None
}

/// Project root: current working directory.
pub fn project_root() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn data_dir() -> PathBuf {
    project_root().join("data")
}

pub fn instructions_dir() -> PathBuf {
    data_dir().join("instructions")
}

pub fn memory_dir() -> PathBuf {
    data_dir().join("memory")
}

pub fn dialogs_dir() -> PathBuf {
    data_dir().join("dialogs")
}

pub fn users_dir() -> PathBuf {
    data_dir().join("users")
}

pub fn files_dir() -> PathBuf {
    data_dir().join("files")
}

pub fn owner_file() -> PathBuf {
    users_dir().join("owner.json")
}

pub fn paired_file() -> PathBuf {
    users_dir().join("paired.json")
}

pub fn pending_file() -> PathBuf {
    users_dir().join("pending.json")
}

pub fn memory_file() -> PathBuf {
    memory_dir().join("MEMORY.md")
}

pub fn agents_file() -> PathBuf {
    instructions_dir().join("AGENTS.md")
}

pub fn soul_file() -> PathBuf {
    instructions_dir().join("SOUL.md")
}

pub fn user_file() -> PathBuf {
    instructions_dir().join("USER.md")
}

pub fn identity_file() -> PathBuf {
    instructions_dir().join("IDENTITY.md")
}

pub fn tools_file() -> PathBuf {
    instructions_dir().join("TOOLS.md")
}

pub fn instructions_memory_file() -> PathBuf {
    instructions_dir().join("MEMORY.md")
}

/// Queue DB path — separate per role to avoid double-processing.
pub fn queue_db_for(role: BotRole) -> PathBuf {
    match role {
        BotRole::Main => data_dir().join("queue.db"),
        BotRole::Rescue => data_dir().join("queue_rescue.db"),
    }
}

/// Default queue DB (main bot). Kept for backward compat.
pub fn queue_db() -> PathBuf {
    queue_db_for(BotRole::Main)
}
