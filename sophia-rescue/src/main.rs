use sophia::config;
use sophia::handlers;
use sophia::outbox;
use sophia::telegram;
use sophia::update_check;

use std::sync::Arc;

use anyhow::{Context, Result};
use sophia::grammers_client::client::{Client, UpdatesConfiguration};
use sophia::grammers_mtsender::SenderPool;
use sophia::grammers_session::storages::SqliteSession;
use sophia::grammers_session::Session;
use tokio::sync::broadcast;
use std::sync::atomic::Ordering;
use tracing::{debug, error, info};

use sophia::vecstore::VecStore;
use sophia::config::Config;
use sophia::pairing::save_owner;
use sophia::queue::MessageQueue;

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

    handlers::init_start_time();

    let config = Config::from_env_rescue().context("Failed to load rescue config")?;
    info!("sophia-rescue starting...");

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

    tokio::spawn(async move {
        pool.runner.run().await;
    });

    // Authenticate
    if !client.is_authorized().await? {
        info!("Not authorized, signing in as bot...");
        match &config.mode {
            config::BotMode::Bot { token } => {
                client
                    .bot_sign_in(token, &config.api_hash)
                    .await
                    .context("Bot sign in failed")?;
                info!("Signed in as bot");
            }
            _ => anyhow::bail!("Rescue bot must use BOT_TOKEN mode"),
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

    // Save owner info
    save_owner(&sophia::serde_json::json!({
        "id": config.owner_id,
        "bot_user_id": me_id.bare_id(),
        "bot_name": me_name,
    }))?;

    // Initialize queue (rescue has its own queue DB)
    let queue = MessageQueue::new(&config::queue_db_for(config.role))?;
    let recovered = queue.recover()?;
    if recovered > 0 {
        info!("Recovered {} stuck messages from queue", recovered);
    }

    // Periodic recovery sweep
    {
        let queue_sweep = queue.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(120));
            loop {
                interval.tick().await;
                let q = queue_sweep.clone();
                match tokio::task::spawn_blocking(move || q.recover_stale(600.0)).await {
                    Ok(Ok(n)) if n > 0 => info!("Recovery sweep: recovered {} stuck messages", n),
                    Ok(Err(e)) => error!("Recovery sweep error: {}", e),
                    Err(e) => error!("Recovery sweep task error: {}", e),
                    _ => {}
                }
            }
        });
    }

    // Initialize session store (rescue has its own sessions DB)
    let sessions = sophia::sessions::SessionStore::new(
        &config::data_dir().join("sessions_rescue.db"),
    ).context("Failed to initialize SessionStore")?;
    {
        let s = sessions.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                interval.tick().await;
                let sc = s.clone();
                tokio::task::spawn_blocking(move || {
                    let _ = sc.expire_stale(24);
                    let _ = sc.cleanup(7);
                }).await.ok();
            }
        });
    }

    // Initialize vector store (shared with main bot)
    let vecstore = Arc::new(
        tokio::task::spawn_blocking(|| {
            VecStore::new(
                &config::data_dir().join("vecstore.db"),
                &config::data_dir().join("vecstore.usearch"),
            )
        })
        .await?
        .context("Failed to initialize VecStore")?,
    );
    info!("VecStore ready ({} vectors)", vecstore.len());

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

    // Update check
    let update_state = update_check::UpdateState::new();

    // Outbox watcher
    outbox::spawn_outbox_watcher(
        client.clone(),
        Arc::clone(&session),
        shutdown_tx.subscribe(),
    );

    // Watchdog: monitor peer bot (main sophia)
    {
        let wd_client = client.clone();
        let wd_session = Arc::clone(&session);
        let owner_id = config.owner_id;
        let peer_service = config.peer_service.clone();
        tokio::spawn(async move {
            sophia::watchdog::run(&wd_client, &wd_session, owner_id, &peer_service).await;
        });
    }

    // Startup notification
    info!("sophia-rescue is running. Press Ctrl+C to stop.");
    {
        let peer_id = sophia::grammers_session::types::PeerId::user_unchecked(config.owner_id);
        if let Some(peer) = session.peer_ref(peer_id).await {
            let _ = telegram::send_long(&client, peer, "🛟 sophia-rescue перезапустилась").await;
        } else {
            info!("Cannot send startup notification (owner peer not cached)");
        }
    }

    // Main update loop
    let mut update_stream = client
        .stream_updates(pool.updates, UpdatesConfiguration { catch_up: false, ..Default::default() })
        .await;
    let mut shutdown_rx = shutdown_tx.subscribe();
    let mut sync_interval = tokio::time::interval(std::time::Duration::from_secs(5));
    sync_interval.tick().await;

    loop {
        tokio::select! {
            _ = sync_interval.tick() => {
                update_stream.sync_update_state().await;
            }
            update = update_stream.next() => {
                match update {
                    Ok(update) => {
                        let client = client.clone();
                        let config = config.clone();
                        let queue = queue.clone();
                        let user_locks = user_locks.clone();
                        let update_state = update_state.clone();
                        let shutdown_tx = shutdown_tx.clone();
                        let vecstore = Arc::clone(&vecstore);
                        let sessions = sessions.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handlers::handle_update(
                                &client, update, &config, me_id, &queue, &user_locks,
                                &update_state, &shutdown_tx, &vecstore, &sessions,
                            ).await {
                                error!("Error handling update: {}", e);
                            }
                        });
                        update_stream.sync_update_state().await;
                    }
                    Err(e) => {
                        error!("Error getting update (will retry): {}", e);
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                info!("Shutting down gracefully...");
                break;
            }
        }
    }

    update_stream.sync_update_state().await;

    if update_state.needs_restart.load(Ordering::SeqCst) {
        info!("Restarting after auto-update...");
        std::process::exit(update_check::EXIT_CODE_RESTART);
    }

    info!("sophia-rescue stopped.");
    Ok(())
}
