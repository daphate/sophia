use std::collections::VecDeque;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tracing::{debug, error, info, warn};

use crate::config::ProxyConfig;
use crate::convert::StreamJsonInput;
use crate::types::*;

/// Build Claude CLI command for stream-json bidirectional protocol.
fn build_cmd(config: &ProxyConfig, system_prompt: Option<&str>, model: Option<&str>) -> Command {
    let mut cmd = Command::new(&config.claude_cli);

    cmd.arg("-p")
        .arg("--input-format")
        .arg("stream-json")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--dangerously-skip-permissions");

    if let Some(sp) = system_prompt {
        if !sp.is_empty() {
            cmd.arg("--system-prompt").arg(sp);
        }
    }

    if let Some(m) = model {
        if !m.is_empty() {
            cmd.arg("--model").arg(m);
        }
    }

    if let Some(max_turns) = config.max_turns {
        cmd.arg("--max-turns").arg(max_turns.to_string());
    }

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    cmd
}

/// Write all stream-json input messages to the Claude CLI stdin, then close it.
async fn write_inputs(
    stdin: &mut tokio::process::ChildStdin,
    inputs: &[StreamJsonInput],
) -> Result<(), String> {
    for (i, input) in inputs.iter().enumerate() {
        let line =
            serde_json::to_string(input).map_err(|e| format!("Failed to serialize input: {e}"))?;
        let preview: String = line.chars().take(500).collect();
        debug!(
            "  stdin[{}]: {}{}",
            i,
            preview,
            if preview.len() < line.len() { "..." } else { "" }
        );
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("Failed to write to stdin: {e}"))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|e| format!("Failed to write newline: {e}"))?;
    }
    stdin
        .flush()
        .await
        .map_err(|e| format!("Failed to flush stdin: {e}"))?;
    Ok(())
}

/// Extract concatenated text from a ClaudeEvent's assistant message content blocks.
fn extract_text(event: &ClaudeEvent) -> String {
    event
        .message
        .as_ref()
        .and_then(|msg| msg.content.as_ref())
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.r#type == "text")
                .filter_map(|b| b.text.as_deref())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// Non-streaming: run Claude CLI and return the full response.
pub async fn call_claude(
    config: &ProxyConfig,
    inputs: &[StreamJsonInput],
    system_prompt: Option<&str>,
    model: Option<&str>,
) -> Result<(String, u64, u64), String> {
    info!(
        "Claude call: {} input messages, system_prompt={}, model={:?}",
        inputs.len(),
        system_prompt.map(|s| s.len()).unwrap_or(0),
        model,
    );

    let mut child = build_cmd(config, system_prompt, model)
        .spawn()
        .map_err(|e| format!("Failed to spawn Claude CLI: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        write_inputs(&mut stdin, inputs).await?;
        drop(stdin);
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(config.timeout_secs.unwrap_or(600)),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| "Claude CLI timed out".to_string())?
    .map_err(|e| format!("Claude CLI failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        error!(
            "Claude CLI error (rc={}): stderr={}, stdout={}",
            output.status,
            &stderr[..stderr.len().min(500)],
            &stdout[..stdout.len().min(500)]
        );
        return Err(format!(
            "Claude CLI exited with {}: {}",
            output.status,
            stderr.trim()
        ));
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let mut last_turn_text = String::new();
    let mut result_text = String::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let event: ClaudeEvent = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        match event.r#type.as_str() {
            "assistant" => {
                let text = extract_text(&event);
                if !text.is_empty() {
                    last_turn_text = text;
                }
                if let Some(msg) = &event.message {
                    if let Some(usage) = &msg.usage {
                        input_tokens += usage.input_tokens.unwrap_or(0);
                        output_tokens += usage.output_tokens.unwrap_or(0);
                    }
                }
            }
            "result" => {
                if let Some(text) = &event.result {
                    result_text = text.clone();
                }
                if let Some(usage) = &event.usage {
                    input_tokens = usage.input_tokens.unwrap_or(input_tokens);
                    output_tokens = usage.output_tokens.unwrap_or(output_tokens);
                }
            }
            _ => {}
        }
    }

    // Prefer result.result if longer (more complete), otherwise last assistant turn
    let final_text = if result_text.len() >= last_turn_text.len() && !result_text.is_empty() {
        result_text
    } else if !last_turn_text.is_empty() {
        last_turn_text
    } else {
        warn!("No text found in Claude output");
        String::new()
    };

    info!(
        "Response: {} chars, in={} out={}",
        final_text.len(),
        input_tokens,
        output_tokens
    );

    Ok((final_text.trim().to_string(), input_tokens, output_tokens))
}

// ── Streaming ──

pub enum StreamEvent {
    /// Incremental text to append to the current content block.
    TextDelta(String),
    /// A new assistant turn started (tool use completed); previous block should be closed.
    NewTurn,
    /// Stream is finished.
    Done,
}

pub struct ClaudeStream {
    child: tokio::process::Child,
    reader: BufReader<tokio::process::ChildStdout>,
    done: bool,
    /// Full text of the current assistant turn (used to compute deltas).
    current_turn_text: String,
    /// Pending events to return before reading more from the stream.
    pending: VecDeque<StreamEvent>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl ClaudeStream {
    pub async fn start(
        config: &ProxyConfig,
        inputs: &[StreamJsonInput],
        system_prompt: Option<&str>,
        model: Option<&str>,
    ) -> Result<Self, String> {
        let mut child = build_cmd(config, system_prompt, model)
            .spawn()
            .map_err(|e| format!("Failed to spawn Claude CLI: {e}"))?;

        if let Some(mut stdin) = child.stdin.take() {
            write_inputs(&mut stdin, inputs).await?;
            drop(stdin);
        }

        let stdout = child.stdout.take().ok_or("No stdout from Claude CLI")?;
        let reader = BufReader::new(stdout);

        Ok(Self {
            child,
            reader,
            done: false,
            current_turn_text: String::new(),
            pending: VecDeque::new(),
            input_tokens: 0,
            output_tokens: 0,
        })
    }

    /// Read the next stream event. Returns None when stream is finished.
    pub async fn next_event(&mut self) -> Option<StreamEvent> {
        if let Some(evt) = self.pending.pop_front() {
            return Some(evt);
        }

        if self.done {
            return None;
        }

        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line).await {
                Ok(0) => {
                    self.done = true;
                    return Some(StreamEvent::Done);
                }
                Ok(_) => {}
                Err(e) => {
                    error!("Error reading Claude stream: {e}");
                    self.done = true;
                    return Some(StreamEvent::Done);
                }
            }

            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let event: ClaudeEvent = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(e) => {
                    debug!("Skipping unparseable line: {e}");
                    continue;
                }
            };

            match event.r#type.as_str() {
                "assistant" => {
                    if let Some(msg) = &event.message {
                        if let Some(usage) = &msg.usage {
                            self.input_tokens = usage.input_tokens.unwrap_or(self.input_tokens);
                            self.output_tokens = usage.output_tokens.unwrap_or(self.output_tokens);
                        }
                    }

                    let full_text = extract_text(&event);
                    if full_text.is_empty() {
                        continue;
                    }

                    if self.current_turn_text.is_empty()
                        || full_text.starts_with(&self.current_turn_text)
                    {
                        // Same turn (or first turn) — text is growing by appending.
                        // Emit the new portion as a delta.
                        if full_text.len() > self.current_turn_text.len() {
                            // Safe to slice: full_text starts with current_turn_text,
                            // so the boundary is at a valid UTF-8 char boundary.
                            let offset = self.current_turn_text.len();
                            self.current_turn_text = full_text;
                            let delta = &self.current_turn_text[offset..];
                            return Some(StreamEvent::TextDelta(delta.to_string()));
                        }
                        // Text didn't grow — nothing new to emit.
                        continue;
                    } else {
                        // New turn — previous turn's text is replaced.
                        debug!(
                            "New turn detected: prev={} chars, new={} chars",
                            self.current_turn_text.len(),
                            full_text.len()
                        );
                        self.current_turn_text = full_text.clone();
                        // Queue the new turn's text as a delta to be emitted after NewTurn.
                        self.pending
                            .push_back(StreamEvent::TextDelta(full_text));
                        return Some(StreamEvent::NewTurn);
                    }
                }
                "result" => {
                    self.done = true;

                    if let Some(usage) = &event.usage {
                        self.input_tokens = usage.input_tokens.unwrap_or(self.input_tokens);
                        self.output_tokens = usage.output_tokens.unwrap_or(self.output_tokens);
                    }

                    // Check if result.result has text beyond what we already streamed.
                    let result_text = event.result.as_deref().unwrap_or("");
                    if !result_text.is_empty()
                        && result_text.len() > self.current_turn_text.len()
                        && result_text.starts_with(&self.current_turn_text)
                    {
                        let delta = &result_text[self.current_turn_text.len()..];
                        if !delta.is_empty() {
                            self.pending.push_back(StreamEvent::Done);
                            return Some(StreamEvent::TextDelta(delta.to_string()));
                        }
                    }

                    return Some(StreamEvent::Done);
                }
                _ => continue,
            }
        }
    }

    pub async fn cleanup(&mut self) {
        let _ = self.child.kill().await;
    }
}
