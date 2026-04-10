use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::Result;
use dashmap::DashMap;
use grammers_client::client::Client;
use grammers_client::update::Update;
use grammers_session::types::{PeerId, PeerKind, PeerRef};
use regex::Regex;
use tokio::sync::{broadcast, Mutex};
use tracing::{error, info, warn};

use crate::config::Config;
use crate::inference;
use crate::memory;
use crate::pairing;
use crate::queue::MessageQueue;
use crate::telegram;
use crate::update_check::{self, UpdateState};

use crate::vecstore::VecStore;

/// Shared state for per-user locks.
pub type UserLocks = Arc<DashMap<i64, Arc<Mutex<()>>>>;

pub fn new_user_locks() -> UserLocks {
    Arc::new(DashMap::new())
}

fn get_lock(locks: &UserLocks, user_id: i64) -> Arc<Mutex<()>> {
    locks
        .entry(user_id)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

pub async fn handle_update(
    client: &Client,
    update: Update,
    config: &Config,
    me_id: PeerId,
    queue: &MessageQueue,
    user_locks: &UserLocks,
    update_state: &UpdateState,
    shutdown_tx: &broadcast::Sender<()>,
    vecstore: &Arc<VecStore>,
) -> Result<()> {
    match update {
        Update::NewMessage(message) => {
            if message.outgoing() {
                return Ok(());
            }
            // Only handle private (user) messages
            let peer_kind = message.peer_id().kind();
            if !matches!(peer_kind, PeerKind::User | PeerKind::UserSelf) {
                return Ok(());
            }
            let peer = match message.peer_ref().await {
                Some(p) => p,
                None => {
                    info!("Could not resolve peer_ref for {:?}, skipping", message.peer_id());
                    return Ok(());
                }
            };

            let sender_peer_id = match message.sender_id() {
                Some(id) => id,
                None => return Ok(()),
            };
            if sender_peer_id == me_id {
                return Ok(());
            }
            let sender_id = sender_peer_id.bare_id();
            if sender_id == 0 {
                return Ok(());
            }

            handle_private_message(
                client, &message, config, sender_id, peer, queue, user_locks,
                update_state, shutdown_tx, vecstore,
            )
            .await
        }
        _ => Ok(()),
    }
}

async fn handle_private_message(
    client: &Client,
    message: &grammers_client::update::Message,
    config: &Config,
    sender_id: i64,
    peer: PeerRef,
    queue: &MessageQueue,
    user_locks: &UserLocks,
    update_state: &UpdateState,
    shutdown_tx: &broadcast::Sender<()>,
    vecstore: &Arc<VecStore>,
) -> Result<()> {
    // Skip old messages from catch_up replay (older than 5 minutes)
    let msg_age = chrono::Utc::now().signed_duration_since(message.date());
    if msg_age.num_seconds() > 300 {
        info!(
            "Skipping stale message from {} (age={}s, msg_id={})",
            sender_id,
            msg_age.num_seconds(),
            message.id()
        );
        return Ok(());
    }

    let text = message.text().trim().to_string();
    let has_media = message.media().is_some();

    if text.is_empty() && !has_media {
        return Ok(());
    }

    let is_owner = sender_id == config.owner_id;
    let msg_id = message.id();

    // --- Command dispatch ---
    if !text.is_empty() && text.starts_with('/') {
        let parts: Vec<&str> = text.splitn(2, ' ').collect();
        let cmd = parts[0].to_lowercase();
        let arg = if parts.len() > 1 {
            parts[1].trim()
        } else {
            ""
        };

        match cmd.as_str() {
            "/pair" => return handle_pair(client, peer, sender_id, is_owner, config).await,
            "/approve" if is_owner => return handle_approve(client, peer, arg).await,
            "/deny" if is_owner => return handle_deny(client, peer, arg).await,
            "/unpair" if is_owner => return handle_unpair(client, peer, arg).await,
            "/exec" if is_owner => return handle_exec(client, peer, arg, config).await,
            "/memory" if is_owner => return handle_memory(client, peer, arg).await,
            "/update" if is_owner => {
                return handle_update_cmd(client, peer, update_state, shutdown_tx).await;
            }
            "/search" if is_owner => return handle_search(client, peer, arg, vecstore).await,
            "/reindex" if is_owner => return handle_reindex(client, peer, vecstore).await,
            "/help" if is_owner || pairing::is_paired(sender_id) => {
                return handle_help(client, peer, is_owner).await;
            }
            _ => {}
        }
    }

    // --- Access check ---
    if !is_owner && !pairing::is_paired(sender_id) {
        telegram::send_long(
            client,
            peer,
            "I don't know you yet. Send /pair to request access.",
        )
        .await?;
        return Ok(());
    }

    // --- Download media ---
    let mut file_paths: Vec<PathBuf> = Vec::new();
    let mut text = text;
    if has_media {
        file_paths = telegram::download_media(client, message, sender_id).await?;
        if text.is_empty() && !file_paths.is_empty() {
            text = "Пользователь отправил файл. Прочитай и опиши его содержимое.".to_string();
        }
    }
    if text.is_empty() {
        return Ok(());
    }

    // --- Enqueue for debounce batching ---
    let file_paths_str = file_paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let chat_id = peer.id.bare_id();
    let (_id, is_dup) = {
        let queue_clone = queue.clone();
        let text_clone = text.clone();
        let fp_clone = file_paths_str.clone();
        tokio::task::spawn_blocking(move || {
            queue_clone.enqueue(sender_id, chat_id, msg_id, &text_clone, &fp_clone)
        }).await??
    };
    if is_dup {
        info!("Duplicate message msg_id={} from {}, skipping", msg_id, sender_id);
        return Ok(());
    }
    info!("Message from {} enqueued (msg_id={}), debounce 2s", sender_id, msg_id);

    // Debounce: wait 2s for more messages from the same user
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Acquire per-user lock — serializes batch processing per user
    let lock = get_lock(user_locks, sender_id);
    let _guard = lock.lock().await;

    // Take all pending messages for this user+chat
    let msgs = {
        let queue_clone = queue.clone();
        tokio::task::spawn_blocking(move || {
            queue_clone.take_batch(sender_id, chat_id)
        }).await??
    };
    if msgs.is_empty() {
        // Another handler already processed our messages
        return Ok(());
    }

    // Combine texts and file paths from the batch
    let combined_text = msgs
        .iter()
        .map(|m| m.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let all_file_paths: Vec<PathBuf> = msgs
        .iter()
        .flat_map(|m| {
            m.file_paths
                .split('\n')
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        })
        .collect();
    let last_msg_id = msgs.last().unwrap().msg_id;
    let queue_ids: Vec<i64> = msgs.iter().map(|m| m.id).collect();

    info!(
        "Processing batch of {} message(s) from {} (react on msg_id={})",
        msgs.len(),
        sender_id,
        last_msg_id
    );

    // React only on the LAST message
    telegram::react(client, peer, last_msg_id, "🫡").await;

    let result = process_message(
        client, config, peer, last_msg_id, sender_id, &combined_text, &all_file_paths, vecstore,
    )
    .await;

    // Mark queue entries done/failed
    let is_ok = result.is_ok();
    {
        let queue_clone = queue.clone();
        tokio::task::spawn_blocking(move || {
            for id in queue_ids {
                if is_ok {
                    if let Err(e) = queue_clone.mark_done(id) {
                        error!("Failed to mark message {} as done: {}", id, e);
                    }
                } else {
                    if let Err(e) = queue_clone.mark_failed(id) {
                        error!("Failed to mark message {} as failed: {}", id, e);
                    }
                }
            }
        }).await.ok();
    }

    result
}

async fn process_message(
    client: &Client,
    config: &Config,
    peer: PeerRef,
    msg_id: i32,
    sender_id: i64,
    text: &str,
    file_paths: &[PathBuf],
    vecstore: &Arc<VecStore>,
) -> Result<()> {
    // Log user message and index it in vecstore
    {
        let text_clone = text.to_string();
        let vs = Arc::clone(vecstore);
        let ts = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        tokio::task::spawn_blocking(move || {
            memory::append_dialog(sender_id, "User", &text_clone);
            if let Err(e) = vs.add(&text_clone, "User", sender_id, &ts) {
                warn!("Failed to index user message: {}", e);
            }
        })
        .await?;
    }

    // Animated thinking reaction: cycle emojis every 5s so user sees we're alive
    telegram::react(client, peer, msg_id, "🤔").await;
    let thinking_client = client.clone();
    let thinking_peer = peer;
    let thinking_msg_id = msg_id;
    let thinking_task = tokio::spawn(async move {
        const THINKING_EMOJIS: &[&str] = &["🤔", "🧐", "🤨", "💭", "🫠"];
        let mut idx = 1; // start from 1 since we already set 🤔
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let emoji = THINKING_EMOJIS[idx % THINKING_EMOJIS.len()];
            telegram::react(&thinking_client, thinking_peer, thinking_msg_id, emoji).await;
            idx += 1;
        }
    });

    // Send typing indicator, keep refreshing it while inference runs
    telegram::send_typing(client, peer).await;

    // Background task: refresh typing every 8s (Telegram typing expires after ~10s)
    let typing_client = client.clone();
    let typing_peer = peer;
    let typing_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(8)).await;
            telegram::send_typing(&typing_client, typing_peer).await;
        }
    });

    // Semantic search for relevant past context
    let semantic_context = {
        let query = text.to_string();
        let vs = Arc::clone(vecstore);
        tokio::task::spawn_blocking(move || {
            match vs.search(&query, Some(15)) {
                Ok(results) => {
                    // Filter out exact matches (current message) and low-relevance
                    let relevant: Vec<_> = results
                        .into_iter()
                        .filter(|r| r.score > 0.3 && r.score < 0.99)
                        .take(10)
                        .collect();
                    if relevant.is_empty() {
                        String::new()
                    } else {
                        info!("Semantic search: {} relevant results (top score={:.3})", relevant.len(), relevant[0].score);
                        crate::vecstore::format_search_context(&relevant, 3000)
                    }
                }
                Err(e) => {
                    warn!("Semantic search failed: {}", e);
                    String::new()
                }
            }
        })
        .await
        .unwrap_or_default()
    };

    // Call Claude with streaming
    let paths = if file_paths.is_empty() {
        None
    } else {
        Some(file_paths)
    };
    let mut stream = match inference::ask_claude_streaming(
        sender_id, text, config, paths, &semantic_context,
    ).await {
        Ok(rx) => rx,
        Err(e) => {
            thinking_task.abort();
            typing_task.abort();
            telegram::react(client, peer, msg_id, "🥶").await;
            error!("Inference failed for msg {}: {}", msg_id, e);
            telegram::send_long(client, peer, "Произошла ошибка при обработке запроса.").await?;
            return Err(anyhow::anyhow!("Inference failed: {}", e));
        }
    };

    // Stream response to Telegram: send first chunk, then edit message as text grows
    let mut accumulated = String::new();
    let mut sent_msg_id: Option<i32> = None;
    let mut last_edit = tokio::time::Instant::now();
    let mut response = String::new();
    let mut cost_info: Option<inference::CostInfo> = None;
    let edit_interval = std::time::Duration::from_millis(1500);
    // Track how much text we've already sent in previous (completed) messages
    let mut sent_in_previous_msgs = 0usize;

    while let Some(event) = stream.recv().await {
        match event {
            inference::StreamEvent::TextDelta(delta) => {
                accumulated.push_str(&delta);

                // Skip leading whitespace for first send
                if sent_msg_id.is_none() && accumulated.trim().is_empty() {
                    continue;
                }

                let display_text = if sent_in_previous_msgs > 0 {
                    let safe_offset = accumulated.ceil_char_boundary(sent_in_previous_msgs);
                    &accumulated[safe_offset..]
                } else {
                    &accumulated
                };

                let now = tokio::time::Instant::now();
                let should_update = now.duration_since(last_edit) >= edit_interval
                    && !display_text.trim().is_empty();

                if should_update {
                    thinking_task.abort(); // stop cycling thinking emojis
                    typing_task.abort(); // stop typing once we start showing text
                    let trimmed = display_text.trim_start().to_string();
                    match sent_msg_id {
                        None => {
                            // First chunk — send new message
                            match telegram::send_and_get_id(client, peer, &trimmed).await {
                                Ok(id) => {
                                    sent_msg_id = Some(id);
                                    telegram::react(client, peer, msg_id, "🧑‍💻").await;
                                }
                                Err(e) => {
                                    error!("Failed to send streaming message: {}", e);
                                }
                            }
                        }
                        Some(edit_id) => {
                            // Check if we're exceeding Telegram limit — need to send new msg
                            if telegram::char_len(&trimmed) > telegram::TG_STREAM_CHARS {
                                // Finalize current message with text up to last newline
                                let byte_limit = telegram::byte_offset_at_char(&trimmed, telegram::TG_STREAM_CHARS);
                                let split_at = trimmed[..byte_limit]
                                    .rfind('\n')
                                    .unwrap_or(byte_limit);
                                let final_chunk = &trimmed[..split_at];
                                let _ = telegram::edit_message(client, peer, edit_id, final_chunk)
                                    .await;
                                // Compute absolute position in `accumulated`.
                                // display_text starts at `display_start` in accumulated;
                                // trimmed is display_text with leading whitespace stripped;
                                // split_at is a byte offset within trimmed.
                                let display_start = accumulated.len() - display_text.len();
                                let leading_ws = display_text.len() - trimmed.len();
                                let abs_split = display_start + leading_ws + split_at;
                                // Skip newlines right after the split point
                                let safe_offset = accumulated.ceil_char_boundary(abs_split);
                                let remainder = accumulated[safe_offset..]
                                    .trim_start_matches('\n')
                                    .to_string();
                                sent_in_previous_msgs = accumulated.len() - remainder.len();
                                // Send new message with remainder
                                if !remainder.trim().is_empty() {
                                    match telegram::send_and_get_id(client, peer, &remainder).await
                                    {
                                        Ok(id) => sent_msg_id = Some(id),
                                        Err(e) => error!("Failed to send overflow msg: {}", e),
                                    }
                                } else {
                                    sent_msg_id = None;
                                }
                            } else {
                                let _ =
                                    telegram::edit_message(client, peer, edit_id, &trimmed).await;
                            }
                        }
                    }
                    last_edit = now;
                }
            }
            inference::StreamEvent::Done {
                full_text,
                cost,
            } => {
                response = full_text;
                cost_info = cost;
            }
            inference::StreamEvent::Error(e) => {
                thinking_task.abort();
                typing_task.abort();
                telegram::react(client, peer, msg_id, "🥶").await;
                error!("Streaming error for msg {}: {}", msg_id, e);
                if sent_msg_id.is_none() {
                    let _ = telegram::send_long(client, peer, "Произошла ошибка при обработке запроса.")
                        .await;
                }
                return Err(anyhow::anyhow!("Streaming error: {}", e));
            }
        }
    }

    thinking_task.abort();
    typing_task.abort();

    // Extract and save memory updates
    let (cleaned, updates) = memory::extract_memory_updates(&response);
    if !updates.is_empty() {
        tokio::task::spawn_blocking(move || {
            for update in &updates {
                memory::append_memory(update);
            }
        })
        .await.unwrap_or_else(|e| {
            warn!("Failed to save memory updates: {}", e);
        });
    }

    if let Some(cost) = &cost_info {
        info!(
            "Inference cost for {}: in={} out={} usd={:?}",
            sender_id, cost.input_tokens, cost.output_tokens, cost.cost_usd
        );
    }

    // Final edit/send with the cleaned (memory-stripped) text.
    // sent_in_previous_msgs tracks position in `accumulated`, but `cleaned` may be
    // shorter if memory tags were removed. Recalculate by finding how much of the
    // original `accumulated` prefix maps to `cleaned`.
    let display_final = if sent_in_previous_msgs > 0 && sent_in_previous_msgs < accumulated.len() {
        // Find the text we already sent (from accumulated) and locate its end in cleaned.
        let safe_acc = accumulated.ceil_char_boundary(sent_in_previous_msgs);
        let already_sent = &accumulated[..safe_acc];
        // If cleaned starts with the same prefix, use the same byte offset
        if cleaned.len() >= already_sent.len() && cleaned.starts_with(already_sent) {
            // safe_acc is valid in cleaned because cleaned starts with already_sent
            cleaned[safe_acc..].trim_start_matches('\n')
        } else {
            // Memory tags were removed from the already-sent portion or text diverged.
            // Count chars we already sent and skip that many chars in cleaned.
            let chars_sent = accumulated[..safe_acc].chars().count();
            let cleaned_offset = telegram::byte_offset_at_char(&cleaned, chars_sent);
            let safe_offset = cleaned.ceil_char_boundary(cleaned_offset.min(cleaned.len()));
            if safe_offset >= cleaned.len() {
                ""
            } else {
                cleaned[safe_offset..].trim_start_matches('\n')
            }
        }
    } else {
        cleaned.trim_start()
    };

    match sent_msg_id {
        Some(edit_id) => {
            // Edit with final cleaned text (may differ from accumulated if memory tags removed)
            if telegram::char_len(display_final) <= telegram::TG_MAX_CHARS {
                let _ = telegram::edit_message(client, peer, edit_id, display_final).await;
            } else {
                // Need to split — edit current and send rest
                let byte_limit = telegram::byte_offset_at_char(display_final, telegram::TG_MAX_CHARS);
                let split_at = display_final[..byte_limit]
                    .rfind('\n')
                    .unwrap_or(byte_limit);
                let _ =
                    telegram::edit_message(client, peer, edit_id, &display_final[..split_at]).await;
                let rest = display_final[split_at..].trim_start_matches('\n');
                if !rest.is_empty() {
                    let _ = telegram::send_long(client, peer, rest).await;
                }
            }
        }
        None => {
            // Never sent anything (very short response?) — send now
            if !cleaned.trim().is_empty() {
                let _ = telegram::send_long(client, peer, cleaned.trim()).await;
            }
        }
    }

    // Log response and index it in vecstore
    {
        let cleaned = cleaned.clone();
        let vs = Arc::clone(vecstore);
        let ts = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        tokio::task::spawn_blocking(move || {
            memory::append_dialog(sender_id, "Sophia", &cleaned);
            if let Err(e) = vs.add(&cleaned, "Sophia", sender_id, &ts) {
                warn!("Failed to index Sophia response: {}", e);
            }
            // Save index periodically (every message for now, cheap operation)
            if let Err(e) = vs.save(&crate::config::data_dir().join("vecstore.usearch")) {
                warn!("Failed to save vecstore index: {}", e);
            }
        })
        .await.unwrap_or_else(|e| {
            warn!("Failed to save dialog/vecstore: {}", e);
        });
    }

    // Done reaction
    telegram::react(client, peer, msg_id, "👌").await;

    Ok(())
}

// --- Command handlers ---

async fn handle_pair(
    client: &Client,
    peer: PeerRef,
    sender_id: i64,
    is_owner: bool,
    _config: &Config,
) -> Result<()> {
    if is_owner {
        telegram::send_long(client, peer, "You're the owner — no pairing needed.").await?;
        return Ok(());
    }
    if pairing::is_paired(sender_id) {
        telegram::send_long(client, peer, "You're already paired!").await?;
        return Ok(());
    }

    let name = format!("User {}", sender_id);
    pairing::add_pending(sender_id, &name)?;
    telegram::send_long(
        client,
        peer,
        "Pairing request sent to the owner. Please wait for approval.",
    )
    .await?;

    info!(
        "Pairing request from {} ({}). Approve with /approve {}",
        name, sender_id, sender_id
    );

    Ok(())
}

async fn handle_approve(client: &Client, peer: PeerRef, arg: &str) -> Result<()> {
    let uid: i64 = match arg.parse() {
        Ok(id) => id,
        Err(_) => {
            telegram::send_long(client, peer, "Usage: /approve <user_id>").await?;
            return Ok(());
        }
    };

    let pending = match pairing::get_pending(uid) {
        Some(p) => p,
        None => {
            telegram::send_long(client, peer, &format!("No pending request from ID {}.", uid))
                .await?;
            return Ok(());
        }
    };

    pairing::add_paired(uid, &pending.name)?;
    pairing::remove_pending(uid)?;
    telegram::send_long(
        client,
        peer,
        &format!("Approved **{}** ({}).", pending.name, uid),
    )
    .await?;

    Ok(())
}

async fn handle_deny(client: &Client, peer: PeerRef, arg: &str) -> Result<()> {
    let uid: i64 = match arg.parse() {
        Ok(id) => id,
        Err(_) => {
            telegram::send_long(client, peer, "Usage: /deny <user_id>").await?;
            return Ok(());
        }
    };

    let pending = match pairing::get_pending(uid) {
        Some(p) => p,
        None => {
            telegram::send_long(client, peer, &format!("No pending request from ID {}.", uid))
                .await?;
            return Ok(());
        }
    };

    pairing::remove_pending(uid)?;
    telegram::send_long(
        client,
        peer,
        &format!("Denied **{}** ({}).", pending.name, uid),
    )
    .await?;

    Ok(())
}

async fn handle_unpair(client: &Client, peer: PeerRef, arg: &str) -> Result<()> {
    let uid: i64 = match arg.parse() {
        Ok(id) => id,
        Err(_) => {
            telegram::send_long(client, peer, "Usage: /unpair <user_id>").await?;
            return Ok(());
        }
    };

    if pairing::remove_paired(uid)? {
        telegram::send_long(client, peer, &format!("Unpaired user {}.", uid)).await?;
    } else {
        telegram::send_long(client, peer, &format!("User {} was not paired.", uid)).await?;
    }
    Ok(())
}

async fn handle_exec(client: &Client, peer: PeerRef, arg: &str, config: &Config) -> Result<()> {
    if !config.exec_enabled {
        telegram::send_long(client, peer, "Command execution is disabled.").await?;
        return Ok(());
    }
    if arg.is_empty() {
        telegram::send_long(client, peer, "Usage: /exec <command>").await?;
        return Ok(());
    }

    let shell_chain = Regex::new(r"[;|&`$()]").unwrap();
    if shell_chain.is_match(arg) {
        telegram::send_long(client, peer, "Blocked: shell chaining/subshells not allowed.")
            .await?;
        return Ok(());
    }

    let parts = match shlex::split(arg) {
        Some(p) => p,
        None => {
            telegram::send_long(client, peer, "Parse error: invalid quoting.").await?;
            return Ok(());
        }
    };
    if parts.is_empty() {
        telegram::send_long(client, peer, "Empty command.").await?;
        return Ok(());
    }

    if !config.exec_allowed_commands.contains(&parts[0]) {
        let allowed = config.exec_allowed_commands.join(", ");
        telegram::send_long(
            client,
            peer,
            &format!(
                "Command `{}` not allowed.\nAllowed: `{}`",
                parts[0], allowed
            ),
        )
        .await?;
        return Ok(());
    }

    let output = tokio::task::spawn_blocking(move || {
        use std::process::Command;
        match Command::new(&parts[0]).args(&parts[1..]).output() {
            Ok(out) => {
                let mut result = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr);
                if !stderr.is_empty() {
                    result.push('\n');
                    result.push_str(&stderr);
                }
                result
            }
            Err(e) => format!("Exec error: {}", e),
        }
    })
    .await?;

    let output = output.trim();
    let output = if output.is_empty() {
        "(no output)"
    } else if output.len() > 4000 {
        &output[..output.floor_char_boundary(4000)]
    } else {
        output
    };

    telegram::send_long(client, peer, &format!("```\n{}\n```", output)).await?;
    Ok(())
}

async fn handle_memory(client: &Client, peer: PeerRef, arg: &str) -> Result<()> {
    if arg.is_empty() {
        let mem = tokio::task::spawn_blocking(memory::read_memory).await?;
        let display = if mem.trim().is_empty() {
            "(empty)".to_string()
        } else {
            mem
        };
        telegram::send_long(client, peer, &format!("**Memory:**\n{}", display)).await?;
        return Ok(());
    }

    if let Some(text) = arg.strip_prefix("add ") {
        let text = text.trim();
        if text.is_empty() {
            telegram::send_long(client, peer, "Usage: /memory add <text>").await?;
        } else {
            let text = text.to_string();
            tokio::task::spawn_blocking(move || memory::append_memory(&text)).await?;
            telegram::send_long(client, peer, "Memory updated.").await?;
        }
        return Ok(());
    }

    if arg == "clear" {
        tokio::task::spawn_blocking(memory::clear_memory).await?;
        telegram::send_long(client, peer, "Memory cleared.").await?;
        return Ok(());
    }

    telegram::send_long(client, peer, "Usage: /memory [add <text> | clear]").await?;
    Ok(())
}

async fn handle_update_cmd(
    client: &Client,
    peer: PeerRef,
    update_state: &UpdateState,
    shutdown_tx: &broadcast::Sender<()>,
) -> Result<()> {
    let has_pending = update_state.pending.lock().await.is_some();
    if !has_pending {
        telegram::send_long(client, peer, "Нет доступных обновлений.").await?;
        return Ok(());
    }

    let ver = {
        let guard = update_state.pending.lock().await;
        guard.as_ref().map(|r| r.version.clone()).unwrap_or_default()
    };

    telegram::send_long(
        client,
        peer,
        &format!("⏳ Обновляю до v{}... Это может занять несколько минут.", ver),
    )
    .await?;

    let success = tokio::task::spawn_blocking(update_check::run_update)
        .await
        .unwrap_or(false);

    if success {
        telegram::send_long(client, peer, "✅ Обновление завершено. Перезапускаюсь...")
            .await?;
        update_state.needs_restart.store(true, Ordering::SeqCst);
        let _ = shutdown_tx.send(());
    } else {
        telegram::send_long(
            client,
            peer,
            "❌ Обновление не удалось. Проверь логи.",
        )
        .await?;
    }
    Ok(())
}

async fn handle_help(client: &Client, peer: PeerRef, is_owner: bool) -> Result<()> {
    let mut lines = vec![
        "**Commands:**".to_string(),
        "`/pair` — Request access".to_string(),
        "`/help` — Show this help".to_string(),
    ];
    if is_owner {
        lines.push(String::new());
        lines.push("**Owner commands:**".to_string());
        lines.push("`/approve <id>` — Approve pairing".to_string());
        lines.push("`/deny <id>` — Deny pairing".to_string());
        lines.push("`/unpair <id>` — Remove paired user".to_string());
        lines.push("`/exec <cmd>` — Run OS command".to_string());
        lines.push("`/memory` — View memory".to_string());
        lines.push("`/memory add <text>` — Add to memory".to_string());
        lines.push("`/memory clear` — Clear memory".to_string());
        lines.push("`/update` — Install pending update and restart".to_string());
        lines.push("`/search <query>` — Semantic search in dialogs".to_string());
        lines.push("`/reindex` — Reindex all dialog files".to_string());
    }
    telegram::send_long(client, peer, &lines.join("\n")).await?;
    Ok(())
}

async fn handle_search(
    client: &Client,
    peer: PeerRef,
    query: &str,
    vecstore: &Arc<VecStore>,
) -> Result<()> {
    if query.is_empty() {
        telegram::send_long(client, peer, "Usage: /search <query>").await?;
        return Ok(());
    }

    let query = query.to_string();
    let vs = Arc::clone(vecstore);
    let results = tokio::task::spawn_blocking(move || vs.search(&query, Some(5))).await??;

    if results.is_empty() {
        telegram::send_long(client, peer, "Ничего не найдено.").await?;
        return Ok(());
    }

    let mut lines = vec![format!("**Результаты поиска** ({} шт, {} в индексе):", results.len(), vecstore.len())];
    for (i, r) in results.iter().enumerate() {
        let text_preview = if r.text.len() > 150 {
            let end = r.text.floor_char_boundary(150);
            format!("{}...", &r.text[..end])
        } else {
            r.text.clone()
        };
        lines.push(format!(
            "{}. [{:.2}] **{}** [{}]: {}",
            i + 1,
            r.score,
            r.role,
            r.timestamp,
            text_preview,
        ));
    }

    telegram::send_long(client, peer, &lines.join("\n")).await?;
    Ok(())
}

async fn handle_reindex(
    client: &Client,
    peer: PeerRef,
    vecstore: &Arc<VecStore>,
) -> Result<()> {
    telegram::send_long(client, peer, "Начинаю переиндексацию диалогов...").await?;

    let vs = Arc::clone(vecstore);
    let result = tokio::task::spawn_blocking(move || -> Result<(usize, usize)> {
        let dialogs_dir = crate::config::dialogs_dir();
        if !dialogs_dir.exists() {
            return Ok((0, 0));
        }

        let mut total_chunks = 0usize;
        let mut total_files = 0usize;

        // Iterate user directories
        for user_entry in std::fs::read_dir(&dialogs_dir)? {
            let user_entry = user_entry?;
            if !user_entry.file_type()?.is_dir() {
                continue;
            }
            let user_id: i64 = match user_entry.file_name().to_str().and_then(|s| s.parse().ok()) {
                Some(id) => id,
                None => continue,
            };

            // Iterate dialog files
            for file_entry in std::fs::read_dir(user_entry.path())? {
                let file_entry = file_entry?;
                let path = file_entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }

                match vs.index_dialog_file(&path, user_id) {
                    Ok(count) => {
                        total_chunks += count;
                        total_files += 1;
                        info!("Indexed {} chunks from {:?}", count, path);
                    }
                    Err(e) => {
                        error!("Failed to index {:?}: {}", path, e);
                    }
                }
            }
        }

        // Save index
        vs.save(&crate::config::data_dir().join("vecstore.usearch"))?;

        Ok((total_files, total_chunks))
    })
    .await??;

    telegram::send_long(
        client,
        peer,
        &format!(
            "Переиндексация завершена: {} файлов, {} чанков. Всего в индексе: {}",
            result.0, result.1, vecstore.len()
        ),
    )
    .await?;

    Ok(())
}
