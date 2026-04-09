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
use tracing::{error, info};

use crate::config::Config;
use crate::inference;
use crate::memory;
use crate::pairing;
use crate::queue::MessageQueue;
use crate::telegram;
use crate::update_check::{self, UpdateState};

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
                update_state, shutdown_tx,
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
) -> Result<()> {
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

    // --- React and process ---
    telegram::react(client, peer, msg_id, "🫡").await;
    info!("Message from {}, processing", sender_id);

    process_message(
        client, config, peer, msg_id, sender_id, &text, &file_paths, queue, user_locks,
    )
    .await
}

async fn process_message(
    client: &Client,
    config: &Config,
    peer: PeerRef,
    msg_id: i32,
    sender_id: i64,
    text: &str,
    file_paths: &[PathBuf],
    _queue: &MessageQueue,
    user_locks: &UserLocks,
) -> Result<()> {
    // Log user message
    {
        let lock = get_lock(user_locks, sender_id);
        let _guard = lock.lock().await;
        tokio::task::spawn_blocking({
            let text = text.to_string();
            move || memory::append_dialog(sender_id, "User", &text)
        })
        .await?;
    }

    // Thinking reaction
    telegram::react(client, peer, msg_id, "🤔").await;

    // Send typing indicator
    telegram::send_typing(client, peer).await;

    // Call Claude
    let paths = if file_paths.is_empty() {
        None
    } else {
        Some(file_paths)
    };
    let (response, cost) = match inference::ask_claude(sender_id, text, config, paths).await {
        Ok(result) => result,
        Err(e) => {
            telegram::react(client, peer, msg_id, "🥶").await;
            error!("Inference failed for msg {}: {}", msg_id, e);
            telegram::send_long(client, peer, "Произошла ошибка при обработке запроса.").await?;
            return Ok(());
        }
    };

    // Composing reaction
    telegram::react(client, peer, msg_id, "🧑‍💻").await;

    // Extract and save memory updates
    let (cleaned, updates) = memory::extract_memory_updates(&response);
    if !updates.is_empty() {
        tokio::task::spawn_blocking(move || {
            for update in &updates {
                memory::append_memory(update);
            }
        })
        .await?;
    }

    if let Some(cost) = &cost {
        info!(
            "Inference cost for {}: in={} out={} usd={:?}",
            sender_id, cost.input_tokens, cost.output_tokens, cost.cost_usd
        );
    }

    // Log response
    {
        let lock = get_lock(user_locks, sender_id);
        let _guard = lock.lock().await;
        tokio::task::spawn_blocking({
            let cleaned = cleaned.clone();
            move || memory::append_dialog(sender_id, "Sophia", &cleaned)
        })
        .await?;
    }

    // Send response
    telegram::send_long(client, peer, &cleaned).await?;

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
        let mut end = 4000;
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        &output[..end]
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
    }
    telegram::send_long(client, peer, &lines.join("\n")).await?;
    Ok(())
}
