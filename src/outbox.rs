//! Outbox: watch `data/outbox/` for message files and send them.
//!
//! File format (JSON): `{ "chat_id": 727377241, "text": "Hello!" }`
//! After sending, the file is deleted.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use grammers_client::client::Client;
use grammers_session::Session;
use grammers_session::storages::SqliteSession;
use serde::Deserialize;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

#[derive(Debug, Deserialize)]
struct OutboxMessage {
    chat_id: i64,
    text: String,
}

fn outbox_dir() -> PathBuf {
    crate::config::data_dir().join("outbox")
}

/// Spawn a task that polls `data/outbox/` every 2 seconds for `.json` files.
pub fn spawn_outbox_watcher(
    client: Client,
    session: Arc<SqliteSession>,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    let dir = outbox_dir();

    // Ensure directory exists
    if let Err(e) = std::fs::create_dir_all(&dir) {
        error!("Cannot create outbox dir {:?}: {}", dir, e);
        return;
    }
    info!("Outbox watcher started on {:?}", dir);

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = process_outbox(&client, &session, &dir).await {
                        error!("Outbox error: {}", e);
                    }
                }
                _ = shutdown_rx.recv() => {
                    info!("Outbox watcher shutting down");
                    break;
                }
            }
        }
    });
}

async fn process_outbox(
    client: &Client,
    session: &Arc<SqliteSession>,
    dir: &std::path::Path,
) -> anyhow::Result<()> {
    let entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext == "json")
            })
            .collect(),
        Err(_) => return Ok(()),
    };

    for entry in entries {
        let path = entry.path();
        debug!("Outbox: processing {:?}", path);

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Cannot read {:?}: {}", path, e);
                continue;
            }
        };

        let msg: OutboxMessage = match serde_json::from_str(&content) {
            Ok(m) => m,
            Err(e) => {
                warn!("Invalid outbox JSON {:?}: {}", path, e);
                // Move bad file so it doesn't retry forever
                let _ = std::fs::rename(&path, path.with_extension("json.bad"));
                continue;
            }
        };

        // Resolve peer
        let peer_id = grammers_session::types::PeerId::user_unchecked(msg.chat_id);
        let peer = session.peer_ref(peer_id).await;

        match peer {
            Some(peer_ref) => {
                match crate::telegram::send_long(client, peer_ref, &msg.text).await {
                    Ok(_) => {
                        info!("Outbox: sent message to {}", msg.chat_id);
                        let _ = std::fs::remove_file(&path);
                    }
                    Err(e) => {
                        error!("Outbox: failed to send to {}: {}", msg.chat_id, e);
                    }
                }
            }
            None => {
                warn!(
                    "Outbox: peer {} not in cache, skipping {:?}",
                    msg.chat_id,
                    path.file_name()
                );
            }
        }
    }

    Ok(())
}
