use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use crate::config::Config;
use crate::memory::{build_system_prompt, load_recent_dialog};

#[derive(Debug)]
pub struct CostInfo {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
}

/// Events emitted during streaming inference.
#[derive(Debug)]
pub enum StreamEvent {
    /// A chunk of text from the model.
    TextDelta(String),
    /// Inference completed with full text and optional cost.
    Done {
        full_text: String,
        cost: Option<CostInfo>,
    },
    /// An error occurred.
    Error(String),
}

#[derive(Debug, thiserror::Error)]
pub enum InferenceError {
#[error("Claude CLI not found at: {0}")]
    CliNotFound(String),
    #[error("Claude CLI error (rc={code}): {stderr}")]
    CliError { code: i32, stderr: String },
    #[error("Failed to parse Claude output: {0}")]
    ParseError(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[allow(dead_code)]
pub async fn ask_claude(
    user_id: i64,
    message: &str,
    config: &Config,
    file_paths: Option<&[PathBuf]>,
) -> Result<(String, Option<CostInfo>), InferenceError> {
    let recent = tokio::task::spawn_blocking({
        let user_id = user_id;
        move || load_recent_dialog(user_id, 15, 3000)
    })
    .await
    .map_err(|e| InferenceError::Other(e.into()))?;

    let system_prompt = tokio::task::spawn_blocking({
        let recent = recent.clone();
        move || build_system_prompt(&recent, "")
    })
    .await
    .map_err(|e| InferenceError::Other(e.into()))?;

    // Build prompt with file references
    let mut prompt_parts = Vec::new();
    if let Some(paths) = file_paths {
        for fp in paths {
            prompt_parts.push(format!("[Attached file: {}]", fp.display()));
        }
        if !prompt_parts.is_empty() {
            prompt_parts.push(String::new());
        }
    }
    prompt_parts.push(message.to_string());
    let full_message = prompt_parts.join("\n");

    let mut cmd = tokio::process::Command::new(&config.claude_cli);
    cmd.args([
        "-p",
        "--output-format",
        "json",
        "--verbose",
        "--dangerously-skip-permissions",
        "--system-prompt",
        &system_prompt,
    ]);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            InferenceError::CliNotFound(config.claude_cli.clone())
        } else {
            InferenceError::Other(e.into())
        }
    })?;

    // Write to stdin
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(full_message.as_bytes()).await.map_err(|e| {
            InferenceError::Other(anyhow::anyhow!("Failed to write to stdin: {}", e))
        })?;
    }

    // Poll process status every 30s instead of hard timeout.
    // We read stdout/stderr ourselves so we can poll with try_wait().
    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut r) = stdout_handle {
            tokio::io::AsyncReadExt::read_to_end(&mut r, &mut buf).await.ok();
        }
        buf
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut r) = stderr_handle {
            tokio::io::AsyncReadExt::read_to_end(&mut r, &mut buf).await.ok();
        }
        buf
    });

    // Poll every 2s for quick responses, log every 30s for long ones.
    let poll_interval = Duration::from_secs(2);
    let mut elapsed_secs: u64 = 0;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                tokio::time::sleep(poll_interval).await;
                elapsed_secs += 2;
                if elapsed_secs % 30 == 0 {
                    debug!(
                        "Claude CLI still running after {}s, waiting...",
                        elapsed_secs
                    );
                }
            }
            Err(e) => {
                return Err(InferenceError::Other(anyhow::anyhow!(
                    "Failed to poll Claude CLI: {}",
                    e
                )));
            }
        }
    };

    let stdout_bytes = stdout_task.await.map_err(|e| InferenceError::Other(e.into()))?;
    let stderr_bytes = stderr_task.await.map_err(|e| InferenceError::Other(e.into()))?;

    let output = std::process::Output {
        status: std::process::ExitStatus::from(status),
        stdout: stdout_bytes,
        stderr: stderr_bytes,
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let truncated = if stderr.len() > 500 {
            &stderr[..stderr.floor_char_boundary(500)]
        } else {
            &stderr
        };
        error!("Claude CLI error (rc={}): {}", output.status.code().unwrap_or(-1), truncated);
        return Err(InferenceError::CliError {
            code: output.status.code().unwrap_or(-1),
            stderr: truncated.to_string(),
        });
    }

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return Err(InferenceError::ParseError("Empty output".into()));
    }

    match parse_claude_output(&raw) {
        Ok(result) => Ok(result),
        Err(e) => {
            warn!("Failed to parse Claude output ({}), returning raw", e);
            Ok((raw, None))
        }
    }
}

#[allow(dead_code)]
fn parse_claude_output(raw: &str) -> Result<(String, Option<CostInfo>)> {
    let data: Value = serde_json::from_str(raw)?;

    let items = if data.is_array() {
        data.as_array().unwrap().clone()
    } else {
        vec![data]
    };

    let mut assistant_texts: Vec<Vec<String>> = Vec::new();
    let mut result_text = String::new();
    let mut total_input: u64 = 0;
    let mut total_output: u64 = 0;
    let mut cost_usd: Option<f64> = None;

    for item in &items {
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match item_type {
            "assistant" => {
                let msg = item.get("message").unwrap_or(item);
                let mut texts = Vec::new();
                if let Some(content) = msg.get("content").and_then(|c| c.as_array()) {
                    for block in content {
                        if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                            if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                                texts.push(t.to_string());
                            }
                        }
                    }
                }
                if !texts.is_empty() {
                    assistant_texts.push(texts);
                }
                if let Some(usage) = msg.get("usage") {
                    total_input += usage
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    total_output += usage
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                }
            }
            "result" => {
                if let Some(r) = item.get("result").and_then(|v| v.as_str()) {
                    result_text = r.to_string();
                }
                if let Some(c) = item.get("cost_usd").and_then(|v| v.as_f64()) {
                    cost_usd = Some(c);
                }
                total_input = item
                    .get("total_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(total_input);
                total_output = item
                    .get("total_output_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(total_output);
            }
            _ => {}
        }
    }

    let final_text = if !result_text.is_empty() {
        result_text
    } else if let Some(last) = assistant_texts.last() {
        last.join("\n")
    } else {
        raw.to_string()
    };

    let cost = Some(CostInfo {
        input_tokens: total_input,
        output_tokens: total_output,
        cost_usd,
    });

    Ok((final_text, cost))
}

/// Streaming version of ask_claude. Sends text deltas through a channel
/// as they arrive from the Claude CLI, then sends a Done event with the
/// full text and cost info.
pub async fn ask_claude_streaming(
    user_id: i64,
    message: &str,
    config: &Config,
    file_paths: Option<&[PathBuf]>,
    semantic_context: &str,
) -> Result<mpsc::Receiver<StreamEvent>, InferenceError> {
    let recent = tokio::task::spawn_blocking({
        let user_id = user_id;
        move || load_recent_dialog(user_id, 15, 3000)
    })
    .await
    .map_err(|e| InferenceError::Other(e.into()))?;

    let semantic_ctx = semantic_context.to_string();
    let system_prompt = tokio::task::spawn_blocking({
        let recent = recent.clone();
        move || build_system_prompt(&recent, &semantic_ctx)
    })
    .await
    .map_err(|e| InferenceError::Other(e.into()))?;

    // Build prompt with file references
    let mut prompt_parts = Vec::new();
    if let Some(paths) = file_paths {
        for fp in paths {
            prompt_parts.push(format!("[Attached file: {}]", fp.display()));
        }
        if !prompt_parts.is_empty() {
            prompt_parts.push(String::new());
        }
    }
    prompt_parts.push(message.to_string());
    let full_message = prompt_parts.join("\n");

    let mut cmd = tokio::process::Command::new(&config.claude_cli);
    cmd.args([
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--dangerously-skip-permissions",
        "--system-prompt",
        &system_prompt,
    ]);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            InferenceError::CliNotFound(config.claude_cli.clone())
        } else {
            InferenceError::Other(e.into())
        }
    })?;

    // Write to stdin
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(full_message.as_bytes()).await.map_err(|e| {
            InferenceError::Other(anyhow::anyhow!("Failed to write to stdin: {}", e))
        })?;
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| InferenceError::Other(anyhow::anyhow!("No stdout from Claude CLI")))?;

    let (tx, rx) = mpsc::channel::<StreamEvent>(64);

    // Spawn background task that reads stream-json lines and sends events
    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut full_text = String::new();
        let mut cost_info: Option<CostInfo> = None;

        loop {
            let line = match tokio::time::timeout(Duration::from_secs(300), lines.next_line()).await {
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) => break, // EOF
                Ok(Err(e)) => {
                    let _ = tx.send(StreamEvent::Error(format!("Read error: {}", e))).await;
                    let _ = child.kill().await;
                    return;
                }
                Err(_) => {
                    // 5 min with no output
                    let _ = tx.send(StreamEvent::Error("Claude CLI idle timeout (5 min no output)".to_string())).await;
                    let _ = child.kill().await;
                    return;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let parsed: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let event_type = parsed
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            match event_type {
                "stream_event" => {
                    if let Some(event) = parsed.get("event") {
                        let inner_type =
                            event.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        if inner_type == "content_block_delta" {
                            if let Some(delta) = event.get("delta") {
                                if delta.get("type").and_then(|v| v.as_str())
                                    == Some("text_delta")
                                {
                                    if let Some(text) =
                                        delta.get("text").and_then(|v| v.as_str())
                                    {
                                        full_text.push_str(text);
                                        if tx.send(StreamEvent::TextDelta(text.to_string()))
                                            .await
                                            .is_err()
                                        {
                                            break; // receiver dropped
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                "result" => {
                    // Extract final text from result if available
                    if let Some(r) = parsed.get("result").and_then(|v| v.as_str()) {
                        full_text = r.to_string();
                    }
                    let total_input = parsed
                        .get("usage")
                        .and_then(|u| u.get("input_tokens"))
                        .and_then(|v| v.as_u64())
                        .or_else(|| {
                            parsed
                                .get("total_input_tokens")
                                .and_then(|v| v.as_u64())
                        })
                        .unwrap_or(0);
                    let total_output = parsed
                        .get("usage")
                        .and_then(|u| u.get("output_tokens"))
                        .and_then(|v| v.as_u64())
                        .or_else(|| {
                            parsed
                                .get("total_output_tokens")
                                .and_then(|v| v.as_u64())
                        })
                        .unwrap_or(0);
                    let cost_usd = parsed
                        .get("total_cost_usd")
                        .and_then(|v| v.as_f64());
                    cost_info = Some(CostInfo {
                        input_tokens: total_input,
                        output_tokens: total_output,
                        cost_usd,
                    });
                }
                _ => {}
            }
        }

        // Wait for process to finish with 10-minute timeout
        let status = match tokio::time::timeout(Duration::from_secs(600), child.wait()).await {
            Ok(result) => result,
            Err(_) => {
                // Timeout — kill the process
                let _ = child.kill().await;
                let _ = tx.send(StreamEvent::Error("Claude CLI timed out after 10 minutes".to_string())).await;
                return;
            }
        };
        if let Ok(st) = &status {
            if !st.success() {
                let _ = tx
                    .send(StreamEvent::Error(format!(
                        "Claude CLI exited with code {}",
                        st.code().unwrap_or(-1)
                    )))
                    .await;
                return;
            }
        }

        let _ = tx
            .send(StreamEvent::Done {
                full_text,
                cost: cost_info,
            })
            .await;
    });

    Ok(rx)
}
