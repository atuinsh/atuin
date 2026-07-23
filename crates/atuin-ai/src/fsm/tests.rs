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
fn atuin_output_call_emits_command_lookup_and_stores_result() {
    let mut fsm = AgentFsm::new(
        vec!["client_v1_atuin_output".to_string()],
        "test-inv".to_string(),
    );
    fsm.handle(Event::UserSubmit("show output".into()));
    fsm.handle(Event::StreamStarted);

    let effects = fsm.handle(Event::StreamToolCall {
        id: "t1".into(),
        name: "atuin_output".into(),
        input: json!({"history_id": "018f011c-9a0a-7000-8000-000000000001"}),
    });

    // The command lookup runs alongside the permission check.
    assert!(matches!(
        effects[0],
        Effect::ResolveOutputCommand { ref tool_id, .. } if tool_id == "t1"
    ));
    assert!(matches!(effects[1], Effect::CheckPermission { .. }));

    let effects = fsm.handle(Event::OutputCommandResolved {
        tool_id: "t1".into(),
        command: Some("cargo test".into()),
    });

    assert!(effects.is_empty());
    let crate::tools::ClientToolCall::AtuinOutput(call) = &fsm.ctx.tools.get("t1").unwrap().tool
    else {
        panic!("expected AtuinOutput tool");
    };
    assert_eq!(call.command.as_deref(), Some("cargo test"));
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

// ============================================================================
// Shell execution timeouts
// ============================================================================

fn fsm_with_shell() -> AgentFsm {
    AgentFsm::new(
        vec![
            "client_v1_read_file".to_string(),
            "client_v1_execute_shell_command".to_string(),
        ],
        "test-inv".to_string(),
    )
}

fn shell_tool_call_event(id: &str) -> Event {
    Event::StreamToolCall {
        id: id.into(),
        name: "execute_shell_command".into(),
        input: json!({
            "command": "sleep 999",
            "shell": "bash",
            "timeout": 60,
            "description": "test"
        }),
    }
}

#[test]
fn shell_tool_schedules_execution_timeout() {
    let mut fsm = fsm_with_shell();
    fsm.handle(Event::UserSubmit("run something".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(shell_tool_call_event("t1"));

    let effects = fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Allowed,
    });

    // Should have ExecuteTool + ScheduleTimeout
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::ExecuteTool { .. }))
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::ScheduleTimeout { kind: effects::TimeoutKind::ToolExecution { tool_id }, .. }
            if tool_id == "t1"
    )));
    assert!(!fsm.ctx.tool_timeout_ids.is_empty());
}

#[test]
fn read_tool_does_not_schedule_timeout() {
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

    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::ExecuteTool { .. }))
    );
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::ScheduleTimeout { .. }))
    );
    assert!(fsm.ctx.tool_timeout_ids.is_empty());
}

#[test]
fn tool_completion_clears_timeout_mapping() {
    let mut fsm = fsm_with_shell();
    fsm.handle(Event::UserSubmit("run".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(shell_tool_call_event("t1"));
    fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Allowed,
    });
    fsm.handle(Event::StreamDone {
        session_id: "s1".into(),
    });

    assert!(!fsm.ctx.tool_timeout_ids.is_empty());

    // Tool completes naturally
    fsm.handle(Event::ToolExecutionDone {
        tool_id: "t1".into(),
        outcome: crate::tools::ToolOutcome::Success("done".into()),
        preview: None,
    });

    assert!(fsm.ctx.tool_timeout_ids.is_empty());
}

#[test]
fn stale_timeout_after_natural_completion_is_ignored() {
    let mut fsm = fsm_with_shell();
    fsm.handle(Event::UserSubmit("run".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(shell_tool_call_event("t1"));
    fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Allowed,
    });
    fsm.handle(Event::StreamDone {
        session_id: "s1".into(),
    });

    // Tool completes naturally
    fsm.handle(Event::ToolExecutionDone {
        tool_id: "t1".into(),
        outcome: crate::tools::ToolOutcome::Success("done".into()),
        preview: None,
    });

    // Stale timeout fires — should be no-op
    let effects = fsm.handle(Event::ToolExecutionTimeout {
        timeout_id: 0,
        tool_id: "t1".into(),
    });

    assert!(effects.is_empty());
}

#[test]
fn timeout_fires_before_completion_emits_abort() {
    let mut fsm = fsm_with_shell();
    fsm.handle(Event::UserSubmit("run".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(shell_tool_call_event("t1"));
    fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Allowed,
    });
    fsm.handle(Event::StreamDone {
        session_id: "s1".into(),
    });

    // Timeout fires while tool is still executing
    let effects = fsm.handle(Event::ToolExecutionTimeout {
        timeout_id: 0,
        tool_id: "t1".into(),
    });

    assert_eq!(effects.len(), 1);
    assert!(matches!(
        effects[0],
        Effect::AbortTool { ref tool_id } if tool_id == "t1"
    ));
    // Timeout mapping cleaned up
    assert!(fsm.ctx.tool_timeout_ids.is_empty());
}

#[test]
fn timeout_respects_llm_specified_duration() {
    let mut fsm = fsm_with_shell();
    fsm.handle(Event::UserSubmit("run".into()));
    fsm.handle(Event::StreamStarted);

    // Tool call with timeout: 120
    fsm.handle(Event::StreamToolCall {
        id: "t1".into(),
        name: "execute_shell_command".into(),
        input: json!({
            "command": "cargo build",
            "shell": "bash",
            "timeout": 120,
            "description": "build"
        }),
    });

    let effects = fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Allowed,
    });

    let timeout_effect = effects
        .iter()
        .find(|e| matches!(e, Effect::ScheduleTimeout { .. }));
    assert!(matches!(
        timeout_effect,
        Some(Effect::ScheduleTimeout { duration, .. }) if *duration == std::time::Duration::from_secs(120)
    ));
}

#[test]
fn cancel_clears_timeout_mappings() {
    let mut fsm = fsm_with_shell();
    fsm.handle(Event::UserSubmit("run".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(shell_tool_call_event("t1"));
    fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Allowed,
    });

    assert!(!fsm.ctx.tool_timeout_ids.is_empty());

    fsm.handle(Event::Cancel);

    assert!(fsm.ctx.tool_timeout_ids.is_empty());
}

#[test]
fn timeout_abort_propagates_timeout_reason_to_preview_and_llm() {
    use super::tools::InterruptReason;

    let mut fsm = fsm_with_shell();
    fsm.handle(Event::UserSubmit("run".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(shell_tool_call_event("t1"));
    fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Allowed,
    });
    fsm.handle(Event::StreamDone {
        session_id: "s1".into(),
    });

    // Timeout fires
    fsm.handle(Event::ToolExecutionTimeout {
        timeout_id: 0,
        tool_id: "t1".into(),
    });

    // Tool completes after abort (interrupted: true from execute_shell_command_streaming)
    fsm.handle(Event::ToolExecutionDone {
        tool_id: "t1".into(),
        outcome: crate::tools::ToolOutcome::Structured {
            stdout: "partial output".into(),
            stderr: String::new(),
            exit_code: None,
            duration_ms: 60000,
            interrupted: true,
        },
        preview: Some(super::tools::ToolPreviewData::Shell {
            lines: vec!["partial output".into()],
            exit_code: None,
            interrupted: None, // FSM overrides this with the reason
        }),
    });

    // Preview should carry Timeout reason
    let tracked = fsm.ctx.tools.get("t1").unwrap();
    let preview = tracked.shell_preview().unwrap();
    assert_eq!(preview.interrupted, Some(InterruptReason::Timeout(60)));

    // LLM content should say "Timed out" not "Interrupted by user"
    let tool_result = fsm.ctx.events.iter().find(
        |e| matches!(e, ConversationEvent::ToolResult { tool_use_id, .. } if tool_use_id == "t1"),
    );
    if let Some(ConversationEvent::ToolResult { content, .. }) = tool_result {
        assert!(
            content.contains("[Timed out after 60s]"),
            "Expected timeout message, got: {content}"
        );
        assert!(!content.contains("[Interrupted by user]"));
    } else {
        panic!("No ToolResult found for t1");
    }
}

#[test]
fn user_interrupt_propagates_user_reason_to_preview_and_llm() {
    use super::tools::InterruptReason;

    let mut fsm = fsm_with_shell();
    fsm.handle(Event::UserSubmit("run".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(shell_tool_call_event("t1"));
    fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Allowed,
    });
    fsm.handle(Event::StreamDone {
        session_id: "s1".into(),
    });

    // User interrupts
    fsm.handle(Event::InterruptTools);

    // Tool completes after abort
    fsm.handle(Event::ToolExecutionDone {
        tool_id: "t1".into(),
        outcome: crate::tools::ToolOutcome::Structured {
            stdout: "partial".into(),
            stderr: String::new(),
            exit_code: None,
            duration_ms: 5000,
            interrupted: true,
        },
        preview: Some(super::tools::ToolPreviewData::Shell {
            lines: vec!["partial".into()],
            exit_code: None,
            interrupted: None, // FSM overrides this with the reason
        }),
    });

    // Preview should carry User reason
    let tracked = fsm.ctx.tools.get("t1").unwrap();
    let preview = tracked.shell_preview().unwrap();
    assert_eq!(preview.interrupted, Some(InterruptReason::User));

    // LLM content should say "Interrupted by user"
    let tool_result = fsm.ctx.events.iter().find(
        |e| matches!(e, ConversationEvent::ToolResult { tool_use_id, .. } if tool_use_id == "t1"),
    );
    if let Some(ConversationEvent::ToolResult { content, .. }) = tool_result {
        assert!(
            content.contains("[Interrupted by user]"),
            "Expected user interrupt message, got: {content}"
        );
    } else {
        panic!("No ToolResult found for t1");
    }
}

#[test]
fn user_interrupt_clears_timeout_mappings_for_aborted_tools() {
    let mut fsm = fsm_with_shell();
    fsm.handle(Event::UserSubmit("run".into()));
    fsm.handle(Event::StreamStarted);
    fsm.handle(shell_tool_call_event("t1"));
    fsm.handle(Event::PermissionResolved {
        tool_id: "t1".into(),
        response: PermissionResponse::Allowed,
    });

    assert!(!fsm.ctx.tool_timeout_ids.is_empty());

    fsm.handle(Event::InterruptTools);

    assert!(fsm.ctx.tool_timeout_ids.is_empty());
}

// ============================================================================
// Model picker
// ============================================================================

fn model_list() -> crate::models::ModelList {
    crate::models::ModelList {
        default: "fast".to_string(),
        models: vec![
            crate::models::ModelInfo {
                alias: "fast".to_string(),
                name: "Comet".to_string(),
                description: "Fastest model".to_string(),
            },
            crate::models::ModelInfo {
                alias: "deep".to_string(),
                name: "Constellation".to_string(),
                description: "Deeper reasoning".to_string(),
            },
        ],
    }
}

#[test]
fn open_model_picker_fetches_when_uncached() {
    let mut fsm = new_fsm();

    let effects = fsm.handle(Event::OpenModelPicker);

    assert_eq!(fsm.ctx.model_picker, Some(ModelPicker::Loading));
    assert!(matches!(effects[..], [Effect::FetchModels]));
}

#[test]
fn open_model_picker_reuses_cache_without_fetching() {
    let mut fsm = new_fsm();
    fsm.ctx.models_cache = Some(model_list());

    let effects = fsm.handle(Event::OpenModelPicker);

    assert_eq!(fsm.ctx.model_picker, Some(ModelPicker::Ready(model_list())));
    assert!(effects.is_empty());
}

#[test]
fn model_list_loaded_populates_picker_and_cache() {
    let mut fsm = new_fsm();
    fsm.handle(Event::OpenModelPicker);

    let effects = fsm.handle(Event::ModelListLoaded(Ok(model_list())));

    assert_eq!(fsm.ctx.models_cache, Some(model_list()));
    assert_eq!(fsm.ctx.model_picker, Some(ModelPicker::Ready(model_list())));
    assert!(effects.is_empty());
}

#[test]
fn model_list_loaded_after_dismissal_caches_but_keeps_picker_closed() {
    let mut fsm = new_fsm();
    fsm.handle(Event::OpenModelPicker);
    fsm.handle(Event::Cancel); // dismiss while loading

    fsm.handle(Event::ModelListLoaded(Ok(model_list())));

    assert_eq!(fsm.ctx.models_cache, Some(model_list()));
    assert_eq!(fsm.ctx.model_picker, None);
}

#[test]
fn model_list_load_failure_closes_picker_with_message() {
    let mut fsm = new_fsm();
    fsm.handle(Event::OpenModelPicker);

    fsm.handle(Event::ModelListLoaded(Err("boom".to_string())));

    assert_eq!(fsm.ctx.model_picker, None);
    assert!(fsm.ctx.models_cache.is_none());
    assert!(fsm.ctx.events.iter().any(|e| matches!(
        e,
        ConversationEvent::OutOfBandOutput { content, .. } if content.contains("boom")
    )));
}

#[test]
fn model_selected_sets_model_and_persists() {
    let mut fsm = new_fsm();
    fsm.handle(Event::OpenModelPicker);
    fsm.handle(Event::ModelListLoaded(Ok(model_list())));

    let effects = fsm.handle(Event::ModelSelected("deep".to_string()));

    assert_eq!(fsm.ctx.model, Some("deep".to_string()));
    assert_eq!(fsm.ctx.model_picker, None);
    assert!(matches!(
        &effects[..],
        [Effect::SaveModelSelection { alias }] if alias == "deep"
    ));
    // Confirmation names the model, not just the alias
    assert!(fsm.ctx.events.iter().any(|e| matches!(
        e,
        ConversationEvent::OutOfBandOutput { content, .. } if content.contains("Constellation")
    )));
}

#[test]
fn cancel_closes_picker_instead_of_exiting() {
    let mut fsm = new_fsm();
    fsm.handle(Event::OpenModelPicker);

    let effects = fsm.handle(Event::Cancel);

    assert_eq!(fsm.ctx.model_picker, None);
    assert!(effects.is_empty());

    // A second Cancel with no picker open exits as usual
    let effects = fsm.handle(Event::Cancel);
    assert!(matches!(effects[..], [Effect::ExitApp(ExitAction::Cancel)]));
}

#[test]
fn user_submit_dismisses_loading_picker() {
    let mut fsm = new_fsm();
    fsm.handle(Event::OpenModelPicker);

    fsm.handle(Event::UserSubmit("hello".into()));

    assert_eq!(fsm.ctx.model_picker, None);
}

#[test]
fn selected_model_survives_new_session() {
    let mut fsm = new_fsm();
    fsm.ctx.models_cache = Some(model_list());
    fsm.handle(Event::OpenModelPicker);
    fsm.handle(Event::ModelSelected("deep".to_string()));

    fsm.handle(Event::NewSession);

    assert_eq!(fsm.ctx.model, Some("deep".to_string()));
    assert_eq!(fsm.ctx.models_cache, Some(model_list()));
}
