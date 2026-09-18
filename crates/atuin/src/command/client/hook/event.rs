//! The domain event the hook command acts on.
//!
//! When an agent sends a hook event through the [`WireHookEvent`] interface, this may or may not be
//! an event we care about. If we don't know how to deserialize it/don't care for it, we need to
//! drop that on the floor. The [`HookEvent`] type represents agent events we know/care about.

use std::fmt::Write as _;
use std::path::PathBuf;

use atuin_client::history::CommandCapture;
use atuin_common::string::NonNulStr;
use atuin_common::string::bounded_buffer::{BoundedBuffer, Limit};
use serde_json::error::Category;

use super::wire::{HookEventName, WireHookEvent, WireToolName, WireToolResponse};

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
        output: Option<CommandOutput>,
    },
}

/// What the agent reported a command printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// The output as the agent saw it, which may be cut at the agent's own limit.
    pub text: String,
    /// Where the agent kept the whole output when `text` is only part of it.
    pub file: Option<PathBuf>,
}

impl CommandOutput {
    /// Fit `text` into a capture the daemon stores: the middle goes once it exceeds
    /// `max_output_bytes`, and the cut lands on line boundaries so a secret can't straddle it
    /// and slip past redaction.
    pub fn into_capture(self, max_output_bytes: usize, secrets_filter: bool) -> CommandCapture {
        let mut buffer = BoundedBuffer::new(Limit::split_evenly(max_output_bytes));
        let _ = buffer.write_str(&self.text);
        let mut contents = buffer.take();
        if let Some(end) = &mut contents.end {
            contents.start.truncate(contents.start.rfind('\n').unwrap_or(0));
            end.drain(..end.find('\n').map_or(end.len(), |n| n + 1));
        }
        if secrets_filter {
            contents.start = atuin_common::secrets::redact(&contents.start).into_owned();
            contents.end = contents.end.map(|end| atuin_common::secrets::redact(&end).into_owned());
        }
        CommandCapture {
            output_start: contents.start,
            output_end: contents.end,
            output_observed_bytes: u64::try_from(self.text.len()).unwrap_or(u64::MAX),
            terminal_width: 0,
            terminal_height: 0,
        }
    }
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
                // TODO(markovejnovic): Is it safe to assume that no exit code
                //                      means "success"?
                let (exit, output) = match wire.tool_response {
                    Some(WireToolResponse::Object {
                        exit_code,
                        stdout,
                        stderr,
                        persisted_output_path,
                    }) => {
                        let mut text = stdout.unwrap_or_default();
                        if let Some(stderr) = stderr.filter(|stderr| !stderr.is_empty()) {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(&stderr);
                        }
                        let output = (!text.is_empty()).then_some(CommandOutput {
                            text,
                            file: persisted_output_path,
                        });
                        (exit_code.unwrap_or(0), output)
                    }
                    Some(WireToolResponse::Output(text)) => {
                        (0, Some(CommandOutput { text, file: None }))
                    }
                    None => (0, None),
                };
                Some(HookEvent::End {
                    tool_use_id,
                    exit,
                    output,
                })
            }
            HookEventName::PostToolUseFailure => Some(HookEvent::End {
                tool_use_id,
                exit: 1,
                output: None,
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

    fn inline(text: &str) -> CommandOutput {
        CommandOutput {
            text: text.into(),
            file: None,
        }
    }

    #[test]
    fn oversized_output_loses_its_middle_on_line_boundaries() {
        let mut text = String::new();
        for n in 0..40 {
            writeln!(text, "line {n:02}").unwrap();
        }
        let capture = inline(&text).into_capture(64, true);
        let end = capture.output_end.expect("a 320-byte output cannot fit in 64");
        assert_eq!(capture.output_start, "line 00\nline 01\nline 02\nline 03");
        assert_eq!(end, "line 37\nline 38\nline 39\n");
        assert_eq!(capture.output_observed_bytes, 320);
    }

    #[test]
    fn secrets_are_redacted_only_when_the_filter_is_on() {
        let text = "AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\n";
        assert!(!inline(text).into_capture(1024, true).output_start.contains("wJalrXUtnFEMI"));
        assert!(inline(text).into_capture(1024, false).output_start.contains("wJalrXUtnFEMI"));
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
        Some(HookEvent::End {
            tool_use_id: "toolu_abc123".into(),
            exit: 3,
            output: Some(inline("hello\n")),
        })
    )]
    // Claude Code reports stdout and stderr apart; stderr follows stdout.
    #[case::post_tool_use_joins_stderr_after_stdout(
        json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": {"exitCode": 1, "stdout": "partial", "stderr": "boom"},
            "tool_use_id": "toolu_abc123"
        }),
        Some(HookEvent::End {
            tool_use_id: "toolu_abc123".into(),
            exit: 1,
            output: Some(inline("partial\nboom")),
        })
    )]
    // Claude Code cuts stdout inline and points at the whole output on disk.
    #[case::post_tool_use_keeps_persisted_output_path(
        json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": {
                "exitCode": 0,
                "stdout": "first 30000 chars",
                "stderr": "",
                "persistedOutputPath": "/tmp/tool-results/abc.txt"
            },
            "tool_use_id": "toolu_abc123"
        }),
        Some(HookEvent::End {
            tool_use_id: "toolu_abc123".into(),
            exit: 0,
            output: Some(CommandOutput {
                text: "first 30000 chars".into(),
                file: Some("/tmp/tool-results/abc.txt".into()),
            }),
        })
    )]
    #[case::post_tool_use_without_exit_code_defaults_zero(
        json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": {},
            "tool_use_id": "toolu_abc123"
        }),
        Some(HookEvent::End { tool_use_id: "toolu_abc123".into(), exit: 0, output: None })
    )]
    // A null exitCode also defaults to 0.
    #[case::null_exit_code_defaults_zero(
        json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": {"exitCode": null},
            "tool_use_id": "toolu_abc123"
        }),
        Some(HookEvent::End { tool_use_id: "toolu_abc123".into(), exit: 0, output: None })
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
        Some(HookEvent::End { tool_use_id: "toolu_abc123".into(), exit: 1, output: None })
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
        Some(HookEvent::End {
            tool_use_id: "example-call".into(),
            exit: 0,
            output: Some(inline("probe")),
        })
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
                Some(HookEvent::End { tool_use_id, exit, output: None })
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
                Some(HookEvent::End { tool_use_id, exit: 1, output: None })
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
