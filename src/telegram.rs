use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use grammers_client::client::Client;
use grammers_client::media::Attribute;
use grammers_client::message::InputMessage;
use grammers_session::types::PeerRef;
use grammers_tl_types as tl;
use tracing::{debug, info, warn};

use crate::config;
use crate::format::md_to_tg_html;

const MAX_FILE_SIZE: u64 = 20 * 1024 * 1024; // 20 MB
pub const TG_MAX_CHARS: usize = 4096;
/// Streaming split threshold (slightly below limit to avoid edge cases).
pub const TG_STREAM_CHARS: usize = 3900;

/// Find byte offset for the given char count limit.
/// Returns the byte position at which `char_limit` characters end.
pub fn byte_offset_at_char(text: &str, char_limit: usize) -> usize {
    text.char_indices()
        .nth(char_limit)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

/// Number of characters (not bytes) in a string.
pub fn char_len(text: &str) -> usize {
    text.chars().count()
}

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

/// Edit an existing message with HTML formatting.
pub async fn edit_message(client: &Client, peer: PeerRef, msg_id: i32, text: &str) -> Result<()> {
    let html = md_to_tg_html(text);
    let truncated = if char_len(&html) > TG_MAX_CHARS {
        let end = byte_offset_at_char(&html, TG_MAX_CHARS);
        &html[..end]
    } else {
        &html
    };
    client
        .edit_message(peer, msg_id, InputMessage::new().html(truncated))
        .await?;
    Ok(())
}

/// Send a message with HTML formatting and return its ID.
/// Truncates at Telegram's 4096-char limit.
pub async fn send_and_get_id(client: &Client, peer: PeerRef, text: &str) -> Result<i32> {
    let html = md_to_tg_html(text);
    let truncated = if char_len(&html) > TG_MAX_CHARS {
        let end = byte_offset_at_char(&html, TG_MAX_CHARS);
        &html[..end]
    } else {
        &html
    };
    let msg = client
        .send_message(peer, InputMessage::new().html(truncated))
        .await?;
    Ok(msg.id())
}

/// Send message with HTML formatting, splitting at 4096 char Telegram limit.
pub async fn send_long(client: &Client, peer: PeerRef, text: &str) -> Result<()> {
    let html = md_to_tg_html(text);
    let mut remaining = html.as_str();
    while !remaining.is_empty() {
        if char_len(remaining) <= TG_MAX_CHARS {
            client
                .send_message(peer, InputMessage::new().html(remaining))
                .await?;
            break;
        }
        let byte_limit = byte_offset_at_char(remaining, TG_MAX_CHARS);
        let split_at = remaining[..byte_limit]
            .rfind('\n')
            .unwrap_or(byte_limit);
        client
            .send_message(peer, InputMessage::new().html(&remaining[..split_at]))
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

/// Generate TTS audio and send as a Telegram voice message.
/// Runs tts.sh to produce WAV, converts to OGG Opus via ffmpeg, uploads and sends.
pub async fn send_voice(client: &Client, peer: PeerRef, text: &str) -> Result<()> {
    let text = text.to_string();
    let ogg_path = tokio::task::spawn_blocking(move || -> Result<PathBuf> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let wav_path = format!("/tmp/sofia_voice_{}.wav", ts);
        let ogg_path = format!("/tmp/sofia_voice_{}.ogg", ts);

        // Generate WAV via Piper TTS
        let tts_script = config::project_root().join("scripts/tts.sh");
        let status = std::process::Command::new("bash")
            .arg(&tts_script)
            .arg(&text)
            .arg(&wav_path)
            .output()?;
        if !status.status.success() {
            anyhow::bail!(
                "TTS failed: {}",
                String::from_utf8_lossy(&status.stderr)
            );
        }
        if !Path::new(&wav_path).exists() {
            anyhow::bail!("TTS produced no output file");
        }

        // Convert WAV → OGG Opus via ffmpeg
        let ffmpeg = std::process::Command::new("ffmpeg")
            .args(["-y", "-i", &wav_path, "-c:a", "libopus", "-b:a", "64k", &ogg_path])
            .output()?;
        // Clean up WAV
        let _ = std::fs::remove_file(&wav_path);
        if !ffmpeg.status.success() {
            anyhow::bail!(
                "ffmpeg conversion failed: {}",
                String::from_utf8_lossy(&ffmpeg.stderr)
            );
        }

        Ok(PathBuf::from(ogg_path))
    })
    .await??;

    // Get audio duration via ffprobe
    let ogg_path_clone = ogg_path.clone();
    let duration_secs = tokio::task::spawn_blocking(move || -> u64 {
        let output = std::process::Command::new("ffprobe")
            .args([
                "-v", "error",
                "-show_entries", "format=duration",
                "-of", "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(&ogg_path_clone)
            .output();
        match output {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                s.trim().parse::<f64>().unwrap_or(1.0).ceil() as u64
            }
            Err(_) => 1,
        }
    })
    .await
    .unwrap_or(1);

    // Upload and send
    let uploaded = client.upload_file(&ogg_path).await?;
    let msg = InputMessage::new()
        .mime_type("audio/ogg")
        .document(uploaded)
        .attribute(Attribute::Voice {
            duration: Duration::from_secs(duration_secs),
            waveform: None,
        });
    client.send_message(peer, msg).await?;

    // Clean up OGG
    let _ = tokio::fs::remove_file(&ogg_path).await;

    info!("Sent voice message ({} sec)", duration_secs);
    Ok(())
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
