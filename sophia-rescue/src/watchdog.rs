use std::time::Instant;

use grammers_client::client::Client;
use grammers_client::message::InputMessage;
use grammers_session::storages::SqliteSession;
use grammers_session::types::{PeerId, PeerRef};
use grammers_session::Session;
use tokio::process::Command;
use tracing::{info, warn};

/// Check interval in seconds.
const CHECK_INTERVAL: u64 = 60;
/// Alert after being dead this many seconds.
const DEAD_THRESHOLD: u64 = 30;

pub async fn run(client: &Client, session: &SqliteSession, owner_id: i64) {
    // Wait for owner peer to appear in cache (populated when owner sends a message)
    let peer = loop {
        if let Some(p) = resolve_owner_peer(session, owner_id).await {
            break p;
        }
        warn!("Watchdog: owner peer not cached yet, retrying in {CHECK_INTERVAL}s...");
        tokio::time::sleep(std::time::Duration::from_secs(CHECK_INTERVAL)).await;
    };

    info!("Watchdog: resolved owner peer, monitoring started");

    let mut dead_since: Option<Instant> = None;
    let mut alerted = false;

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(CHECK_INTERVAL));
    // Skip immediate first tick
    interval.tick().await;

    loop {
        interval.tick().await;

        let alive = is_sophia_alive().await;

        if alive {
            if dead_since.is_some() {
                if alerted {
                    let _ = client
                        .send_message(
                            peer,
                            InputMessage::new()
                                .text("✅ sophia recovered and is running again."),
                        )
                        .await;
                }
                dead_since = None;
                alerted = false;
            }
        } else {
            let first_dead = *dead_since.get_or_insert_with(Instant::now);
            let dead_secs = first_dead.elapsed().as_secs();

            if dead_secs >= DEAD_THRESHOLD && !alerted {
                let _ = client
                    .send_message(
                        peer,
                        InputMessage::new().text(&format!(
                            "🚨 sophia is down for >{DEAD_THRESHOLD}s!\n\
                             launchd did not restart it.\n\
                             Use /restart or /status to investigate."
                        )),
                    )
                    .await;
                alerted = true;
            }
        }
    }
}

async fn resolve_owner_peer(session: &SqliteSession, owner_id: i64) -> Option<PeerRef> {
    let peer_id = PeerId::user_unchecked(owner_id);
    session.peer_ref(peer_id).await
}

async fn is_sophia_alive() -> bool {
    let output = Command::new("launchctl")
        .args(["list", "com.sophia.bot"])
        .output()
        .await;

    match output {
        Ok(out) => {
            if !out.status.success() {
                return false;
            }
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() >= 3 && parts[2] == "com.sophia.bot" {
                    return parts[0].trim() != "-";
                }
            }
            false
        }
        Err(_) => false,
    }
}
