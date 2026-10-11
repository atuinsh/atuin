//! The domain event the hook command acts on.
//!
//! When an agent sends a hook event through the [`WireHookEvent`] interface, this may or may not be
//! an event we care about. If we don't know how to deserialize it/don't care for it, we need to
//! drop that on the floor. The [`HookEvent`] type represents agent events we know/care about.

use std::path::PathBuf;

use atuin_common::string::NonNulStr;
use serde_json::error::Category;

use super::wire::{HookEventName, WireHookEvent, WireToolName};
use crate::i18n::fl;

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
    #[error("{}", fl!("hook-malformed-json", line = *.line, column = *.column))]
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
        /// Its exit code, when the event says it.
        exit: Option<i64>,
        /// The session's transcript, where Codex records the exit code its event doesn't say.
        transcript: Option<PathBuf>,
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
            HookEventName::PostToolUse => Some(HookEvent::End {
                tool_use_id,
                exit: wire.tool_response.and_then(|response| response.exit_code()),
                transcript: wire.transcript_path,
            }),
            // Failed: with the exit code its error starts with (`Exit code 2`), else 1.
            HookEventName::PostToolUseFailure => Some(HookEvent::End {
                tool_use_id,
                exit: Some(wire.error.as_deref().and_then(failure_exit_code).unwrap_or(1)),
                transcript: wire.transcript_path,
            }),
            HookEventName::Other => None,
        }
    }
}

/// The exit code a Claude Code failure's `error` starts with (`Exit code <n>`), if it does.
fn failure_exit_code(error: &str) -> Option<i64> {
    error.lines().next()?.strip_prefix("Exit code ")?.trim().parse().ok()
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
        Some(HookEvent::End { tool_use_id: "toolu_abc123".into(), exit: Some(3), transcript: None })
    )]
    #[case::post_tool_use_without_exit_code_says_none(
        json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": {},
            "tool_use_id": "toolu_abc123"
        }),
        Some(HookEvent::End { tool_use_id: "toolu_abc123".into(), exit: None, transcript: None })
    )]
    // Nor does a null exitCode.
    #[case::null_exit_code_says_none(
        json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": {"exitCode": null},
            "tool_use_id": "toolu_abc123"
        }),
        Some(HookEvent::End { tool_use_id: "toolu_abc123".into(), exit: None, transcript: None })
    )]
    // PostToolUseFailure without an exit code in its error is exit 1, whatever tool_response says.
    #[case::failure_forces_exit_one_ignoring_response(
        json!({
            "hook_event_name": "PostToolUseFailure",
            "tool_name": "Bash",
            "tool_input": {"command": "false"},
            "tool_response": {"exitCode": 0},
            "tool_use_id": "toolu_abc123"
        }),
        Some(HookEvent::End { tool_use_id: "toolu_abc123".into(), exit: Some(1), transcript: None })
    )]
    // Claude Code's failure says a shell command's exit code first in its error (Claude Code
    // 2.1.291, `PostToolUseFailure`'s `error`).
    #[case::failure_with_its_exit_code(
        json!({
            "hook_event_name": "PostToolUseFailure",
            "tool_name": "Bash",
            "tool_input": {"command": "ls /nope"},
            "tool_use_id": "toolu_abc123",
            "error": "Exit code 2\nls: cannot access '/nope': No such file or directory",
            "is_interrupt": false
        }),
        Some(HookEvent::End { tool_use_id: "toolu_abc123".into(), exit: Some(2), transcript: None })
    )]
    // Codex 0.160 says only the output, and where its transcript is (a real payload's fields).
    #[case::codex_post_tool_use(
        json!({
            "session_id": "01a11e4d-75d7-70f2-8c71-7f1cc87cbe85",
            "turn_id": "01a11e4d-761f-7073-955e-e96a0a319fc3",
            "transcript_path": "/w/.codex/sessions/2026/10/09/rollout-x.jsonl",
            "cwd": "/w",
            "hook_event_name": "PostToolUse",
            "model": "gpt-6-luna",
            "permission_mode": "bypassPermissions",
            "tool_name": "Bash",
            "tool_input": {"command": "ls /nonexistent_juliettea"},
            "tool_response": "ls: cannot access '/nonexistent_juliettea': No such file or directory\n",
            "tool_use_id": "exec-55bd0a92-7032-4356-b380-ac1cc1ea66f4"
        }),
        Some(HookEvent::End {
            tool_use_id: "exec-55bd0a92-7032-4356-b380-ac1cc1ea66f4".into(),
            exit: None,
            transcript: Some("/w/.codex/sessions/2026/10/09/rollout-x.jsonl".into()),
        })
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

    /// Codex serializes `tool_response` as a bare string, unlike the object
    /// Claude Code sends. Dropping the completion leaves the history entry
    /// opened by the matching `PreToolUse` unfinished, so nothing is recorded
    /// when the daemon owns the store (issue #4169).
    #[rstest]
    #[case::string_tool_response(
        r#"{"hook_event_name":"PostToolUse","tool_name":"Bash","tool_use_id":"example-call","session_id":"example-session","cwd":"/tmp","tool_input":{"command":"printf probe"},"tool_response":"probe"}"#,
        Some(HookEvent::End { tool_use_id: "example-call".into(), exit: None, transcript: None })
    )]
    fn completion_accepts_string_tool_response(
        #[case] input: &str,
        #[case] expected: Option<HookEvent>,
    ) {
        assert_eq!(HookEvent::from_json_str(input).unwrap(), expected);
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
        let err = HookEvent::from_json_str(input).unwrap_err();
        let ParseError::MalformedJson { line, column, .. } = err;

        assert!(line >= 1 && column >= 1, "position should be 1-based, got {line}:{column}");
        assert_eq!(
            err.to_string(),
            format!("hook payload is not valid JSON at line {line}, column {column}")
        );
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
                Some(HookEvent::End { tool_use_id, exit: Some(exit), transcript: None })
            );
        }

        /// `PostToolUseFailure` records exit 1, whatever the response claims,
        /// unless its error says the exit code.
        #[test]
        fn failure_without_an_exit_code_in_its_error_exits_one(
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
                Some(HookEvent::End { tool_use_id, exit: Some(1), transcript: None })
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
