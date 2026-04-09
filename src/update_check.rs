use semver::Version;
use tracing::{info, warn};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_REPO: &str = "daphate/sophia";

pub async fn check_for_updates() {
    let current = match Version::parse(CURRENT_VERSION) {
        Ok(v) => v,
        Err(_) => return,
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
        Err(_) => return,
    };

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            warn!("Update check failed: {}", e);
            return;
        }
    };

    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(_) => return,
    };

    let tag = match json["tag_name"].as_str() {
        Some(t) => t.strip_prefix('v').unwrap_or(t),
        None => return,
    };

    let latest = match Version::parse(tag) {
        Ok(v) => v,
        Err(_) => return,
    };

    if latest > current {
        info!("========================================");
        info!("New version available: v{} (current: v{})", latest, current);
        info!("Update: cd sophia && git pull && cargo build --release");
        info!("========================================");
    } else {
        info!("Sophia v{} — up to date", current);
    }
}
