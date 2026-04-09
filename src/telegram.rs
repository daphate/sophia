use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use grammers_client::client::Client;
use grammers_client::message::InputMessage;
use grammers_session::types::PeerRef;
use grammers_tl_types as tl;
use tracing::{debug, info, warn};

use crate::config;

const MAX_FILE_SIZE: u64 = 20 * 1024 * 1024; // 20 MB
const TG_MAX_LEN: usize = 4096;

/// Set a reaction emoji on a message.
pub async fn react(client: &Client, peer: PeerRef, msg_id: i32, emoji: &str) {
    let result = client
        .invoke(&tl::functions::messages::SendReaction {
            peer: peer.into(),
            msg_id,
            reaction: Some(vec![tl::enums::Reaction::Emoji(
                tl::types::ReactionEmoji {
                    emoticon: emoji.to_string(),
                },
            )]),
            big: false,
            add_to_recent: false,
        })
        .await;

    if let Err(e) = result {
        debug!("Failed to set reaction {}: {}", emoji, e);
    }
}

/// Send message, splitting at 4096 char Telegram limit.
pub async fn send_long(client: &Client, peer: PeerRef, text: &str) -> Result<()> {
    let mut remaining = text;
    while !remaining.is_empty() {
        if remaining.len() <= TG_MAX_LEN {
            client
                .send_message(peer, InputMessage::new().text(remaining))
                .await?;
            break;
        }
        let split_at = remaining[..TG_MAX_LEN]
            .rfind('\n')
            .unwrap_or(TG_MAX_LEN);
        client
            .send_message(peer, InputMessage::new().text(&remaining[..split_at]))
            .await?;
        remaining = remaining[split_at..].trim_start_matches('\n');
    }
    Ok(())
}

/// Download media from a message. Returns list of saved file paths.
pub async fn download_media(
    client: &Client,
    message: &grammers_client::message::Message,
    sender_id: i64,
) -> Result<Vec<PathBuf>> {
    let media = match message.media() {
        Some(m) => m,
        None => return Ok(vec![]),
    };

    // Create user directory
    let user_dir = config::files_dir().join(sender_id.to_string());
    std::fs::create_dir_all(&user_dir)?;

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let filename = format!("{}_file_{}", ts, message.id());
    let dest = user_dir.join(&filename);

    match client.download_media(&media, &dest).await {
        Ok(_) => {
            if let Ok(meta) = std::fs::metadata(&dest) {
                if meta.len() > MAX_FILE_SIZE {
                    warn!("Downloaded file too large ({} bytes), removing", meta.len());
                    let _ = std::fs::remove_file(&dest);
                    return Ok(vec![]);
                }
                info!("Downloaded file: {} ({} bytes)", dest.display(), meta.len());
            }
            Ok(vec![dest])
        }
        Err(e) => {
            warn!("Failed to download media: {}", e);
            Ok(vec![])
        }
    }
}

/// Send typing indicator.
pub async fn send_typing(client: &Client, peer: PeerRef) {
    let _ = client
        .invoke(&tl::functions::messages::SetTyping {
            peer: peer.into(),
            top_msg_id: None,
            action: tl::enums::SendMessageAction::SendMessageTypingAction,
        })
        .await;
}
