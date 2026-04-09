use std::time::Instant;

use tokio::process::Command;

static START_TIME: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

pub fn init_start_time() {
    START_TIME.get_or_init(Instant::now);
}

fn uptime_str() -> String {
    let elapsed = START_TIME.get().map(|t| t.elapsed()).unwrap_or_default();
    let secs = elapsed.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// Handle an incoming message. Always returns a response.
pub async fn handle(text: &str) -> String {
    let text = text.trim();
    let (cmd, args) = match text.split_once(' ') {
        Some((c, a)) => (c, a.trim()),
        None => (text, ""),
    };

    match cmd {
        "/ping" => format!("🏓 pong (uptime: {})", uptime_str()),
        "/status" => cmd_status().await,
        "/restart" => cmd_restart().await,
        "/logs" => cmd_logs(args).await,
        "/exec" => {
            if args.is_empty() {
                "Usage: /exec <cmd>".into()
            } else {
                cmd_exec(args).await
            }
        }
        "/help" | "/start" => help_text(),
        _ => {
            if text.starts_with('/') {
                format!("❓ Неизвестная команда: {cmd}\n\n{}", help_text())
            } else {
                format!(
                    "🛟 Я — rescue-бот. Слежу за основной Софией и могу её перезапустить.\n\n{}",
                    help_text()
                )
            }
        }
    }
}

fn help_text() -> String {
    "🛟 sophia-rescue commands:\n\
     /ping — alive check\n\
     /status — main sophia process status\n\
     /restart — restart main sophia via launchctl\n\
     /logs [N] — last N lines of sophia logs (default 50)\n\
     /exec <cmd> — run shell command\n\
     /help — this message"
        .into()
}

async fn cmd_status() -> String {
    let output = Command::new("launchctl")
        .args(["list", "com.sophia.bot"])
        .output()
        .await;

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);

            if !out.status.success() {
                return format!(
                    "❌ sophia service not found in launchctl\n{}",
                    stderr.trim()
                );
            }

            let mut pid = "?".to_string();
            let mut exit_status = "?".to_string();
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() >= 3 && parts[2] == "com.sophia.bot" {
                    pid = parts[0].trim().replace('-', "not running");
                    exit_status = parts[1].trim().to_string();
                }
            }

            let log_age = std::fs::metadata("/tmp/sophia.log")
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map(|d| {
                    let secs = d.as_secs();
                    if secs < 60 {
                        format!("{secs}s ago")
                    } else if secs < 3600 {
                        format!("{}m ago", secs / 60)
                    } else {
                        format!("{}h ago", secs / 3600)
                    }
                })
                .unwrap_or_else(|| "unknown".into());

            format!(
                "📊 sophia status:\n\
                 PID: {pid}\n\
                 Exit status: {exit_status}\n\
                 Last log activity: {log_age}\n\
                 Rescue uptime: {}",
                uptime_str()
            )
        }
        Err(e) => format!("❌ Failed to check launchctl: {e}"),
    }
}

async fn cmd_restart() -> String {
    let uid_output = Command::new("id").arg("-u").output().await;
    let uid = match uid_output {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Err(_) => "501".to_string(),
    };

    let output = Command::new("launchctl")
        .args(["kickstart", "-k", &format!("gui/{uid}/com.sophia.bot")])
        .output()
        .await;

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                format!("✅ sophia restarted\n{}", stdout.trim())
            } else {
                format!(
                    "❌ restart failed (exit {})\n{}{}",
                    out.status.code().unwrap_or(-1),
                    stdout.trim(),
                    stderr.trim()
                )
            }
        }
        Err(e) => format!("❌ Failed to run launchctl: {e}"),
    }
}

async fn cmd_logs(args: &str) -> String {
    let n: usize = args.parse().unwrap_or(50);
    let n = n.min(200);

    let mut result = String::new();

    result.push_str("📄 /tmp/sophia.log:\n");
    match tail_file("/tmp/sophia.log", n).await {
        Ok(lines) => result.push_str(&lines),
        Err(e) => result.push_str(&format!("(error: {e})\n")),
    }

    result.push_str("\n📄 /tmp/sophia.err:\n");
    match tail_file("/tmp/sophia.err", n.min(30)).await {
        Ok(lines) => result.push_str(&lines),
        Err(e) => result.push_str(&format!("(error: {e})\n")),
    }

    if result.len() > 4000 {
        result.truncate(4000);
        result.push_str("\n... (truncated)");
    }

    result
}

async fn tail_file(path: &str, n: usize) -> Result<String, String> {
    let output = Command::new("tail")
        .args(["-n", &n.to_string(), path])
        .output()
        .await
        .map_err(|e| e.to_string())?;

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn cmd_exec(cmd: &str) -> String {
    let output = Command::new("sh").arg("-c").arg(cmd).output().await;

    match output {
        Ok(out) => {
            let mut result = String::new();
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);

            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str("stderr:\n");
                result.push_str(&stderr);
            }
            if result.is_empty() {
                result = format!("(exit code: {})", out.status.code().unwrap_or(-1));
            }

            if result.len() > 4000 {
                result.truncate(4000);
                result.push_str("\n... (truncated)");
            }

            result
        }
        Err(e) => format!("❌ exec failed: {e}"),
    }
}
