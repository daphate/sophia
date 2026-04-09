use std::collections::HashMap;

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::config;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedUser {
    pub name: String,
    pub paired_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingRequest {
    pub name: String,
    pub requested_at: String,
}

// --- Owner ---

pub fn save_owner(info: &serde_json::Value) -> Result<()> {
    let dir = config::users_dir();
    std::fs::create_dir_all(&dir)?;
    let data = serde_json::to_string_pretty(info)?;
    std::fs::write(config::owner_file(), data)?;
    Ok(())
}

// --- Paired ---

pub fn load_paired() -> HashMap<String, PairedUser> {
    let path = config::paired_file();
    if !path.exists() {
        return HashMap::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_paired(paired: &HashMap<String, PairedUser>) -> Result<()> {
    std::fs::create_dir_all(config::users_dir())?;
    let data = serde_json::to_string_pretty(paired)?;
    std::fs::write(config::paired_file(), data)?;
    Ok(())
}

pub fn is_paired(user_id: i64) -> bool {
    let paired = load_paired();
    paired.contains_key(&user_id.to_string())
}

pub fn add_paired(user_id: i64, name: &str) -> Result<()> {
    let mut paired = load_paired();
    paired.insert(
        user_id.to_string(),
        PairedUser {
            name: name.to_string(),
            paired_at: Utc::now().to_rfc3339(),
        },
    );
    save_paired(&paired)
}

pub fn remove_paired(user_id: i64) -> Result<bool> {
    let mut paired = load_paired();
    let key = user_id.to_string();
    if paired.remove(&key).is_some() {
        save_paired(&paired)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

// --- Pending (persistent) ---

pub fn load_pending() -> HashMap<String, PendingRequest> {
    let path = config::pending_file();
    if !path.exists() {
        return HashMap::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_pending(pending: &HashMap<String, PendingRequest>) -> Result<()> {
    std::fs::create_dir_all(config::users_dir())?;
    let data = serde_json::to_string_pretty(pending)?;
    std::fs::write(config::pending_file(), data)?;
    Ok(())
}

pub fn add_pending(user_id: i64, name: &str) -> Result<()> {
    let mut pending = load_pending();
    pending.insert(
        user_id.to_string(),
        PendingRequest {
            name: name.to_string(),
            requested_at: Utc::now().to_rfc3339(),
        },
    );
    save_pending(&pending)
}

pub fn get_pending(user_id: i64) -> Option<PendingRequest> {
    let pending = load_pending();
    pending.get(&user_id.to_string()).cloned()
}

pub fn remove_pending(user_id: i64) -> Result<()> {
    let mut pending = load_pending();
    pending.remove(&user_id.to_string());
    save_pending(&pending)
}
