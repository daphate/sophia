use serde::{Deserialize, Serialize};

use crate::config::Config;

const TG_MAX_CHARS: usize = 4096;

// ── Telegram Bot API types (minimal) ──────────────────────────────────

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct TgResponse<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<Message>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Message {
    pub message_id: i64,
    pub from: Option<User>,
    pub chat: Chat,
    pub text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct User {
    pub id: i64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Chat {
    pub id: i64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct BotUser {
    pub id: i64,
    pub first_name: String,
    pub username: Option<String>,
}

#[derive(Serialize)]
struct SendMessageBody {
    chat_id: i64,
    text: String,
    parse_mode: Option<String>,
}

// ── API client ────────────────────────────────────────────────────────

pub struct TgClient {
    http: reqwest::Client,
    config: Config,
}

impl TgClient {
    pub fn new(config: Config) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("Failed to create HTTP client");
        Self { http, config }
    }

    pub fn owner_id(&self) -> i64 {
        self.config.owner_id
    }

    #[allow(dead_code)]
    pub fn sophia_root(&self) -> &std::path::Path {
        &self.config.sophia_root
    }

    /// Validate bot token on startup.
    pub async fn get_me(&self) -> Result<BotUser, String> {
        let resp: TgResponse<BotUser> = self
            .http
            .get(self.config.api_url("getMe"))
            .send()
            .await
            .map_err(|e| format!("getMe request failed: {e}"))?
            .json()
            .await
            .map_err(|e| format!("getMe parse failed: {e}"))?;

        resp.result.ok_or_else(|| {
            format!(
                "getMe failed: {}",
                resp.description.unwrap_or_default()
            )
        })
    }

    /// Long-poll for updates.
    pub async fn get_updates(&self, offset: i64) -> Result<Vec<Update>, String> {
        let url = format!(
            "{}?offset={}&timeout=30&allowed_updates=[\"message\"]",
            self.config.api_url("getUpdates"),
            offset
        );

        let resp: TgResponse<Vec<Update>> = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("getUpdates failed: {e}"))?
            .json()
            .await
            .map_err(|e| format!("getUpdates parse failed: {e}"))?;

        Ok(resp.result.unwrap_or_default())
    }

    /// Send a text message, splitting if >4096 chars.
    pub async fn send_message(&self, chat_id: i64, text: &str) {
        if text.is_empty() {
            return;
        }

        // Split into chunks respecting char limit
        let mut remaining = text;
        while !remaining.is_empty() {
            let chunk = if remaining.len() <= TG_MAX_CHARS {
                remaining
            } else {
                // Find a good split point
                let boundary = remaining[..TG_MAX_CHARS]
                    .rfind('\n')
                    .unwrap_or(TG_MAX_CHARS);
                &remaining[..boundary]
            };

            let body = SendMessageBody {
                chat_id,
                text: chunk.to_string(),
                parse_mode: None,
            };

            let _ = self
                .http
                .post(self.config.api_url("sendMessage"))
                .json(&body)
                .send()
                .await;

            remaining = &remaining[chunk.len()..];
            // Strip leading newline after split
            remaining = remaining.strip_prefix('\n').unwrap_or(remaining);
        }
    }
}
