use std::net::SocketAddr;
use std::path::Path;

/// Load .env file from working directory if it exists.
/// Simple key=value parser, no quoting support needed.
pub fn load_dotenv() {
    let path = Path::new(".env");
    if !path.exists() {
        return;
    }
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            // Only set if not already in environment (env takes precedence)
            if std::env::var(key).is_err() {
                // SAFETY: called before any threads are spawned (single-threaded init)
                unsafe { std::env::set_var(key, value) };
            }
        }
    }
}

#[derive(Clone)]
pub struct ProxyConfig {
    pub claude_cli: String,
    pub bind_addr: SocketAddr,
    pub timeout_secs: Option<u64>,
    pub max_turns: Option<u64>,
    pub model_name: String,
}

impl ProxyConfig {
    pub fn from_env() -> Self {
        let host = std::env::var("PROXY_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port: u16 = std::env::var("PROXY_PORT")
            .unwrap_or_else(|_| "8080".to_string())
            .parse()
            .expect("PROXY_PORT must be a number");

        Self {
            claude_cli: std::env::var("CLAUDE_CLI").unwrap_or_else(|_| "claude".to_string()),
            bind_addr: SocketAddr::new(host.parse().expect("Invalid PROXY_HOST"), port),
            timeout_secs: std::env::var("INFERENCE_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok()),
            max_turns: std::env::var("MAX_TURNS")
                .ok()
                .and_then(|v| v.parse().ok()),
            model_name: std::env::var("MODEL_NAME")
                .unwrap_or_else(|_| "claude-opus-4-6".to_string()),
        }
    }
}
