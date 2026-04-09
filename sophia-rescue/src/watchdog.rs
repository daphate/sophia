use std::time::Instant;

use tokio::process::Command;

use crate::api::TgClient;

/// Check interval in seconds.
const CHECK_INTERVAL: u64 = 60;
/// Alert after being dead this many seconds.
const DEAD_THRESHOLD: u64 = 30;

pub async fn run(tg: &TgClient) {
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
                // Was dead, now recovered
                if alerted {
                    tg.send_message(
                        tg.owner_id(),
                        "✅ sophia recovered and is running again.",
                    )
                    .await;
                }
                dead_since = None;
                alerted = false;
            }
        } else {
            // Process is dead
            let first_dead = *dead_since.get_or_insert_with(Instant::now);
            let dead_secs = first_dead.elapsed().as_secs();

            if dead_secs >= DEAD_THRESHOLD && !alerted {
                tg.send_message(
                    tg.owner_id(),
                    &format!(
                        "🚨 sophia is down for >{DEAD_THRESHOLD}s!\n\
                         launchd did not restart it.\n\
                         Use /restart or /status to investigate."
                    ),
                )
                .await;
                alerted = true;
            }
        }
    }
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
            // Parse PID from "PID\tStatus\tLabel" format
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() >= 3 && parts[2] == "com.sophia.bot" {
                    // PID is "-" when not running
                    return parts[0].trim() != "-";
                }
            }
            false
        }
        Err(_) => false,
    }
}
