//! The domain event the hook command acts on.
//!
//! When an agent sends a hook event through the [`WireHookEvent`] interface, this may or may not be
//! an event we care about. If we don't know how to deserialize it/don't care for it, we need to
//! drop that on the floor. The [`HookEvent`] type represents agent events we know/care about.

use atuin_common::string::NonNulStr;
use serde_json::error::Category;

use super::wire::{AgyEventName, AgyHookEvent, HookEventName, WireHookEvent, WireToolName};

/// Why a hook payload could not be parsed.
///
/// A payload that is well-formed JSON but not an event Atuin models is *not* an
/// error — it reduces to `Ok(None)`. Only input an agent could never send by
/// design surfaces here.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// The payload on stdin was not valid JSON — a syntax error or truncated
    /// input. Agents always emit syntactically valid JSON, so this signals a
    /// real fault rather than an event to skip.
    #[error("hook payload is not valid JSON at line {line}, column {column}")]
    MalformedJson {
        line: usize,
        column: usize,
        #[source]
        source: serde_json::Error,
    },
}

/// An agent hook event Atuin cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    /// A Bash command is about to run; open a history entry.
    Start {
        command: NonNulStr,
        intent: Option<String>,
        tool_use_id: String,
    },
    /// A Bash command finished; close the matching history entry.
    End {
        tool_use_id: String,
        exit: i64,
    },
}

impl From<WireHookEvent> for Option<HookEvent> {
    /// Reduce a decoded wire event to a [`HookEvent`], or `None` when we don't care about the given
    /// event.
    ///
    /// We **don't** care about:
    ///   - Non-`Bash` tool invocations.
    ///   - Tool invocations which are missing a `tool_use_id`.
    fn from(wire: WireHookEvent) -> Self {
        if matches!(wire.tool_name, WireToolName::Other) {
            return None;
        }

        // Present but empty is as good as missing: a start could never be
        // matched to its end.
        if wire.tool_use_id.is_empty() {
            return None;
        }
        let tool_use_id = wire.tool_use_id;

        match wire.hook_event_name {
            HookEventName::PreToolUse => {
                let (command, intent) = match wire.tool_input {
                    Some(input) => (input.command, input.description),
                    None => (None, None),
                };

                // A missing or empty command has nothing to record.
                let command = command.filter(|command| !command.is_empty())?;

                Some(HookEvent::Start {
                    command,
                    intent,
                    tool_use_id,
                })
            }
            HookEventName::PostToolUse => {
                let exit = wire.tool_response.and_then(|response| response.exit_code).unwrap_or(0);
                Some(HookEvent::End { tool_use_id, exit })
            }
            HookEventName::PostToolUseFailure => Some(HookEvent::End {
                tool_use_id,
                exit: 1,
            }),
            HookEventName::Other => None,
        }
    }
}

impl HookEvent {
    /// Parse a raw hook payload (the JSON an agent writes to stdin) into a [`HookEvent`], or `None`
    /// when there is nothing to record.
    ///
    /// Well-formed JSON that doesn't fit the hook-event schema yields `Ok(None)`.
    ///
    /// Only *malformed* JSON (a syntax error or truncated input) is surfaced as a
    /// [`ParseError`]: an agent could never send that legitimately, so it signals
    /// a real fault worth seeing.
    pub fn from_json_str(input: &str) -> Result<Option<Self>, ParseError> {
        match serde_json::from_str::<WireHookEvent>(input) {
            Ok(wire) => Ok(wire.into()),
            Err(err) if err.classify() == Category::Data => Ok(None),
            Err(err) => Err(ParseError::MalformedJson {
                line: err.line(),
                column: err.column(),
                source: err,
            }),
        }
    }

    /// Parse a raw Antigravity hook payload into a [`HookEvent`], or `None`
    /// when there is nothing to record.
    ///
    /// Returns whether the event was a `PreToolUse`, which the caller needs
    /// even when there is nothing to record: Antigravity requires every
    /// `PreToolUse` hook to answer `{"decision": "allow"}` on stdout (and
    /// `{}` for `PostToolUse`), including invocations for tools Atuin ignores.
    ///
    /// Antigravity sends no tool-use id, so a start is correlated with its end
    /// through `conversationId` plus `stepIdx`, and completion carries no
    /// numeric exit code — a non-empty `error` string means exit 1.
    pub fn from_agy_json_str(input: &str) -> Result<(bool, Option<Self>), ParseError> {
        let wire = match serde_json::from_str::<AgyHookEvent>(input) {
            Ok(wire) => wire,
            Err(err) if err.classify() == Category::Data => {
                return Ok((false, None));
            }
            Err(err) => {
                return Err(ParseError::MalformedJson {
                    line: err.line(),
                    column: err.column(),
                    source: err,
                });
            }
        };

        let is_pre = matches!(wire.event_name, AgyEventName::PreToolUse);
        Ok((is_pre, Self::from_agy(wire)))
    }

    /// Reduce a decoded Antigravity wire event to a [`HookEvent`], or `None`
    /// when we don't care about the given event.
    ///
    /// We **don't** care about:
    ///   - Non-`run_command` tool invocations.
    ///   - Tool invocations missing a command, a conversation id, or a step index:
    ///     a start could never be matched to its end without all three.
    fn from_agy(wire: AgyHookEvent) -> Option<Self> {
        let tool_use_id = match (wire.conversation_id, wire.step_idx) {
            (Some(conversation_id), Some(step_idx)) if !conversation_id.is_empty() => {
                format!("{conversation_id}-{step_idx}")
            }
            _ => return None,
        };

        match wire.event_name {
            AgyEventName::PreToolUse => {
                let command = wire
                    .tool_call
                    .as_ref()
                    .filter(|call| call.name.as_deref() == Some("run_command"))
                    .and_then(|call| call.args.as_ref())
                    .and_then(|args| args.command_line.clone())
                    .filter(|command| !command.is_empty())?;

                // Antigravity's pre-tool-use args carry no description.
                Some(HookEvent::Start {
                    command,
                    intent: None,
                    tool_use_id,
                })
            }
            AgyEventName::PostToolUse => {
                let failed = wire.error.as_deref().is_some_and(|error| !error.is_empty());
                Some(HookEvent::End {
                    tool_use_id,
                    exit: i64::from(failed),
                })
            }
            AgyEventName::Other => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;
    use serde_json::json;

    use super::*;

    fn non_nul(s: &str) -> NonNulStr {
        NonNulStr::new(s.to_owned()).unwrap()
    }

    #[rstest]
    #[case::pre_tool_use_with_intent(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "echo hello", "description": "Test greeting"},
            "tool_use_id": "toolu_abc123",
            "session_id": "sess1",
            "cwd": "/tmp"
        }),
        Some(HookEvent::Start {
            command: non_nul("echo hello"),
            intent: Some("Test greeting".into()),
            tool_use_id: "toolu_abc123".into(),
        })
    )]
    #[case::pre_tool_use_without_description(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "tool_use_id": "toolu_abc123"
        }),
        Some(HookEvent::Start { command: non_nul("ls"), intent: None, tool_use_id: "toolu_abc123".into() })
    )]
    #[case::post_tool_use_uses_exit_code(
        json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "echo hello"},
            "tool_response": {"exitCode": 3, "stdout": "hello\n"},
            "tool_use_id": "toolu_abc123"
        }),
        Some(HookEvent::End { tool_use_id: "toolu_abc123".into(), exit: 3 })
    )]
    #[case::post_tool_use_without_exit_code_defaults_zero(
        json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": {},
            "tool_use_id": "toolu_abc123"
        }),
        Some(HookEvent::End { tool_use_id: "toolu_abc123".into(), exit: 0 })
    )]
    // A null exitCode also defaults to 0.
    #[case::null_exit_code_defaults_zero(
        json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": {"exitCode": null},
            "tool_use_id": "toolu_abc123"
        }),
        Some(HookEvent::End { tool_use_id: "toolu_abc123".into(), exit: 0 })
    )]
    // PostToolUseFailure forces exit 1 and ignores tool_response entirely.
    #[case::failure_forces_exit_one_ignoring_response(
        json!({
            "hook_event_name": "PostToolUseFailure",
            "tool_name": "Bash",
            "tool_input": {"command": "false"},
            "tool_response": {"exitCode": 0},
            "tool_use_id": "toolu_abc123"
        }),
        Some(HookEvent::End { tool_use_id: "toolu_abc123".into(), exit: 1 })
    )]
    // Non-Bash tools are never recorded.
    #[case::non_bash_tool_skipped(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": "/tmp/test.txt", "content": "hello"},
            "tool_use_id": "toolu_abc123"
        }),
        None
    )]
    // A missing tool_use_id can't be correlated start↔end → skip.
    #[case::missing_tool_use_id_skipped(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "echo hi"}
        }),
        None
    )]
    // An empty tool_use_id is treated the same as missing.
    #[case::empty_tool_use_id_skipped(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "echo hi"},
            "tool_use_id": ""
        }),
        None
    )]
    // An empty command has nothing to record.
    #[case::empty_command_skipped(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": ""},
            "tool_use_id": "toolu_abc123"
        }),
        None
    )]
    // A command carrying a NUL fails to deserialize, so the whole event is
    // dropped rather than recording a mangled command (issue #3589).
    #[case::command_with_nul_rejected(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "echo hi\0rm -rf /"},
            "tool_use_id": "toolu_abc123"
        }),
        None
    )]
    // A command that is nothing but a NUL prefix is likewise rejected.
    #[case::command_only_nul_rejected(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "\0rm -rf /"},
            "tool_use_id": "toolu_abc123"
        }),
        None
    )]
    // No tool_input at all → empty command → skip.
    #[case::missing_tool_input_skipped(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_use_id": "toolu_abc123"
        }),
        None
    )]
    // A null tool_input decodes to None → skip.
    #[case::null_tool_input_skipped(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": null,
            "tool_use_id": "toolu_abc123"
        }),
        None
    )]
    // An event name we don't model is ignored.
    #[case::unknown_event_skipped(
        json!({
            "hook_event_name": "SomeFutureEvent",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "tool_use_id": "toolu_abc123"
        }),
        None
    )]
    // A missing event name is ignored.
    #[case::missing_event_skipped(
        json!({
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "tool_use_id": "toolu_abc123"
        }),
        None
    )]
    fn parses_agent_event(#[case] input: serde_json::Value, #[case] expected: Option<HookEvent>) {
        assert_eq!(HookEvent::from_json_str(&input.to_string()).unwrap(), expected);
    }

    /// Antigravity payloads decode through [`HookEvent::from_agy_json_str`],
    /// which additionally reports whether the event was a `PreToolUse` — the
    /// caller needs that even for skipped events to answer Antigravity's
    /// mandatory stdout contract.
    #[rstest]
    #[case::agy_pre_run_command_starts(
        json!({
            "hookEventName": "PreToolUse",
            "toolCall": {"name": "run_command", "args": {"CommandLine": "npm test", "Cwd": "/workspace"}},
            "stepIdx": 19,
            "conversationId": "ec33ebf9",
        }),
        (true, Some(HookEvent::Start {
            command: non_nul("npm test"),
            intent: None,
            tool_use_id: "ec33ebf9-19".into(),
        }))
    )]
    // Antigravity's pre-tool-use args carry no description, so there is never
    // an intent.
    #[case::agy_pre_has_no_intent(
        json!({
            "hookEventName": "PreToolUse",
            "toolCall": {"name": "run_command", "args": {"CommandLine": "ls"}},
            "stepIdx": 0,
            "conversationId": "conv",
        }),
        (true, Some(HookEvent::Start {
            command: non_nul("ls"),
            intent: None,
            tool_use_id: "conv-0".into(),
        }))
    )]
    // Non-shell tools are never recorded, but still count as pre-tool-use for
    // the stdout answer.
    #[case::agy_pre_other_tool_skipped_but_pre(
        json!({
            "hookEventName": "PreToolUse",
            "toolCall": {"name": "list_dir", "args": {"DirectoryPath": "/tmp"}},
            "stepIdx": 4,
            "conversationId": "conv",
        }),
        (true, None)
    )]
    // A missing tool call has nothing to record.
    #[case::agy_pre_missing_tool_call_skipped(
        json!({
            "hookEventName": "PreToolUse",
            "stepIdx": 4,
            "conversationId": "conv",
        }),
        (true, None)
    )]
    // Without a conversation id a start could never be matched to its end.
    #[case::agy_pre_missing_conversation_skipped(
        json!({
            "hookEventName": "PreToolUse",
            "toolCall": {"name": "run_command", "args": {"CommandLine": "ls"}},
            "stepIdx": 4,
        }),
        (true, None)
    )]
    // Without a step index a start could never be matched to its end.
    #[case::agy_pre_missing_step_skipped(
        json!({
            "hookEventName": "PreToolUse",
            "toolCall": {"name": "run_command", "args": {"CommandLine": "ls"}},
            "conversationId": "conv",
        }),
        (true, None)
    )]
    // An empty command has nothing to record.
    #[case::agy_pre_empty_command_skipped(
        json!({
            "hookEventName": "PreToolUse",
            "toolCall": {"name": "run_command", "args": {"CommandLine": ""}},
            "stepIdx": 4,
            "conversationId": "conv",
        }),
        (true, None)
    )]
    // A successful post-tool-use (empty error) records exit 0.
    #[case::agy_post_success_exits_zero(
        json!({
            "hookEventName": "PostToolUse",
            "toolCall": {"name": "run_command", "args": {"CommandLine": "npm test"}},
            "stepIdx": 5,
            "error": "",
            "conversationId": "ec33ebf9",
        }),
        (false, Some(HookEvent::End { tool_use_id: "ec33ebf9-5".into(), exit: 0 }))
    )]
    // A missing error field also means success.
    #[case::agy_post_missing_error_exits_zero(
        json!({
            "hookEventName": "PostToolUse",
            "toolCall": {"name": "run_command", "args": {"CommandLine": "ls"}},
            "stepIdx": 5,
            "conversationId": "conv",
        }),
        (false, Some(HookEvent::End { tool_use_id: "conv-5".into(), exit: 0 }))
    )]
    // A non-empty error string records exit 1; Antigravity sends no numeric code.
    #[case::agy_post_error_exits_one(
        json!({
            "hookEventName": "PostToolUse",
            "toolCall": {"name": "run_command", "args": {"CommandLine": "false"}},
            "stepIdx": 5,
            "error": "exit status 1",
            "conversationId": "conv",
        }),
        (false, Some(HookEvent::End { tool_use_id: "conv-5".into(), exit: 1 }))
    )]
    // An event name we don't model is ignored and is not pre-tool-use.
    #[case::agy_unknown_event_skipped(
        json!({
            "hookEventName": "Stop",
            "stepIdx": 5,
            "conversationId": "conv",
        }),
        (false, None)
    )]
    // A Claude-shaped payload carries no hookEventName, so the Antigravity
    // parser skips it rather than cross-reading another agent's events.
    #[case::agy_parser_ignores_claude_payload(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "tool_use_id": "toolu_abc123"
        }),
        (false, None)
    )]
    fn parses_agy_event(
        #[case] input: serde_json::Value,
        #[case] expected: (bool, Option<HookEvent>),
    ) {
        assert_eq!(HookEvent::from_agy_json_str(&input.to_string()).unwrap(), expected);
    }

    /// Well-formed JSON that isn't a hook event we model is skipped, not an
    /// error — it decodes cleanly as JSON but doesn't fit the schema.
    #[rstest]
    #[case::json_but_not_an_object("42")]
    #[case::missing_required_fields(r#"{"tool_name": "Bash"}"#)]
    #[case::wrong_typed_tool_use_id(
        r#"{"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_use_id": 5, "tool_input": {"command": "ls"}}"#
    )]
    fn well_formed_non_events_are_skipped(#[case] input: &str) {
        assert_eq!(HookEvent::from_json_str(input).unwrap(), None);
    }

    /// Malformed JSON is a genuine fault an agent could never send by design,
    /// so it surfaces as a typed error carrying the failure position.
    #[rstest]
    #[case::not_json("not json")]
    #[case::truncated(r#"{"tool_name":"#)]
    fn malformed_json_is_an_error(#[case] input: &str) {
        let ParseError::MalformedJson { line, column, .. } =
            HookEvent::from_json_str(input).unwrap_err();

        assert!(line >= 1 && column >= 1, "position should be 1-based, got {line}:{column}");
    }

    proptest! {
        /// Any Bash `PreToolUse` with a non-empty command becomes a `Start`
        /// carrying that command, the tool id, and the optional description as
        /// intent — regardless of the surrounding fields.
        #[test]
        fn bash_pre_tool_use_yields_start(
            command in r"[^\p{Cc}]+",
            tool_use_id in r"[^\p{Cc}]+",
            description in proptest::option::of(r"[^\p{Cc}]*"),
        ) {
            let mut tool_input = serde_json::Map::new();
            tool_input.insert("command".to_string(), json!(command));
            if let Some(intent) = &description {
                tool_input.insert("description".to_string(), json!(intent));
            }
            let input = json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Bash",
                "tool_input": serde_json::Value::Object(tool_input),
                "tool_use_id": tool_use_id,
            });

            prop_assert_eq!(
                HookEvent::from_json_str(&input.to_string()).unwrap(),
                Some(HookEvent::Start {
                    command: NonNulStr::new(command).unwrap(),
                    intent: description,
                    tool_use_id,
                })
            );
        }

        /// Any Bash `PostToolUse` reports the exit code verbatim, for every i64.
        #[test]
        fn bash_post_tool_use_reports_exit_code(
            exit in any::<i64>(),
            tool_use_id in r"[^\p{Cc}]+",
        ) {
            let input = json!({
                "hook_event_name": "PostToolUse",
                "tool_name": "Bash",
                "tool_response": {"exitCode": exit},
                "tool_use_id": tool_use_id,
            });

            prop_assert_eq!(
                HookEvent::from_json_str(&input.to_string()).unwrap(),
                Some(HookEvent::End { tool_use_id, exit })
            );
        }

        /// `PostToolUseFailure` always records exit 1, whatever the response
        /// claims.
        #[test]
        fn failure_event_always_exits_one(
            reported_exit in any::<i64>(),
            tool_use_id in r"[^\p{Cc}]+",
        ) {
            let input = json!({
                "hook_event_name": "PostToolUseFailure",
                "tool_name": "Bash",
                "tool_response": {"exitCode": reported_exit},
                "tool_use_id": tool_use_id,
            });

            prop_assert_eq!(
                HookEvent::from_json_str(&input.to_string()).unwrap(),
                Some(HookEvent::End { tool_use_id, exit: 1 })
            );
        }

        /// Any tool other than Bash is skipped, whatever the event or fields.
        #[test]
        fn non_bash_tool_is_always_skipped(
            tool_name in r"[^\p{Cc}]+".prop_filter("must not be Bash", |s| s.as_str() != "Bash"),
            event in proptest::sample::select(vec![
                "PreToolUse", "PostToolUse", "PostToolUseFailure", "Frobnicate",
            ]),
            tool_use_id in r"[^\p{Cc}]+",
        ) {
            let input = json!({
                "hook_event_name": event,
                "tool_name": tool_name,
                "tool_input": {"command": "ls"},
                "tool_response": {"exitCode": 0},
                "tool_use_id": tool_use_id,
            });

            prop_assert_eq!(HookEvent::from_json_str(&input.to_string()).unwrap(), None);
        }
    }
}
