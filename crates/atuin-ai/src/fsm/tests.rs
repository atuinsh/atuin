//! Pure FSM transition tests. No IO, no async.

use serde_json::json;

use super::*;
use effects::{Effect, ExitAction};
use events::{Event, PermissionChoice, PermissionResponse};

fn new_fsm() -> AgentFsm {
    AgentFsm::new(
        vec!["client_v1_read_file".to_string()],
        "test-inv".to_string(),
    )
}

// ============================================================================
// Idle → Turn
// ============================================================================

#[test]
fn user_submit_starts_turn() {
    let mut fsm = new_fsm();

    let effects = fsm.handle(Event::UserSubmit("hello".into()));

    assert!(matches!(
        fsm.state,
        AgentState::Turn {
            stream: StreamPhase::Connecting
        }
    ));
    assert_eq!(effects.len(), 1);
    assert!(matches!(effects[0], Effect::StartStream { .. }));
    // User message was pushed to events
    assert!(fsm.ctx.events.iter().any(|e| matches!(
        e,
        ConversationEvent::UserMessage { content } if content == "hello"
    )));
}

#[test]
fn stream_started_transitions_to_streaming() {
    let mut fsm = new_fsm();
    fsm.handle(Event::UserSubmit("hello".into()));

    let effects = fsm.handle(Event::StreamStarted);

    assert!(matches!(
        fsm.state,
        AgentState::Turn {
            stream: StreamPhase::Streaming { status: None }
        }
    ));
    assert!(effects.is_empty());
}

#[test]
fn stream_chunk_accumulates_text() {
    let mut fsm = new_fsm();
    fsm.handle(Event::UserSubmit("hello".into()));
    fsm.handle(Event::StreamStarted);

    fsm.handle(Event::StreamChunk("Hello ".into()));
    fsm.handle(Event::StreamChunk("world!".into()));

    assert_eq!(fsm.ctx.current_response, "Hello world!");
}

#[test]
fn stream_done_without_tools_goes_idle() {
    let mut fsm = new_fsm();
    fsm.handle(Event::UserSubmit("hello".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(Event::StreamChunk("Hi there!".into()));

    let effects = fsm.handle(Event::StreamDone {
        session_id: "s1".into(),
    });

    assert_eq!(fsm.state, AgentState::Idle { confirmation: None });
    assert_eq!(fsm.ctx.session_id, Some("s1".to_string()));
    assert!(effects.iter().any(|e| matches!(e, Effect::Persist)));
    // Text was committed to events
    assert!(fsm.ctx.events.iter().any(|e| matches!(
        e,
        ConversationEvent::Text { content } if content == "Hi there!"
    )));
}

// ============================================================================
// Tool lifecycle
// ============================================================================

#[test]
fn stream_tool_call_tracks_tool_and_emits_check_permission() {
    let mut fsm = new_fsm();
    fsm.handle(Event::UserSubmit("read a file".into()));
    fsm.handle(Event::StreamStarted);

    let effects = fsm.handle(Event::StreamToolCall {
        id: "t1".into(),
        name: "read_file".into(),
        input: json!({"file_path": "/tmp/test.txt"}),
    });

    assert!(fsm.ctx.tools.get("t1").is_some());
    assert_eq!(effects.len(), 1);
    assert!(matches!(effects[0], Effect::CheckPermission { .. }));
}

#[test]
fn permission_allowed_transitions_to_executing() {
    let mut fsm = new_fsm();
    fsm.handle(Event::UserSubmit("read".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(Event::StreamToolCall {
        id: "t1".into(),
        name: "read_file".into(),
        input: json!({"file_path": "/tmp/test.txt"}),
    });

    let effects = fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Allowed,
    });

    assert_eq!(fsm.ctx.tools.get("t1").unwrap().state, ToolState::Executing);
    assert!(matches!(effects[0], Effect::ExecuteTool { .. }));
}

#[test]
fn permission_ask_transitions_to_awaiting() {
    let mut fsm = new_fsm();
    fsm.handle(Event::UserSubmit("read".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(Event::StreamToolCall {
        id: "t1".into(),
        name: "read_file".into(),
        input: json!({"file_path": "/tmp/test.txt"}),
    });

    let effects = fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Ask,
    });

    assert_eq!(
        fsm.ctx.tools.get("t1").unwrap().state,
        ToolState::AwaitingPermission
    );
    assert!(effects.is_empty());
}

#[test]
fn tool_done_after_stream_done_continues_conversation() {
    let mut fsm = new_fsm();
    fsm.handle(Event::UserSubmit("read".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(Event::StreamToolCall {
        id: "t1".into(),
        name: "read_file".into(),
        input: json!({"file_path": "/tmp/test.txt"}),
    });
    fsm.handle(Event::StreamDone {
        session_id: "".into(),
    });
    fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Allowed,
    });

    // Now in Turn { Done } with one tool Executing
    let effects = fsm.handle(Event::ToolExecutionDone {
        tool_id: "t1".into(),
        outcome: crate::tools::ToolOutcome::Success("file contents".into()),
        preview: None,
    });

    // Turn complete → continuation
    assert!(matches!(
        fsm.state,
        AgentState::Turn {
            stream: StreamPhase::Connecting
        }
    ));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::StartStream { .. }))
    );
}

#[test]
fn continuation_turn_without_new_tools_goes_idle() {
    let mut fsm = new_fsm();
    fsm.handle(Event::UserSubmit("read".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(Event::StreamToolCall {
        id: "t1".into(),
        name: "read_file".into(),
        input: json!({"file_path": "/tmp/test.txt"}),
    });
    fsm.handle(Event::StreamDone {
        session_id: "".into(),
    });
    fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Allowed,
    });
    // Tool completes → continuation starts
    fsm.handle(Event::ToolExecutionDone {
        tool_id: "t1".into(),
        outcome: crate::tools::ToolOutcome::Success("contents".into()),
        preview: None,
    });
    assert!(matches!(
        fsm.state,
        AgentState::Turn {
            stream: StreamPhase::Connecting
        }
    ));

    // Continuation stream: text only, no new tools
    fsm.handle(Event::StreamStarted);
    fsm.handle(Event::StreamChunk("Here's the file.".into()));
    let effects = fsm.handle(Event::StreamDone {
        session_id: "".into(),
    });

    // Should go Idle, NOT start another continuation
    assert_eq!(fsm.state, AgentState::Idle { confirmation: None });
    assert!(effects.iter().any(|e| matches!(e, Effect::Persist)));
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::StartStream { .. }))
    );
}

#[test]
fn tool_done_before_stream_done_stays_in_turn() {
    let mut fsm = new_fsm();
    fsm.handle(Event::UserSubmit("read".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(Event::StreamToolCall {
        id: "t1".into(),
        name: "read_file".into(),
        input: json!({"file_path": "/tmp/test.txt"}),
    });
    fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Allowed,
    });

    // Tool completes but stream hasn't sent Done yet
    let effects = fsm.handle(Event::ToolExecutionDone {
        tool_id: "t1".into(),
        outcome: crate::tools::ToolOutcome::Success("contents".into()),
        preview: None,
    });

    // Still in Turn — stream phase is Streaming, not Done
    assert!(matches!(
        fsm.state,
        AgentState::Turn {
            stream: StreamPhase::Streaming { .. }
        }
    ));
    assert!(effects.is_empty());
}

// ============================================================================
// Cancel
// ============================================================================

#[test]
fn cancel_during_streaming_goes_idle() {
    let mut fsm = new_fsm();
    fsm.handle(Event::UserSubmit("hello".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(Event::StreamChunk("partial text".into()));

    let effects = fsm.handle(Event::Cancel);

    assert_eq!(fsm.state, AgentState::Idle { confirmation: None });
    assert!(effects.iter().any(|e| matches!(e, Effect::AbortStream)));
    assert!(effects.iter().any(|e| matches!(e, Effect::Persist)));
    // Partial text committed with cancel suffix
    assert!(fsm.ctx.events.iter().any(|e| matches!(
        e,
        ConversationEvent::Text { content } if content.contains("[User cancelled")
    )));
}

#[test]
fn stale_permission_resolved_after_cancel_is_ignored() {
    let mut fsm = new_fsm();
    fsm.handle(Event::UserSubmit("read".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(Event::StreamToolCall {
        id: "t1".into(),
        name: "read_file".into(),
        input: json!({"file_path": "/tmp/test.txt"}),
    });
    fsm.handle(Event::StreamDone {
        session_id: "".into(),
    });
    // Tool is in CheckingPermission, cancel happens before permission resolves
    fsm.handle(Event::Cancel);
    assert_eq!(fsm.state, AgentState::Idle { confirmation: None });

    // Stale permission result arrives — tool is already Completed (cancelled)
    let effects = fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Allowed,
    });

    // Should NOT emit ExecuteTool — the tool was cancelled
    assert!(effects.is_empty());
}

#[test]
fn cancel_during_turn_with_pending_tools() {
    let mut fsm = new_fsm();
    fsm.handle(Event::UserSubmit("hello".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(Event::StreamToolCall {
        id: "t1".into(),
        name: "read_file".into(),
        input: json!({"file_path": "/tmp/test.txt"}),
    });
    fsm.handle(Event::StreamDone {
        session_id: "".into(),
    });
    fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Allowed,
    });
    // Tool is Executing, stream is Done

    let effects = fsm.handle(Event::Cancel);

    assert_eq!(fsm.state, AgentState::Idle { confirmation: None });
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::AbortTool { .. }))
    );
    // Error ToolResult injected
    assert!(fsm.ctx.events.iter().any(|e| matches!(
        e,
        ConversationEvent::ToolResult { tool_use_id, is_error: true, .. } if tool_use_id == "t1"
    )));
    // SystemContext about cancellation
    assert!(fsm.ctx.events.iter().any(|e| matches!(
        e,
        ConversationEvent::SystemContext { content } if content.contains("cancelled")
    )));
}

#[test]
fn stale_tool_result_after_cancel_is_ignored() {
    let mut fsm = new_fsm();
    fsm.handle(Event::UserSubmit("hello".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(Event::StreamToolCall {
        id: "t1".into(),
        name: "read_file".into(),
        input: json!({"file_path": "/tmp/test.txt"}),
    });
    fsm.handle(Event::StreamDone {
        session_id: "".into(),
    });
    fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Allowed,
    });
    fsm.handle(Event::Cancel);

    // Stale event arrives
    let effects = fsm.handle(Event::ToolExecutionDone {
        tool_id: "t1".into(),
        outcome: crate::tools::ToolOutcome::Success("contents".into()),
        preview: None,
    });

    assert_eq!(fsm.state, AgentState::Idle { confirmation: None });
    assert!(effects.is_empty());
}

// ============================================================================
// Confirmation
// ============================================================================

#[test]
fn dangerous_command_enters_confirmation() {
    let mut fsm = new_fsm();
    // Simulate a dangerous command in history
    fsm.ctx.events.push(ConversationEvent::ToolCall {
        id: "sc1".into(),
        name: "suggest_command".into(),
        input: json!({"command": "rm -rf /", "description": "bad", "confidence": "high", "danger": "high"}),
    });

    let effects = fsm.handle(Event::ExecuteCommand);

    assert!(matches!(
        fsm.state,
        AgentState::Idle {
            confirmation: Some(_)
        }
    ));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::ScheduleTimeout { .. }))
    );
}

#[test]
fn second_execute_confirms_and_exits() {
    let mut fsm = new_fsm();
    fsm.ctx.events.push(ConversationEvent::ToolCall {
        id: "sc1".into(),
        name: "suggest_command".into(),
        input: json!({"command": "rm -rf /", "description": "bad", "confidence": "high", "danger": "high"}),
    });
    fsm.handle(Event::ExecuteCommand);

    let effects = fsm.handle(Event::ExecuteCommand);

    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::ExitApp(ExitAction::Execute(cmd)) if cmd == "rm -rf /"
    )));
}

#[test]
fn confirmation_timeout_clears_confirmation() {
    let mut fsm = new_fsm();
    fsm.ctx.events.push(ConversationEvent::ToolCall {
        id: "sc1".into(),
        name: "suggest_command".into(),
        input: json!({"command": "rm -rf /", "description": "bad", "confidence": "high", "danger": "high"}),
    });
    fsm.handle(Event::ExecuteCommand);
    let timeout_id = match &fsm.state {
        AgentState::Idle {
            confirmation: Some(c),
        } => c.timeout_id,
        _ => panic!("expected confirmation"),
    };

    fsm.handle(Event::ConfirmationTimeout { timeout_id });

    assert_eq!(fsm.state, AgentState::Idle { confirmation: None });
}

// ============================================================================
// Error / Retry
// ============================================================================

#[test]
fn stream_error_goes_to_error_state() {
    let mut fsm = new_fsm();
    fsm.handle(Event::UserSubmit("hello".into()));
    fsm.handle(Event::StreamStarted);

    fsm.handle(Event::StreamError("network error".into()));

    assert_eq!(fsm.state, AgentState::Error("network error".to_string()));
}

#[test]
fn retry_from_error_starts_new_stream() {
    let mut fsm = new_fsm();
    fsm.handle(Event::UserSubmit("hello".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(Event::StreamError("fail".into()));

    let effects = fsm.handle(Event::Retry);

    assert!(matches!(
        fsm.state,
        AgentState::Turn {
            stream: StreamPhase::Connecting
        }
    ));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::StartStream { .. }))
    );
}

// ============================================================================
// Permission choices
// ============================================================================

#[test]
fn permission_deny_completes_turn_and_continues() {
    let mut fsm = new_fsm();
    fsm.handle(Event::UserSubmit("read".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(Event::StreamToolCall {
        id: "t1".into(),
        name: "read_file".into(),
        input: json!({"file_path": "/tmp/test.txt"}),
    });
    fsm.handle(Event::StreamDone {
        session_id: "".into(),
    });
    fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Ask,
    });

    let effects = fsm.handle(Event::PermissionUserChoice {
        tool_id: "t1".into(),
        choice: PermissionChoice::Deny,
    });

    // Turn should complete since all tools resolved and stream is done
    // → continuation needed (there was a tool result to send back)
    assert!(matches!(
        fsm.state,
        AgentState::Turn {
            stream: StreamPhase::Connecting
        }
    ));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::StartStream { .. }))
    );
    // Error result was injected
    assert!(fsm.ctx.events.iter().any(|e| matches!(
        e,
        ConversationEvent::ToolResult { tool_use_id, is_error: true, .. } if tool_use_id == "t1"
    )));
}
