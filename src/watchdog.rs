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

/// Run the watchdog loop, monitoring a launchd service.
///
/// `service_label` — the launchd service to monitor (e.g. "com.sophia.bot")
pub async fn run(
    client: &Client,
    session: &SqliteSession,
    owner_id: i64,
    service_label: &str,
) {
    let peer = loop {
        if let Some(p) = resolve_peer(session, owner_id).await {
            break p;
        }
        warn!(
            "Watchdog: owner peer not cached yet, retrying in {}s...",
            CHECK_INTERVAL
        );
        tokio::time::sleep(std::time::Duration::from_secs(CHECK_INTERVAL)).await;
    };

    info!(
        "Watchdog: resolved owner peer, monitoring {} started",
        service_label
    );

    let mut dead_since: Option<Instant> = None;
    let mut alerted = false;

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(CHECK_INTERVAL));
    interval.tick().await; // skip immediate first tick

    loop {
        interval.tick().await;

        let alive = is_service_alive(service_label).await;

        if alive {
            if dead_since.is_some() {
                if alerted {
                    let _ = client
                        .send_message(
                            peer,
                            InputMessage::new().text(&format!(
                                "✅ {} recovered and is running again.",
                                service_label
                            )),
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
                            "🚨 {} is down for >{}s!\n\
                             Use /restart or /status to investigate.",
                            service_label, DEAD_THRESHOLD
                        )),
                    )
                    .await;
                alerted = true;
            }
        }
    }
}

async fn resolve_peer(session: &SqliteSession, owner_id: i64) -> Option<PeerRef> {
    let peer_id = PeerId::user_unchecked(owner_id);
    session.peer_ref(peer_id).await
}

/// Check if a launchd service is alive by parsing `launchctl list <label>`.
pub async fn is_service_alive(label: &str) -> bool {
    let output = Command::new("launchctl")
        .args(["list", label])
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
                if parts.len() >= 3 && parts[2] == label {
                    return parts[0].trim() != "-";
                }
            }
            false
        }
        Err(_) => false,
    }
}
