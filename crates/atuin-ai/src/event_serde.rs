//! Manual serialization for ConversationEvent to/from storage format.
//!
//! The storage format is decoupled from the Rust enum so the two can evolve
//! independently. Each event is stored as an `(event_type, event_data)` pair
//! where `event_data` is a JSON string.

use eyre::{Result, eyre};
use serde_json::Value;

use crate::tui::ConversationEvent;

/// Serialize a ConversationEvent into an (event_type, event_data_json) pair
/// suitable for database storage.
pub(crate) fn serialize_event(event: &ConversationEvent) -> (String, String) {
    match event {
        ConversationEvent::UserMessage { content } => (
            "user_message".to_string(),
            serde_json::json!({ "content": content }).to_string(),
        ),
        ConversationEvent::Text { content } => (
            "text".to_string(),
            serde_json::json!({ "content": content }).to_string(),
        ),
        ConversationEvent::ToolCall { id, name, input } => (
            "tool_call".to_string(),
            serde_json::json!({
                "id": id,
                "name": name,
                "input": input,
            })
            .to_string(),
        ),
        ConversationEvent::ToolResult {
            tool_use_id,
            content,
            is_error,
            remote,
            content_length,
        } => (
            "tool_result".to_string(),
            serde_json::json!({
                "tool_use_id": tool_use_id,
                "content": content,
                "is_error": is_error,
                "remote": remote,
                "content_length": content_length,
            })
            .to_string(),
        ),
        ConversationEvent::OutOfBandOutput {
            name,
            command,
            content,
        } => (
            "out_of_band_output".to_string(),
            serde_json::json!({
                "name": name,
                "command": command,
                "content": content,
            })
            .to_string(),
        ),
        ConversationEvent::SystemContext { content } => (
            "system_context".to_string(),
            serde_json::json!({ "content": content }).to_string(),
        ),
    }
}

/// Deserialize an (event_type, event_data_json) pair from storage back into a
/// ConversationEvent.
pub(crate) fn deserialize_event(event_type: &str, event_data: &str) -> Result<ConversationEvent> {
    let data: Value = serde_json::from_str(event_data)
        .map_err(|e| eyre!("failed to parse event_data JSON: {e}"))?;

    match event_type {
        "user_message" => Ok(ConversationEvent::UserMessage {
            content: json_string(&data, "content")?,
        }),
        "text" => Ok(ConversationEvent::Text {
            content: json_string(&data, "content")?,
        }),
        "tool_call" => Ok(ConversationEvent::ToolCall {
            id: json_string(&data, "id")?,
            name: json_string(&data, "name")?,
            input: data
                .get("input")
                .cloned()
                .ok_or_else(|| eyre!("tool_call missing 'input' field"))?,
        }),
        "tool_result" => Ok(ConversationEvent::ToolResult {
            tool_use_id: json_string(&data, "tool_use_id")?,
            content: json_string(&data, "content")?,
            is_error: data
                .get("is_error")
                .and_then(Value::as_bool)
                .ok_or_else(|| eyre!("tool_result missing 'is_error' field"))?,
            remote: data.get("remote").and_then(Value::as_bool).unwrap_or(false),
            content_length: data
                .get("content_length")
                .and_then(Value::as_u64)
                .map(|v| v as usize),
        }),
        "out_of_band_output" => Ok(ConversationEvent::OutOfBandOutput {
            name: json_string(&data, "name")?,
            command: data
                .get("command")
                .and_then(|v| if v.is_null() { None } else { v.as_str() })
                .map(String::from),
            content: json_string(&data, "content")?,
        }),
        "system_context" => Ok(ConversationEvent::SystemContext {
            content: json_string(&data, "content")?,
        }),
        other => Err(eyre!("unknown event type: {other}")),
    }
}

fn json_string(data: &Value, field: &str) -> Result<String> {
    data.get(field)
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| eyre!("missing or non-string field '{field}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(event: &ConversationEvent) -> ConversationEvent {
        let (event_type, event_data) = serialize_event(event);
        deserialize_event(&event_type, &event_data).unwrap()
    }

    #[test]
    fn test_user_message() {
        let event = ConversationEvent::UserMessage {
            content: "hello world".to_string(),
        };
        let result = round_trip(&event);
        assert!(
            matches!(result, ConversationEvent::UserMessage { content } if content == "hello world")
        );
    }

    #[test]
    fn test_text() {
        let event = ConversationEvent::Text {
            content: "response text".to_string(),
        };
        let result = round_trip(&event);
        assert!(
            matches!(result, ConversationEvent::Text { content } if content == "response text")
        );
    }

    #[test]
    fn test_tool_call() {
        let input = serde_json::json!({"command": "ls -la", "danger": "low"});
        let event = ConversationEvent::ToolCall {
            id: "tc_123".to_string(),
            name: "suggest_command".to_string(),
            input: input.clone(),
        };
        let result = round_trip(&event);
        match result {
            ConversationEvent::ToolCall {
                id,
                name,
                input: result_input,
            } => {
                assert_eq!(id, "tc_123");
                assert_eq!(name, "suggest_command");
                assert_eq!(result_input, input);
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn test_tool_result() {
        let event = ConversationEvent::ToolResult {
            tool_use_id: "tc_123".to_string(),
            content: "file contents here".to_string(),
            is_error: false,
            remote: false,
            content_length: None,
        };
        let result = round_trip(&event);
        match result {
            ConversationEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
                remote,
                content_length,
            } => {
                assert_eq!(tool_use_id, "tc_123");
                assert_eq!(content, "file contents here");
                assert!(!is_error);
                assert!(!remote);
                assert!(content_length.is_none());
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_tool_result_error() {
        let event = ConversationEvent::ToolResult {
            tool_use_id: "tc_456".to_string(),
            content: "permission denied".to_string(),
            is_error: true,
            remote: false,
            content_length: None,
        };
        let result = round_trip(&event);
        match result {
            ConversationEvent::ToolResult { is_error, .. } => assert!(is_error),
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_tool_result_remote() {
        let event = ConversationEvent::ToolResult {
            tool_use_id: "tc_789".to_string(),
            content: "ref:abc123".to_string(),
            is_error: false,
            remote: true,
            content_length: Some(4096),
        };
        let result = round_trip(&event);
        match result {
            ConversationEvent::ToolResult {
                remote,
                content_length,
                ..
            } => {
                assert!(remote);
                assert_eq!(content_length, Some(4096));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_tool_result_backwards_compat() {
        // Old stored data without remote/content_length fields should deserialize
        // with defaults (remote=false, content_length=None)
        let event = deserialize_event(
            "tool_result",
            r#"{"tool_use_id":"tc_old","content":"old result","is_error":false}"#,
        )
        .unwrap();
        match event {
            ConversationEvent::ToolResult {
                remote,
                content_length,
                ..
            } => {
                assert!(!remote);
                assert!(content_length.is_none());
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_out_of_band_with_command() {
        let event = ConversationEvent::OutOfBandOutput {
            name: "System".to_string(),
            command: Some("/help".to_string()),
            content: "help text".to_string(),
        };
        let result = round_trip(&event);
        match result {
            ConversationEvent::OutOfBandOutput {
                name,
                command,
                content,
            } => {
                assert_eq!(name, "System");
                assert_eq!(command.as_deref(), Some("/help"));
                assert_eq!(content, "help text");
            }
            _ => panic!("expected OutOfBandOutput"),
        }
    }

    #[test]
    fn test_out_of_band_without_command() {
        let event = ConversationEvent::OutOfBandOutput {
            name: "System".to_string(),
            command: None,
            content: "some output".to_string(),
        };
        let result = round_trip(&event);
        match result {
            ConversationEvent::OutOfBandOutput { command, .. } => {
                assert!(command.is_none());
            }
            _ => panic!("expected OutOfBandOutput"),
        }
    }

    #[test]
    fn test_unknown_event_type() {
        let result = deserialize_event("banana", "{}");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unknown event type")
        );
    }

    #[test]
    fn test_invalid_json() {
        let result = deserialize_event("text", "not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_field() {
        let result = deserialize_event("text", "{}");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("content"));
    }

    #[test]
    fn test_text_with_special_characters() {
        let event = ConversationEvent::Text {
            content: "line1\nline2\ttab \"quotes\" \\backslash 🎉".to_string(),
        };
        let result = round_trip(&event);
        assert!(
            matches!(result, ConversationEvent::Text { content } if content == "line1\nline2\ttab \"quotes\" \\backslash 🎉")
        );
    }

    #[test]
    fn test_tool_call_with_nested_input() {
        let input = serde_json::json!({
            "command": "echo 'hello'",
            "nested": { "a": [1, 2, 3], "b": null }
        });
        let event = ConversationEvent::ToolCall {
            id: "tc_1".to_string(),
            name: "execute_shell_command".to_string(),
            input: input.clone(),
        };
        let result = round_trip(&event);
        match result {
            ConversationEvent::ToolCall {
                input: result_input,
                ..
            } => {
                assert_eq!(result_input, input);
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn test_system_context() {
        let event = ConversationEvent::SystemContext {
            content: "[system: new invocation started]".to_string(),
        };
        let result = round_trip(&event);
        assert!(
            matches!(result, ConversationEvent::SystemContext { content } if content == "[system: new invocation started]")
        );
    }
}
