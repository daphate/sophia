use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json};
use futures::stream;
use tracing::{debug, error, info};

use crate::claude::{self, StreamEvent};
use crate::config::ProxyConfig;
use crate::convert;
use crate::types::*;

fn message_id() -> String {
    format!("msg_{}", uuid::Uuid::new_v4().simple())
}

fn error_json(status: StatusCode, error_type: &str, message: &str) -> impl IntoResponse {
    (
        status,
        Json(ErrorResponse {
            r#type: "error",
            error: ApiError {
                r#type: error_type.to_string(),
                message: message.to_string(),
            },
        }),
    )
}

/// Map incoming model name to a Claude CLI model flag.
fn resolve_cli_model(model: &Option<String>) -> Option<String> {
    let m = model.as_deref()?;
    let clean = m
        .trim_start_matches("sophia-proxy/")
        .trim_start_matches("secondf8n/")
        .trim_start_matches("openrouter/")
        .trim_start_matches("anthropic/");

    match clean {
        s if s.contains("claude") => Some(s.to_string()),
        "sophia" => None,
        _ => Some(clean.to_string()),
    }
}

// POST /v1/messages
pub async fn create_message(
    State(config): State<ProxyConfig>,
    Json(req): Json<MessagesRequest>,
) -> impl IntoResponse {
    if req.messages.is_empty() {
        return error_json(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "messages array is empty",
        )
        .into_response();
    }

    let model_label = req
        .model
        .clone()
        .unwrap_or_else(|| config.model_name.clone());

    info!(
        "Messages: model={}, messages={}, stream={}",
        model_label,
        req.messages.len(),
        req.stream
    );

    // Debug log incoming messages
    for (i, msg) in req.messages.iter().enumerate() {
        let preview = match &msg.content {
            MessageContent::Text(s) => {
                let t: String = s.chars().take(200).collect();
                if t.len() < s.len() {
                    format!("{t}...")
                } else {
                    t
                }
            }
            MessageContent::Blocks(blocks) => format!("[{} blocks]", blocks.len()),
        };
        debug!("  msg[{}] role={} content={}", i, msg.role, preview);
    }

    // Extract system prompt from request
    let system_prompt = req.system.as_ref().map(|sp| sp.as_text());

    // Convert messages to CLI stream-json format
    let inputs = convert::convert_messages(&req.messages).await;
    let cli_model = resolve_cli_model(&req.model);

    if req.stream {
        handle_streaming(config, inputs, system_prompt, cli_model, model_label)
            .await
            .into_response()
    } else {
        handle_non_streaming(config, inputs, system_prompt, cli_model, model_label)
            .await
            .into_response()
    }
}

async fn handle_non_streaming(
    config: ProxyConfig,
    inputs: Vec<convert::StreamJsonInput>,
    system_prompt: Option<String>,
    cli_model: Option<String>,
    model_label: String,
) -> impl IntoResponse {
    match claude::call_claude(
        &config,
        &inputs,
        system_prompt.as_deref(),
        cli_model.as_deref(),
    )
    .await
    {
        Ok((text, input_tokens, output_tokens)) => Json(MessagesResponse {
            id: message_id(),
            r#type: "message",
            role: "assistant",
            content: vec![ResponseContentBlock::Text { text }],
            model: model_label,
            stop_reason: "end_turn",
            stop_sequence: None,
            usage: Usage {
                input_tokens,
                output_tokens,
            },
        })
        .into_response(),
        Err(e) => {
            error!("Claude error: {e}");
            error_json(StatusCode::INTERNAL_SERVER_ERROR, "api_error", &e).into_response()
        }
    }
}

async fn handle_streaming(
    config: ProxyConfig,
    inputs: Vec<convert::StreamJsonInput>,
    system_prompt: Option<String>,
    cli_model: Option<String>,
    model_label: String,
) -> impl IntoResponse {
    let claude_stream = match claude::ClaudeStream::start(
        &config,
        &inputs,
        system_prompt.as_deref(),
        cli_model.as_deref(),
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            error!("Claude stream error: {e}");
            return error_json(StatusCode::INTERNAL_SERVER_ERROR, "api_error", &e).into_response();
        }
    };

    let id = message_id();
    let (tx, rx) = tokio::sync::mpsc::channel::<Event>(64);

    tokio::spawn(async move {
        let mut claude = claude_stream;
        let mut block_index: u32 = 0;
        let mut block_open = false;

        // Send message_start
        let msg_start = MessageStartEvent {
            r#type: "message_start",
            message: MessageStartBody {
                id: id.clone(),
                r#type: "message",
                role: "assistant",
                content: [],
                model: model_label.clone(),
                stop_reason: None,
                stop_sequence: None,
                usage: Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
            },
        };
        let _ = tx
            .send(
                Event::default()
                    .event("message_start")
                    .data(serde_json::to_string(&msg_start).unwrap()),
            )
            .await;

        // Send initial ping
        let _ = tx
            .send(
                Event::default()
                    .event("ping")
                    .data(r#"{"type":"ping"}"#),
            )
            .await;

        loop {
            match claude.next_event().await {
                Some(StreamEvent::TextDelta(text)) => {
                    if !block_open {
                        let cbs = ContentBlockStartEvent {
                            r#type: "content_block_start",
                            index: block_index,
                            content_block: ContentBlockRef {
                                r#type: "text",
                                text: "",
                            },
                        };
                        let _ = tx
                            .send(
                                Event::default()
                                    .event("content_block_start")
                                    .data(serde_json::to_string(&cbs).unwrap()),
                            )
                            .await;
                        block_open = true;
                    }

                    let cbd = ContentBlockDeltaEvent {
                        r#type: "content_block_delta",
                        index: block_index,
                        delta: TextDelta {
                            r#type: "text_delta",
                            text,
                        },
                    };
                    let _ = tx
                        .send(
                            Event::default()
                                .event("content_block_delta")
                                .data(serde_json::to_string(&cbd).unwrap()),
                        )
                        .await;
                }
                Some(StreamEvent::NewTurn) => {
                    if block_open {
                        let cbs = ContentBlockStopEvent {
                            r#type: "content_block_stop",
                            index: block_index,
                        };
                        let _ = tx
                            .send(
                                Event::default()
                                    .event("content_block_stop")
                                    .data(serde_json::to_string(&cbs).unwrap()),
                            )
                            .await;
                        block_index += 1;
                        block_open = false;
                    }
                }
                Some(StreamEvent::Done) => {
                    if block_open {
                        let cbs = ContentBlockStopEvent {
                            r#type: "content_block_stop",
                            index: block_index,
                        };
                        let _ = tx
                            .send(
                                Event::default()
                                    .event("content_block_stop")
                                    .data(serde_json::to_string(&cbs).unwrap()),
                            )
                            .await;
                    }

                    let md = MessageDeltaEvent {
                        r#type: "message_delta",
                        delta: MessageDeltaBody {
                            stop_reason: "end_turn",
                            stop_sequence: None,
                        },
                        usage: Usage {
                            input_tokens: claude.input_tokens,
                            output_tokens: claude.output_tokens,
                        },
                    };
                    let _ = tx
                        .send(
                            Event::default()
                                .event("message_delta")
                                .data(serde_json::to_string(&md).unwrap()),
                        )
                        .await;

                    let ms = MessageStopEvent {
                        r#type: "message_stop",
                    };
                    let _ = tx
                        .send(
                            Event::default()
                                .event("message_stop")
                                .data(serde_json::to_string(&ms).unwrap()),
                        )
                        .await;

                    break;
                }
                None => break,
            }
        }

        claude.cleanup().await;
    });

    // Convert the mpsc receiver into an SSE stream
    let sse_stream = stream::unfold(rx, |mut rx| async {
        let event = rx.recv().await?;
        Some((Ok::<_, std::convert::Infallible>(event), rx))
    });

    Sse::new(sse_stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

// GET /v1/models
pub async fn list_models(State(config): State<ProxyConfig>) -> Json<ModelsResponse> {
    Json(ModelsResponse {
        object: "list",
        data: vec![
            model_entry("secondf8n/sophia", "sophia-proxy"),
            model_entry(&config.model_name, "sophia-proxy"),
            model_entry("claude-opus-4-6", "anthropic"),
            model_entry("claude-sonnet-4-6", "anthropic"),
            model_entry("claude-haiku-4-5-20251001", "anthropic"),
        ],
    })
}

fn model_entry(id: &str, owned_by: &str) -> ModelInfo {
    ModelInfo {
        id: id.to_string(),
        object: "model",
        created: 1700000000,
        owned_by: owned_by.to_string(),
    }
}

// GET /v1/models/{model_id}
pub async fn get_model(
    State(config): State<ProxyConfig>,
    axum::extract::Path(model_id): axum::extract::Path<String>,
) -> Json<ModelInfo> {
    Json(ModelInfo {
        id: model_id,
        object: "model",
        created: 1700000000,
        owned_by: if config.model_name.contains("claude") {
            "anthropic"
        } else {
            "sophia-proxy"
        }
        .to_string(),
    })
}
