use std::time::Instant;

use tokio::process::Command;
use tracing::{error, info};

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
                // Plain text → forward to Claude CLI for conversation
                cmd_chat(text).await
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
                let line = line.trim().trim_end_matches(';');
                if let Some((key, value)) = line.split_once(" = ") {
                    let key = key.trim().trim_matches('"');
                    let value = value.trim().trim_matches('"');
                    match key {
                        "PID" => pid = value.to_string(),
                        "LastExitStatus" => exit_status = value.to_string(),
                        _ => {}
                    }
                }
            }
            if pid == "?" {
                pid = "not running".to_string();
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
        let boundary = result.floor_char_boundary(4000);
        result.truncate(boundary);
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

async fn cmd_chat(user_text: &str) -> String {
    let sophia_root = std::env::var("SOPHIA_ROOT")
        .unwrap_or_else(|_| "/Users/lokitheone/sophia".into());
    let claude_cli = std::env::var("CLAUDE_CLI").unwrap_or_else(|_| "claude".into());

    let system_prompt = build_rescue_system_prompt(&sophia_root).await;

    info!("Calling Claude CLI for chat: {} chars", user_text.len());

    let result = Command::new(&claude_cli)
        .args([
            "-p",
            "--output-format", "json",
            "--dangerously-skip-permissions",
            "--system-prompt", &system_prompt,
        ])
        .current_dir(&sophia_root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    let mut child = match result {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to spawn Claude CLI: {e}");
            return format!("❌ Не удалось запустить Claude: {e}");
        }
    };

    // Write user text to stdin
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(user_text.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }

    // Wait for result (up to 120s)
    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(120),
        child.wait_with_output(),
    )
    .await
    {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            error!("Claude CLI error: {e}");
            return format!("❌ Ошибка Claude: {e}");
        }
        Err(_) => {
            error!("Claude CLI timed out after 120s");
            return "⏱ Claude не ответил за 120 секунд.".into();
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse JSON response — extract "result" field
    if let Some(text) = extract_claude_response(&stdout) {
        if text.is_empty() {
            "🤷 Claude вернул пустой ответ.".into()
        } else if text.len() > 4000 {
            let boundary = text.floor_char_boundary(4000);
            format!("{}\n... (truncated)", &text[..boundary])
        } else {
            text
        }
    } else {
        // Fallback: try raw stdout
        let raw = stdout.trim();
        if raw.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Claude CLI empty output. stderr: {}", stderr.chars().take(500).collect::<String>());
            "❌ Claude не вернул ответ.".into()
        } else if raw.len() > 4000 {
            let boundary = raw.floor_char_boundary(4000);
            format!("{}\n... (truncated)", &raw[..boundary])
        } else {
            raw.to_string()
        }
    }
}

fn extract_claude_response(json_str: &str) -> Option<String> {
    // Claude JSON output: find last line that parses as JSON with "result" field
    for line in json_str.lines().rev() {
        let line = line.trim();
        if line.starts_with('{') {
            // Try to extract "result" field manually (avoid serde dependency)
            if let Some(pos) = line.find("\"result\"") {
                // Find the string value after "result":
                let after = &line[pos + 8..]; // skip "result"
                if let Some(colon) = after.find(':') {
                    let after_colon = after[colon + 1..].trim();
                    if after_colon.starts_with('"') {
                        // Extract quoted string value
                        let content = &after_colon[1..];
                        if let Some(end) = find_unescaped_quote(content) {
                            let raw = &content[..end];
                            return Some(unescape_json_string(raw));
                        }
                    }
                }
            }
        }
    }
    None
}

fn find_unescaped_quote(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2; // skip escaped char
        } else if bytes[i] == b'"' {
            return Some(i);
        } else {
            i += 1;
        }
    }
    None
}

fn unescape_json_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some('"') => result.push('"'),
                Some('\\') => result.push('\\'),
                Some('/') => result.push('/'),
                Some(other) => {
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }
    result
}

async fn build_rescue_system_prompt(sophia_root: &str) -> String {
    let instructions_dir = format!("{sophia_root}/data/instructions");
    let mut prompt = String::new();

    // Load key instruction files
    for filename in ["SOUL.md", "IDENTITY.md", "USER.md"] {
        let path = format!("{instructions_dir}/{filename}");
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            prompt.push_str(&format!("# {filename}\n{content}\n\n"));
        }
    }

    prompt.push_str(
        "# Контекст\n\
         Ты отвечаешь через rescue-бота (дублёр). Основная София может быть недоступна.\n\
         У тебя есть доступ к рабочей директории sophia и всем инструментам Claude Code.\n\
         Отвечай как София — кратко, по делу, с характером.\n\
         Если просят починить или проверить основную Софию — используй доступные инструменты.\n"
    );

    prompt
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
                let boundary = result.floor_char_boundary(4000);
                result.truncate(boundary);
                result.push_str("\n... (truncated)");
            }

            result
        }
        Err(e) => format!("❌ exec failed: {e}"),
    }
}
