#![allow(dead_code)]

use serde::{Deserialize, Serialize};

// ── Anthropic Messages API request types ──

#[derive(Debug, Deserialize)]
pub struct MessagesRequest {
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub system: Option<SystemPrompt>,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub stream: bool,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<u32>,
    pub stop_sequences: Option<Vec<String>>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum SystemPrompt {
    Text(String),
    Blocks(Vec<SystemBlock>),
}

#[derive(Debug, Deserialize, Clone)]
pub struct SystemBlock {
    pub r#type: String,
    pub text: String,
}

impl SystemPrompt {
    pub fn as_text(&self) -> String {
        match self {
            SystemPrompt::Text(s) => s.clone(),
            SystemPrompt::Blocks(blocks) => blocks
                .iter()
                .map(|b| b.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n"),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Message {
    pub role: String,
    pub content: MessageContent,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    pub fn as_text(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: ImageSource },
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "type")]
pub enum ImageSource {
    #[serde(rename = "base64")]
    Base64 { media_type: String, data: String },
    #[serde(rename = "url")]
    Url { url: String },
}

// ── Anthropic Messages API response types ──

#[derive(Debug, Serialize)]
pub struct MessagesResponse {
    pub id: String,
    pub r#type: &'static str,
    pub role: &'static str,
    pub content: Vec<ResponseContentBlock>,
    pub model: String,
    pub stop_reason: &'static str,
    pub stop_sequence: Option<String>,
    pub usage: Usage,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum ResponseContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
}

#[derive(Debug, Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

// ── Streaming event types ──

#[derive(Debug, Serialize)]
pub struct MessageStartEvent {
    pub r#type: &'static str,
    pub message: MessageStartBody,
}

#[derive(Debug, Serialize)]
pub struct MessageStartBody {
    pub id: String,
    pub r#type: &'static str,
    pub role: &'static str,
    pub content: [(); 0],
    pub model: String,
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
    pub usage: Usage,
}

#[derive(Debug, Serialize)]
pub struct ContentBlockStartEvent {
    pub r#type: &'static str,
    pub index: u32,
    pub content_block: ContentBlockRef,
}

#[derive(Debug, Serialize)]
pub struct ContentBlockRef {
    pub r#type: &'static str,
    pub text: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ContentBlockDeltaEvent {
    pub r#type: &'static str,
    pub index: u32,
    pub delta: TextDelta,
}

#[derive(Debug, Serialize)]
pub struct TextDelta {
    pub r#type: &'static str,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct ContentBlockStopEvent {
    pub r#type: &'static str,
    pub index: u32,
}

#[derive(Debug, Serialize)]
pub struct MessageDeltaEvent {
    pub r#type: &'static str,
    pub delta: MessageDeltaBody,
    pub usage: Usage,
}

#[derive(Debug, Serialize)]
pub struct MessageDeltaBody {
    pub stop_reason: &'static str,
    pub stop_sequence: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MessageStopEvent {
    pub r#type: &'static str,
}

// ── Error response (Anthropic format) ──

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub r#type: &'static str,
    pub error: ApiError,
}

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub r#type: String,
    pub message: String,
}

// ── Models endpoint (kept for convenience) ──

#[derive(Debug, Serialize)]
pub struct ModelsResponse {
    pub object: &'static str,
    pub data: Vec<ModelInfo>,
}

#[derive(Debug, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub owned_by: String,
}

// ── Claude CLI stream-json types ──

#[derive(Debug, Deserialize)]
pub struct ClaudeEvent {
    pub r#type: String,
    pub subtype: Option<String>,
    pub message: Option<ClaudeMessage>,
    pub result: Option<String>,
    pub total_cost_usd: Option<f64>,
    pub usage: Option<ClaudeUsage>,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Debug, Deserialize)]
pub struct ClaudeMessage {
    pub content: Option<Vec<ClaudeContentBlock>>,
    pub usage: Option<ClaudeMessageUsage>,
    pub stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ClaudeContentBlock {
    pub r#type: String,
    pub text: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ClaudeMessageUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ClaudeUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}
