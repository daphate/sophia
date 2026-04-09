use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tracing::{error, warn};

use crate::config::Config;
use crate::memory::{build_system_prompt, load_recent_dialog};

#[derive(Debug)]
pub struct CostInfo {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, thiserror::Error)]
pub enum InferenceError {
    #[error("Claude CLI timed out after {0}s")]
    Timeout(u64),
    #[error("Claude CLI not found at: {0}")]
    CliNotFound(String),
    #[error("Claude CLI error (rc={code}): {stderr}")]
    CliError { code: i32, stderr: String },
    #[error("Failed to parse Claude output: {0}")]
    ParseError(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

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
        move || build_system_prompt(&recent)
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

    // Wait with timeout
    let output = tokio::time::timeout(
        Duration::from_secs(config.inference_timeout),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| {
        // Kill the process on timeout
        InferenceError::Timeout(config.inference_timeout)
    })?
    .map_err(|e| InferenceError::Other(e.into()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let truncated = if stderr.len() > 500 {
            let mut end = 500;
            while !stderr.is_char_boundary(end) {
                end -= 1;
            }
            &stderr[..end]
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
