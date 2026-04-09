use std::path::PathBuf;

use anyhow::{Context, Result};

/// Rescue bot configuration — loaded from sophia's .env
pub struct Config {
    pub api_id: i32,
    pub api_hash: String,
    pub bot_token: String,
    pub owner_id: i64,
    pub sophia_root: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        // Load .env from sophia project root
        let sophia_root = PathBuf::from(
            std::env::var("SOPHIA_ROOT").unwrap_or_else(|_| "/Users/lokitheone/sophia".into()),
        );
        let env_path = sophia_root.join(".env");
        if env_path.exists() {
            dotenvy::from_path(&env_path).ok();
        }

        let api_id: i32 = std::env::var("API_ID")
            .context("API_ID not set")?
            .parse()
            .context("API_ID must be an integer")?;

        let api_hash = std::env::var("API_HASH").context("API_HASH not set")?;

        let bot_token =
            std::env::var("RESCUE_BOT_TOKEN").context("RESCUE_BOT_TOKEN not set")?;

        let owner_id: i64 = std::env::var("OWNER_ID")
            .context("OWNER_ID not set")?
            .parse()
            .context("OWNER_ID must be an integer")?;

        Ok(Self {
            api_id,
            api_hash,
            bot_token,
            owner_id,
            sophia_root,
        })
    }
}
