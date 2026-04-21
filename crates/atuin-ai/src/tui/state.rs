//! Core state types for the conversation protocol.
//!
//! ConversationEvent and events_to_messages are the canonical representations
//! used by both the FSM and the context window builder. AppMode is used by
//! the view layer for component prop derivation.

/// Conversation event types matching the API protocol.
#[derive(Debug, Clone)]
pub(crate) enum ConversationEvent {
    /// User message (what the user typed)
    UserMessage { content: String },
    /// Text content from assistant (streamed or complete)
    Text { content: String },
    /// Tool call from assistant
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool result (from server-side or client-side execution)
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
        /// Server-side results are stored in the DB; the client sends an opaque
        /// reference (`remote: true`) instead of the full content.
        remote: bool,
        /// Approximate content length for token estimation of remote results.
        content_length: Option<usize>,
    },
    /// Out-of-band output from the system — not sent to the server
    OutOfBandOutput {
        name: String,
        command: Option<String>,
        content: String,
    },
    /// Context injected for the LLM that is not rendered in the TUI.
    /// Converted to a user message in the API protocol.
    SystemContext { content: String },
}

impl ConversationEvent {
    /// Whether this event represents actual conversation content sent to the API.
    pub(crate) fn is_api_content(&self) -> bool {
        match self {
            ConversationEvent::UserMessage { .. } => true,
            ConversationEvent::Text { .. } => true,
            ConversationEvent::ToolCall { .. } => true,
            ConversationEvent::ToolResult { .. } => true,
            ConversationEvent::OutOfBandOutput { .. } => false,
            ConversationEvent::SystemContext { .. } => false,
        }
    }

    /// Extract command from a suggest_command tool call.
    pub(crate) fn as_command(&self) -> Option<&str> {
        if let ConversationEvent::ToolCall { name, input, .. } = self
            && name == "suggest_command"
        {
            return input.get("command").and_then(|v| v.as_str());
        }
        None
    }
}

/// Application mode for key handling and component props.
///
/// Derived from AgentState in the view layer via `From<&AgentState>`.
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub(crate) enum AppMode {
    /// User is typing input
    Input,
    /// Waiting for generation (showing spinner)
    Generating,
    /// Streaming SSE response
    Streaming,
    /// Error state, can retry
    Error,
}

/// Convert a slice of conversation events to Claude API message format.
///
/// This is the canonical event-to-message conversion, used by the context window
/// builder to convert turn slices independently. The logic handles combining
/// adjacent Text + ToolCall events into single assistant messages with mixed
/// content blocks.
pub(crate) fn events_to_messages(events: &[ConversationEvent]) -> Vec<serde_json::Value> {
    let mut messages = Vec::new();
    let mut i = 0;

    while i < events.len() {
        match &events[i] {
            ConversationEvent::UserMessage { content } => {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": content
                }));
                i += 1;
            }
            ConversationEvent::Text { content } if content.is_empty() => {
                i += 1;
            }
            ConversationEvent::Text { content } => {
                let next_is_tool_call = events
                    .get(i + 1)
                    .is_some_and(|e| matches!(e, ConversationEvent::ToolCall { .. }));

                if next_is_tool_call {
                    let mut content_blocks = Vec::new();

                    if !content.is_empty() {
                        content_blocks.push(serde_json::json!({
                            "type": "text",
                            "text": content
                        }));
                    }

                    while let Some(ConversationEvent::ToolCall {
                        id, name, input, ..
                    }) = events.get(i + 1)
                    {
                        content_blocks.push(serde_json::json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": input
                        }));
                        i += 1;
                    }

                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content_blocks
                    }));
                    i += 1;
                } else {
                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content
                    }));
                    i += 1;
                }
            }
            ConversationEvent::ToolCall { .. } => {
                let mut tool_uses = Vec::new();
                while i < events.len() {
                    if let ConversationEvent::ToolCall {
                        id, name, input, ..
                    } = &events[i]
                    {
                        tool_uses.push(serde_json::json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": input
                        }));
                        i += 1;
                    } else {
                        break;
                    }
                }
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": tool_uses
                }));
            }
            ConversationEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
                remote,
                content_length,
            } => {
                let tool_result = if *remote {
                    let mut obj = serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "remote": true,
                        "is_error": is_error
                    });
                    if let Some(len) = content_length {
                        obj["content_length"] = serde_json::json!(len);
                    }
                    obj
                } else {
                    serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": content,
                        "is_error": is_error
                    })
                };
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": [tool_result]
                }));
                i += 1;
            }
            ConversationEvent::OutOfBandOutput { .. } => {
                i += 1;
            }
            ConversationEvent::SystemContext { content } => {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": content
                }));
                i += 1;
            }
        }
    }

    messages
}
