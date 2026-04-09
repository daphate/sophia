use std::path::PathBuf;

/// Rescue bot configuration — loaded from sophia's .env
pub struct Config {
    pub bot_token: String,
    pub owner_id: i64,
    pub sophia_root: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        // Load .env from sophia project root
        let sophia_root = PathBuf::from(
            std::env::var("SOPHIA_ROOT")
                .unwrap_or_else(|_| "/Users/lokitheone/sophia".into()),
        );
        let env_path = sophia_root.join(".env");
        if env_path.exists() {
            dotenvy::from_path(&env_path).ok();
        }

        let bot_token = std::env::var("RESCUE_BOT_TOKEN")
            .map_err(|_| "RESCUE_BOT_TOKEN not set")?;

        let owner_id: i64 = std::env::var("OWNER_ID")
            .map_err(|_| "OWNER_ID not set")?
            .parse()
            .map_err(|_| "OWNER_ID must be an integer")?;

        Ok(Self {
            bot_token,
            owner_id,
            sophia_root,
        })
    }

    pub fn api_url(&self, method: &str) -> String {
        format!("https://api.telegram.org/bot{}/{}", self.bot_token, method)
    }
}
