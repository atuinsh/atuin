// ───────────────────────────────────────────────────────────────────
// SSE streaming
// ───────────────────────────────────────────────────────────────────

use atuin_client::settings::AiCapabilities;
use atuin_common::tls::ensure_crypto_provider;

use eventsource_stream::Eventsource;
use eyre::{Context, Result};
use futures::StreamExt;
use reqwest::Url;
use reqwest::header::USER_AGENT;

use crate::context::ClientContext;

static APP_USER_AGENT: &str = concat!("atuin/", env!("CARGO_PKG_VERSION"));

/// Frames that alter the stream lifecycle — terminal or state-changing.
#[derive(Debug, Clone)]
pub(crate) enum StreamControl {
    Done { session_id: String },
    Error(String),
    StatusChanged(String),
}

/// Frames that carry conversation content — they mutate the event log.
#[derive(Debug, Clone)]
pub(crate) enum StreamContent {
    TextChunk(String),
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
        remote: bool,
        content_length: Option<usize>,
    },
}

/// A frame from the SSE stream, classified as control or content.
#[derive(Debug, Clone)]
pub(crate) enum StreamFrame {
    Content(StreamContent),
    Control(StreamControl),
}

/// Per-turn request payload for the chat API.
pub(crate) struct ChatRequest {
    pub messages: Vec<serde_json::Value>,
    pub session_id: Option<String>,
    pub capabilities: Vec<String>,
    pub invocation_id: String,
}

impl ChatRequest {
    pub(crate) fn new(
        messages: Vec<serde_json::Value>,
        session_id: Option<String>,
        capabilities: &AiCapabilities,
        invocation_id: String,
    ) -> Self {
        let mut caps = vec!["client_invocations".to_string()];
        if capabilities.enable_history_search.unwrap_or(true) {
            caps.push("client_v1_atuin_history".to_string());
        }
        if capabilities.enable_file_tools.unwrap_or(true) {
            caps.push("client_v1_read_file".to_string());
            caps.push("client_v1_edit_file".to_string());
            caps.push("client_v1_write_file".to_string());
        }
        if capabilities.enable_command_execution.unwrap_or(true) {
            caps.push("client_v1_execute_shell_command".to_string());
        }
        if let Ok(extra) = std::env::var("ATUIN_AI__ADDITIONAL_CAPS") {
            caps.extend(
                extra
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            );
        }

        Self {
            messages,
            session_id,
            capabilities: caps,
            invocation_id,
        }
    }
}

pub(crate) fn create_chat_stream(
    hub_address: String,
    token: String,
    request: ChatRequest,
    client_ctx: ClientContext,
    send_cwd: bool,
    last_command: Option<String>,
) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamFrame>> + Send>> {
    Box::pin(async_stream::stream! {
        ensure_crypto_provider();
        let endpoint = match hub_url(&hub_address, "/api/cli/chat") {
            Ok(url) => url,
            Err(e) => {
                yield Err(e);
                return;
            }
        };

        tracing::debug!("Sending SSE request to {endpoint}");

        let context = client_ctx.to_json(send_cwd, last_command.as_deref());

        let mut request_body = serde_json::json!({
            "messages": request.messages,
            "context": context,
            "capabilities": request.capabilities,
            "invocation_id": request.invocation_id
        });

        if let Some(ref sid) = request.session_id {
            tracing::trace!("Including session_id in request: {sid}");
            request_body["session_id"] = serde_json::json!(sid);
        }

        let client = reqwest::Client::new();
        let response = match client
            .post(endpoint.clone())
            .header("Accept", "text/event-stream")
            .header(USER_AGENT, APP_USER_AGENT)
            .bearer_auth(&token)
            .json(&request_body)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                yield Err(eyre::eyre!("Failed to send SSE request: {}", e));
                return;
            }
        };

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            tracing::error!("SSE request failed with status: {status}, clearing session");
            let _ = atuin_client::hub::delete_session().await;
            yield Err(eyre::eyre!("Hub session expired. Re-run to authenticate again."));
            return;
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            tracing::error!("SSE request failed ({}): {}", status, body);
            yield Err(eyre::eyre!("SSE request failed ({}): {}", status, body));
            return;
        }

        let byte_stream = response.bytes_stream();
        let mut stream = byte_stream.eventsource();

        while let Some(event) = stream.next().await {
            match event {
                Ok(sse_event) => {
                    let event_type = sse_event.event.as_str();
                    let data = sse_event.data.clone();

                    tracing::debug!(event_type = %event_type, "SSE event received");

                    match event_type {
                        "text" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data)
                                && let Some(content) = json.get("content").and_then(|v| v.as_str())
                            {
                                yield Ok(StreamFrame::Content(StreamContent::TextChunk(content.to_string())));
                            }
                        }
                        "tool_call" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                                let id = json.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let name = json.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let input = json.get("input").cloned().unwrap_or(serde_json::json!({}));
                                yield Ok(StreamFrame::Content(StreamContent::ToolCall { id, name, input }));
                            }
                        }
                        "tool_result" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                                let tool_use_id = json.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let content = json.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let is_error = json.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                                let remote = json.get("remote").and_then(|v| v.as_bool()).unwrap_or(false);
                                let content_length = json.get("content_length").and_then(|v| v.as_u64()).map(|v| v as usize);
                                yield Ok(StreamFrame::Content(StreamContent::ToolResult { tool_use_id, content, is_error, remote, content_length }));
                            }
                        }
                        "status" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data)
                                && let Some(state) = json.get("state").and_then(|v| v.as_str())
                            {
                                yield Ok(StreamFrame::Control(StreamControl::StatusChanged(state.to_string())));
                            }
                        }
                        "done" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                                let session_id = json.get("session_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                yield Ok(StreamFrame::Control(StreamControl::Done { session_id }));
                            } else {
                                yield Ok(StreamFrame::Control(StreamControl::Done { session_id: String::new() }));
                            }
                            break;
                        }
                        "error" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                                let message = json.get("message").and_then(|v| v.as_str()).unwrap_or("Unknown error").to_string();
                                tracing::error!("SSE error: {}", message);
                                yield Ok(StreamFrame::Control(StreamControl::Error(message)));
                            } else {
                                tracing::error!("SSE error: {}", data);
                                yield Ok(StreamFrame::Control(StreamControl::Error(data)));
                            }
                            break;
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    yield Err(eyre::eyre!("SSE error: {}", e));
                    break;
                }
            }
        }
    })
}

fn hub_url(base: &str, path: &str) -> Result<Url> {
    let base_with_slash = if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    };
    let stripped = path.strip_prefix('/').unwrap_or(path);
    Url::parse(&base_with_slash)?
        .join(stripped)
        .context("failed to build hub URL")
}
