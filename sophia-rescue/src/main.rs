mod api;
mod commands;
mod config;
mod watchdog;

use api::TgClient;
use config::Config;

#[tokio::main]
async fn main() {
    commands::init_start_time();

    // Load config
    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("FATAL: {e}");
            std::process::exit(1);
        }
    };

    eprintln!("sophia-rescue starting...");
    eprintln!("  sophia root: {}", config.sophia_root.display());

    // Build Telegram client and validate token
    let tg = TgClient::new(config);

    let me = match tg.get_me().await {
        Ok(me) => me,
        Err(e) => {
            eprintln!("FATAL: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "Logged in as {} (@{})",
        me.first_name,
        me.username.as_deref().unwrap_or("?")
    );

    // Spawn watchdog
    // Safety: TgClient needs to be shared between watchdog and polling loop.
    // We use a simple &'static leak since this is a long-running daemon.
    let tg: &'static TgClient = Box::leak(Box::new(tg));

    tokio::spawn(async move {
        watchdog::run(tg).await;
    });

    // Notify owner on startup
    tg.send_message(
        tg.owner_id(),
        "🛟 sophia-rescue запущена и следит за основной Софией.",
    )
    .await;

    // Main polling loop
    let mut offset: i64 = 0;

    loop {
        match tg.get_updates(offset).await {
            Ok(updates) => {
                for update in updates {
                    offset = update.update_id + 1;

                    if let Some(msg) = update.message {
                        if let Some(text) = &msg.text {
                            if let Some(response) = commands::handle(tg, msg.chat.id, text).await {
                                tg.send_message(msg.chat.id, &response).await;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("Polling error: {e}");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}
