use base64::Engine;
use serde::Serialize;
use tracing::warn;

use crate::types::{ContentBlock, ImageSource, Message, MessageContent};

/// A single input line for Claude CLI stream-json protocol.
#[derive(Debug, Serialize)]
pub struct StreamJsonInput {
    pub r#type: String,
    pub message: StreamJsonMessage,
}

#[derive(Debug, Serialize)]
pub struct StreamJsonMessage {
    pub role: String,
    pub content: Vec<CliContentBlock>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum CliContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: CliImageSource },
}

#[derive(Debug, Serialize)]
pub struct CliImageSource {
    pub r#type: &'static str,
    pub media_type: String,
    pub data: String,
}

/// Convert Anthropic messages to Claude CLI stream-json input lines.
/// Consecutive messages with the same role are merged (Claude CLI requires strict alternation).
pub async fn convert_messages(messages: &[Message]) -> Vec<StreamJsonInput> {
    let mut inputs: Vec<StreamJsonInput> = Vec::new();

    for msg in messages {
        let role = msg.role.as_str();
        if role != "user" && role != "assistant" {
            warn!("Skipping message with unsupported role: {role}");
            continue;
        }

        let blocks = convert_content(&msg.content).await;

        // Merge consecutive same-role messages
        if let Some(last) = inputs.last_mut() {
            if last.message.role == role {
                last.message.content.extend(blocks);
                continue;
            }
        }

        inputs.push(StreamJsonInput {
            r#type: role.to_string(),
            message: StreamJsonMessage {
                role: role.to_string(),
                content: blocks,
            },
        });
    }

    inputs
}

/// Convert Anthropic content (string or array of blocks) to CLI content blocks.
async fn convert_content(content: &MessageContent) -> Vec<CliContentBlock> {
    match content {
        MessageContent::Text(s) => vec![CliContentBlock::Text { text: s.clone() }],
        MessageContent::Blocks(blocks) => {
            let mut result = Vec::new();
            for block in blocks {
                match block {
                    ContentBlock::Text { text } => {
                        result.push(CliContentBlock::Text { text: text.clone() });
                    }
                    ContentBlock::Image { source } => {
                        if let Some(cli_block) = convert_image(source).await {
                            result.push(cli_block);
                        }
                    }
                }
            }
            result
        }
    }
}

/// Convert an Anthropic image source to a CLI image block.
/// Base64 sources pass through directly; URL sources are downloaded and encoded.
async fn convert_image(source: &ImageSource) -> Option<CliContentBlock> {
    match source {
        ImageSource::Base64 { media_type, data } => Some(CliContentBlock::Image {
            source: CliImageSource {
                r#type: "base64",
                media_type: media_type.clone(),
                data: data.clone(),
            },
        }),
        ImageSource::Url { url } => match download_and_encode(url).await {
            Ok(block) => Some(block),
            Err(e) => {
                warn!(
                    "Failed to download image {}: {e}",
                    &url[..url.len().min(80)]
                );
                None
            }
        },
    }
}

/// Download an HTTP URL and return it as a base64 CLI image block.
async fn download_and_encode(url: &str) -> Result<CliContentBlock, String> {
    let resp = reqwest::get(url)
        .await
        .map_err(|e| format!("HTTP download failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {} downloading image", resp.status()));
    }

    let media_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.split(';').next().unwrap_or("").trim().to_string())
        .unwrap_or_else(|| guess_mime(url));

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read response body: {e}"))?;

    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);

    Ok(CliContentBlock::Image {
        source: CliImageSource {
            r#type: "base64",
            media_type,
            data,
        },
    })
}

fn guess_mime(url: &str) -> String {
    let ext = url
        .rsplit('/')
        .next()
        .and_then(|f| f.split('?').next())
        .and_then(|f| f.rsplit('.').next())
        .unwrap_or("");

    match ext.to_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
    .to_string()
}
