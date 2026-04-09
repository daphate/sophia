mod commands;
mod config;
mod watchdog;

use std::sync::Arc;

use anyhow::{Context, Result};
use grammers_client::client::{Client, UpdatesConfiguration};
use grammers_client::message::InputMessage;
use grammers_client::update::Update;
use grammers_mtsender::SenderPool;
use grammers_session::storages::SqliteSession;
use grammers_session::types::{PeerId, PeerRef};
#[allow(unused_imports)]
use grammers_session::Session; // trait needed for peer_ref()
use tracing::{error, info};

use config::Config;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    commands::init_start_time();

    let config = Config::from_env().context("Failed to load config")?;
    info!("sophia-rescue starting...");
    info!("  sophia root: {}", config.sophia_root.display());

    // Session & sender pool (same pattern as main sophia)
    let session_path = config.sophia_root.join("rescue.session");
    let session = Arc::new(
        SqliteSession::open(&session_path)
            .await
            .context("Failed to open session")?,
    );
    let pool = SenderPool::new(Arc::clone(&session), config.api_id);
    let client = Client::new(pool.handle);

    // Spawn the network I/O runner
    tokio::spawn(async move {
        pool.runner.run().await;
    });

    // Authenticate
    if !client.is_authorized().await? {
        info!("Signing in as bot...");
        client
            .bot_sign_in(&config.bot_token, &config.api_hash)
            .await
            .context("Bot sign-in failed")?;
        info!("Signed in successfully");
    }

    let me = client.get_me().await?;
    info!(
        "Logged in as {} (@{})",
        me.first_name().unwrap_or("rescue"),
        me.username().unwrap_or("?")
    );

    let owner_id = config.owner_id;

    // Spawn watchdog (peer cache fills after owner sends first message)
    let wd_client = client.clone();
    let wd_session = Arc::clone(&session);
    tokio::spawn(async move {
        watchdog::run(&wd_client, &wd_session, owner_id).await;
    });

    // Notify owner on startup (only if peer is already cached from a previous session)
    if let Some(peer) = resolve_owner(&session, owner_id).await {
        let _ = client
            .send_message(
                peer,
                InputMessage::new().text("🛟 sophia-rescue запущена и следит за основной Софией."),
            )
            .await;
    } else {
        info!("Owner peer not cached yet — startup notification skipped. Send /ping to initialize.");
    }

    // Main update loop
    let mut update_stream = client
        .stream_updates(
            pool.updates,
            UpdatesConfiguration {
                catch_up: false,
                ..Default::default()
            },
        )
        .await;

    info!("Listening for updates...");

    loop {
        match update_stream.next().await {
            Ok(update) => {
                if let Update::NewMessage(message) = update {
                    if message.outgoing() {
                        update_stream.sync_update_state().await;
                        continue;
                    }

                    let sender_id = match message.sender_id() {
                        Some(id) => id.bare_id() as i64,
                        None => {
                            update_stream.sync_update_state().await;
                            continue;
                        }
                    };

                    // Only respond to owner
                    if sender_id != owner_id {
                        update_stream.sync_update_state().await;
                        continue;
                    }

                    let text = message.text();
                    if !text.is_empty() && text.starts_with('/') {
                        if let Some(response) = commands::handle(text).await {
                            if let Some(peer) = message.peer_ref().await {
                                send_long(&client, peer, &response).await;
                            }
                        }
                    }
                }

                update_stream.sync_update_state().await;
            }
            Err(e) => {
                error!("Error getting update: {e}");
                break;
            }
        }
    }

    Ok(())
}

/// Resolve owner by ID using the session's peer cache.
async fn resolve_owner(session: &SqliteSession, owner_id: i64) -> Option<PeerRef> {
    let peer_id = PeerId::user_unchecked(owner_id);
    session.peer_ref(peer_id).await
}

/// Send message, splitting at 4096 char Telegram limit.
async fn send_long(client: &Client, peer: PeerRef, text: &str) {
    const TG_MAX_CHARS: usize = 4096;

    let mut remaining = text;
    while !remaining.is_empty() {
        if remaining.chars().count() <= TG_MAX_CHARS {
            let _ = client
                .send_message(peer, InputMessage::new().text(remaining))
                .await;
            break;
        }

        let byte_limit = remaining
            .char_indices()
            .nth(TG_MAX_CHARS)
            .map(|(idx, _)| idx)
            .unwrap_or(remaining.len());

        let split_at = remaining[..byte_limit]
            .rfind('\n')
            .unwrap_or(byte_limit);

        let _ = client
            .send_message(peer, InputMessage::new().text(&remaining[..split_at]))
            .await;

        remaining = remaining[split_at..].trim_start_matches('\n');
    }
}
