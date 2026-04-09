mod config;
mod handlers;
mod inference;
mod memory;
mod pairing;
mod queue;
mod telegram;
mod update_check;

use std::io::{self, BufRead, Write as _};
use std::sync::Arc;

use anyhow::{Context, Result};
use grammers_client::client::{Client, UpdatesConfiguration};
use grammers_mtsender::SenderPool;
use grammers_session::storages::SqliteSession;
use tokio::sync::broadcast;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, error, info};

use crate::config::Config;
use crate::pairing::save_owner;
use crate::queue::MessageQueue;

#[tokio::main]
async fn main() -> Result<()> {
    let debug_mode = std::env::args().any(|a| a == "--debug");

    let default_filter = if debug_mode { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter)),
        )
        .init();

    if debug_mode {
        info!("Debug mode enabled");
    }

    let config = Config::from_env().context("Failed to load config")?;
    info!("Config loaded, connecting to Telegram...");

    // Session & sender pool
    let session_path = config::project_root()
        .join(&config.session_name)
        .with_extension("session");
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

    // Authenticate if needed
    if !client.is_authorized().await? {
        info!("Not authorized, starting login...");

        match &config.mode {
            config::BotMode::Bot { token } => {
                client
                    .bot_sign_in(token, &config.api_hash)
                    .await
                    .context("Bot sign in failed")?;
                info!("Signed in as bot");
            }
            config::BotMode::Userbot { phone_number } => {
                let token = client
                    .request_login_code(phone_number, &config.api_hash)
                    .await
                    .context("Failed to request login code")?;

                print!("Enter the code you received: ");
                io::stdout().flush()?;
                let code = io::stdin().lock().lines().next().context("No input")??;

                use grammers_client::client::SignInError;
                match client.sign_in(&token, code.trim()).await {
                    Ok(_) => info!("Signed in successfully"),
                    Err(SignInError::PasswordRequired(password_token)) => {
                        print!("Enter 2FA password: ");
                        io::stdout().flush()?;
                        let password =
                            io::stdin().lock().lines().next().context("No input")??;
                        client
                            .check_password(password_token, password.trim())
                            .await
                            .context("2FA password check failed")?;
                        info!("Signed in with 2FA");
                    }
                    Err(e) => return Err(anyhow::anyhow!("Sign in failed: {}", e)),
                }
            }
        }
    }

    let me = client.get_me().await.context("Failed to get self")?;
    let me_id = me.id();
    let me_name = format!(
        "{} {}",
        me.first_name().unwrap_or(""),
        me.last_name().unwrap_or("")
    )
    .trim()
    .to_string();
    info!("Logged in as {} (ID: {:?})", me_name, me_id);

    // Populate peer cache (required for stream_updates to work in userbot mode)
    if !config.is_bot() {
        info!("Loading dialogs to populate peer cache...");
        let mut dialogs = client.iter_dialogs();
        let mut dialog_count = 0u32;
        while let Some(_) = dialogs.next().await? {
            dialog_count += 1;
        }
        info!("Cached {} dialogs", dialog_count);
    }

    // Save owner info
    save_owner(&serde_json::json!({
        "id": config.owner_id,
        "bot_user_id": me_id.bare_id(),
        "bot_name": me_name,
    }))?;

    // Initialize queue
    let queue = MessageQueue::new(&config::queue_db())?;
    let recovered = queue.recover()?;
    if recovered > 0 {
        info!("Recovered {} stuck messages from queue", recovered);
    }

    // Per-user locks
    let user_locks = handlers::new_user_locks();

    // Shutdown signal
    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        if let Err(e) = tokio::signal::ctrl_c().await {
            error!("Failed to listen for ctrl+c: {}", e);
        }
        info!("Shutdown signal received");
        let _ = shutdown_tx_clone.send(());
    });

    // Periodic queue cleanup
    let queue_clone = queue.clone();
    let mut shutdown_rx_cleanup = shutdown_tx.subscribe();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = queue_clone.cleanup(24) {
                        error!("Queue cleanup error: {}", e);
                    }
                }
                _ = shutdown_rx_cleanup.recv() => break,
            }
        }
    });

    // Periodic update check
    let needs_restart = Arc::new(AtomicBool::new(false));
    if config.update_check_hours > 0 {
        let interval_secs = config.update_check_hours * 3600;
        let auto_update = config.auto_update;
        let needs_restart = needs_restart.clone();
        let shutdown_tx_update = shutdown_tx.clone();
        let mut shutdown_rx_updates = shutdown_tx.subscribe();
        tokio::spawn(async move {
            // Check immediately on startup
            if update_check::check_for_updates(auto_update).await {
                needs_restart.store(true, Ordering::SeqCst);
                let _ = shutdown_tx_update.send(());
                return;
            }
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(interval_secs));
            interval.tick().await; // skip first (already checked)
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if update_check::check_for_updates(auto_update).await {
                            needs_restart.store(true, Ordering::SeqCst);
                            let _ = shutdown_tx_update.send(());
                            break;
                        }
                    }
                    _ = shutdown_rx_updates.recv() => break,
                }
            }
        });
    }

    // Main update loop
    info!("Sophia is running. Press Ctrl+C to stop.");
    let mut update_stream = client
        .stream_updates(pool.updates, UpdatesConfiguration { catch_up: true, ..Default::default() })
        .await;
    let mut shutdown_rx = shutdown_tx.subscribe();

    loop {
        tokio::select! {
            update = update_stream.next() => {
                match update {
                    Ok(update) => {
                        match &update {
                            grammers_client::update::Update::Raw(raw) => {
                                let dbg = format!("{:?}", raw.raw);
                                let end = dbg.floor_char_boundary(200);
                                debug!("Raw update: {}", &dbg[..end]);
                            }
                            other => {
                                debug!("Update: {:?}", std::mem::discriminant(other));
                            }
                        }
                        let client = client.clone();
                        let config = config.clone();
                        let queue = queue.clone();
                        let user_locks = user_locks.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handlers::handle_update(
                                &client, update, &config, me_id, &queue, &user_locks,
                            ).await {
                                error!("Error handling update: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Error getting update: {}", e);
                        break;
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                info!("Shutting down gracefully...");
                break;
            }
        }
    }

    // Sync updates state before exit
    update_stream.sync_update_state().await;

    if needs_restart.load(Ordering::SeqCst) {
        info!("Restarting after auto-update...");
        std::process::exit(update_check::EXIT_CODE_RESTART);
    }

    info!("Sophia stopped.");
    Ok(())
}
