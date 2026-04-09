use std::process::Command;

use semver::Version;
use tracing::{error, info, warn};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_REPO: &str = "daphate/sophia";

/// Exit code that signals "I updated, restart me".
pub const EXIT_CODE_RESTART: i32 = 42;

/// Check GitHub for a new release. If `auto_update` is true, pulls and rebuilds.
/// Returns `true` if an update was applied and the process should restart.
pub async fn check_for_updates(auto_update: bool) -> bool {
    let current = match Version::parse(CURRENT_VERSION) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );

    let client = match reqwest::Client::builder()
        .user_agent("sophia-bot")
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            warn!("Update check failed: {}", e);
            return false;
        }
    };

    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(_) => return false,
    };

    let tag = match json["tag_name"].as_str() {
        Some(t) => t.strip_prefix('v').unwrap_or(t),
        None => return false,
    };

    let latest = match Version::parse(tag) {
        Ok(v) => v,
        Err(_) => return false,
    };

    if latest <= current {
        info!("Sophia v{} — up to date", current);
        return false;
    }

    info!("========================================");
    info!("New version available: v{} (current: v{})", latest, current);

    if !auto_update {
        info!("Update: cd sophia && git pull && cargo build --release");
        info!("Or set AUTO_UPDATE=true for automatic updates");
        info!("========================================");
        return false;
    }

    info!("Auto-updating...");
    info!("========================================");

    if !run_update() {
        error!("Auto-update failed, continuing with current version");
        return false;
    }

    info!("Update complete, restarting...");
    true
}

fn run_update() -> bool {
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
