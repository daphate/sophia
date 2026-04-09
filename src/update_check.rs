use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use semver::Version;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_REPO: &str = "daphate/sophia";

/// Exit code that signals "I updated, restart me".
pub const EXIT_CODE_RESTART: i32 = 42;

/// Info about an available release.
pub struct ReleaseInfo {
    pub version: String,
    pub body: String,
    pub url: String,
}

/// Shared state for pending updates.
#[derive(Clone)]
pub struct UpdateState {
    /// Pending release info.
    pub pending: Arc<Mutex<Option<ReleaseInfo>>>,
    /// Set to true after successful pull+build to trigger restart.
    pub needs_restart: Arc<AtomicBool>,
}

impl UpdateState {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(None)),
            needs_restart: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Check GitHub for a new release. Returns release info if a newer version exists.
pub async fn check_for_updates(update_state: &UpdateState) -> Option<String> {
    let current = match Version::parse(CURRENT_VERSION) {
        Ok(v) => v,
        Err(_) => return None,
    };

    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );

    let client = match reqwest::Client::builder()
        .user_agent("sophia-bot")
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return None,
    };

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            warn!("Update check failed: {}", e);
            return None;
        }
    };

    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(_) => return None,
    };

    let tag = match json["tag_name"].as_str() {
        Some(t) => t.strip_prefix('v').unwrap_or(t),
        None => return None,
    };

    let latest = match Version::parse(tag) {
        Ok(v) => v,
        Err(_) => return None,
    };

    if latest <= current {
        info!("Sophia v{} — up to date", current);
        return None;
    }

    let version = latest.to_string();
    let body = json["body"].as_str().unwrap_or("").to_string();
    let html_url = json["html_url"]
        .as_str()
        .unwrap_or_else(|| {
            Box::leak(
                format!("https://github.com/{}/releases/tag/v{}", GITHUB_REPO, version)
                    .into_boxed_str(),
            )
        })
        .to_string();

    info!("New version available: v{} (current: v{})", version, current);

    let ver_clone = version.clone();
    *update_state.pending.lock().await = Some(ReleaseInfo {
        version,
        body,
        url: html_url,
    });

    Some(ver_clone)
}

/// Pull and rebuild. Returns true on success.
pub fn run_update() -> bool {
    info!("Running: git pull");
    let pull = Command::new("git").args(["pull", "--ff-only"]).status();
    match pull {
        Ok(s) if s.success() => {}
        Ok(s) => {
            error!("git pull failed with {}", s);
            return false;
        }
        Err(e) => {
            error!("git pull error: {}", e);
            return false;
        }
    }

    info!("Running: cargo build --release (this may take a while)");
    let build = Command::new("cargo")
        .args(["build", "--release"])
        .status();
    match build {
        Ok(s) if s.success() => true,
        Ok(s) => {
            error!("cargo build failed with {}", s);
            false
        }
        Err(e) => {
            error!("cargo build error: {}", e);
            false
        }
    }
}

/// Format a notification message for the owner.
pub fn format_update_message(info: &ReleaseInfo) -> String {
    let mut msg = format!("🆕 Доступна новая версия: **v{}**\n", info.version);
    if !info.body.is_empty() {
        // Trim release notes to reasonable length
        let notes = if info.body.len() > 1500 {
            let end = info.body.floor_char_boundary(1500);
            format!("{}...", &info.body[..end])
        } else {
            info.body.clone()
        };
        msg.push_str(&format!("\n{}\n", notes));
    }
    msg.push_str(&format!("\n🔗 {}\n", info.url));
    msg.push_str("\nОтправь /update чтобы обновиться.");
    msg
}
