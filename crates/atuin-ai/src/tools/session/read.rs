//! `atuin_ai_session_read`: page through one captured AI-agent session's transcript.

use std::fmt::Write as _;

use atuin_client::ai_session::{HarnessSession, Message, Session, human_prompt};
use atuin_client::settings::Settings;
use atuin_common::harnesstools::session::{Content, Role};
use atuin_common::range::Clamped;
use atuin_common::string::NonBlankString;
use atuin_common::time::UtcOffsetExt;
use atuin_daemon::AiClient;
use atuin_daemon::grpc::ai::session::pb::get_session_event::Event;
use futures::StreamExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::caller::{Caller, is_own};
use super::{connect, is_subagent, label, one_line, timestamp, truncate};
use crate::commands::session::{harness_name, select_session};
use crate::tools::ToolOutcome;

/// Per-block character budgets in the default (abridged) view. Tool output dominates transcripts
/// by volume, and a model reading back a session mostly needs what was asked, said and decided.
const TEXT_CHARS: usize = 2_000;
const THINKING_CHARS: usize = 600;
const TOOL_INPUT_CHARS: usize = 300;
const TOOL_RESULT_CHARS: usize = 400;
/// The per-block budget for a single-message read; still bounded so one giant tool result
/// cannot flood the context.
const FULL_CHARS: usize = 20_000;
/// Roughly how much a multi-message page may hold (about 4k tokens) before it ends early.
const PAGE_CHARS: usize = 16_000;

// Doc comments on the fields are the descriptions the model reads in the tool schema.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AtuinAiSessionReadToolCall {
    /// The session to read: a full session id or a unique prefix of one, as returned by
    /// atuin_ai_session_list or atuin_ai_session_search. 'latest' reads the most recent session
    /// before this one.
    pub session_id: NonBlankString,
    /// Message number to start from (0-based). Negative values count back from the end over
    /// messages with content, so -10 shows the last ten things said. To see a search hit in
    /// context, start a few messages before its message number.
    #[serde(default)]
    pub start: i64,
    /// Maximum number of messages to show. Long messages and harness-injected context are
    /// abridged on a multi-message page; read a single message (limit: 1) to see it whole.
    #[serde(default)]
    pub limit: Clamped<u32, 1, 200, 40>,
}

impl AtuinAiSessionReadToolCall {
    /// A single-message read is the way to see one message whole, so it is never abridged.
    fn full(&self) -> bool {
        self.limit.get() == 1
    }

    pub(crate) async fn execute(&self, settings: &Settings, caller: &Caller<'_>) -> ToolOutcome {
        let mut client = match connect(settings).await {
            Ok(client) => client,
            Err(outcome) => return outcome,
        };

        let selector = self.session_id.trim();
        let sessions = match super::list_sessions(&mut client, None).await {
            Ok(sessions) => sessions,
            Err(e) => return ToolOutcome::Error(format!("Listing AI sessions failed: {e}")),
        };
        // `latest` means the last session a person ran before this one: not the caller's own
        // live session, and not a subagent fragment.
        let sessions = if selector.eq_ignore_ascii_case("latest") {
            let own = caller.own_session_id(&mut client).await;
            sessions.into_iter().filter(|s| !is_own(s, own.as_deref()) && !is_subagent(s)).collect()
        } else {
            sessions
        };
        let handle = match select_session(sessions, selector) {
            Ok(handle) => handle,
            Err(e) => {
                return ToolOutcome::Error(
                    e.to_string().replace("atuin ai session list", "atuin_ai_session_list"),
                );
            }
        };

        let (session, messages) = match read_session(&mut client, handle).await {
            Ok(read) => read,
            Err(e) => return ToolOutcome::Error(format!("Reading the AI session failed: {e}")),
        };
        ToolOutcome::Success(self.render(&session, &messages, time::UtcOffset::local_or_utc()))
    }

    fn render(&self, s: &Session, messages: &[Message], offset: time::UtcOffset) -> String {
        let total = messages.len();
        // Out-of-range starts (either sign) clamp to the ends of the transcript. A negative start
        // counts back over messages with something to read: sessions often end in metadata-only
        // rows (Codex), and `-1` landing on one would show an empty page.
        let magnitude = usize::try_from(self.start.unsigned_abs()).unwrap_or(usize::MAX);
        let start = if self.start < 0 {
            let readable: Vec<usize> = messages
                .iter()
                .enumerate()
                .filter(|(_, m)| has_content(m))
                .map(|(i, _)| i)
                .collect();
            readable.len().checked_sub(magnitude).map_or(0, |k| readable[k])
        } else {
            magnitude.min(total)
        };
        // `limit` counts messages with something to read, so a stretch of structural rows (a
        // session's opening attachments, say) cannot fill a page with nothing.
        let limit = self.limit.get() as usize;
        let mut end = start;
        let mut counted = 0;

        let mut out = String::new();
        let _ = writeln!(out, "session  {} [{}]", s.handle.session, harness_name(s.handle.harness));
        let label = label(s);
        if !label.is_empty() {
            let _ = writeln!(out, "title    {label}");
        }
        if let Some(cwd) = &s.cwd {
            let _ = writeln!(out, "cwd      {}", cwd.display());
        }
        if let Some(branch) = s.git_branch.as_deref().filter(|b| !b.is_empty()) {
            let _ = writeln!(out, "branch   {branch}");
        }
        if let Some(model) = &s.model {
            let _ = writeln!(out, "model    {model}");
        }
        if let Some(parent) = &s.parent {
            let _ = writeln!(out, "parent   {} (this is a subagent session)", parent.session);
        }
        let _ = writeln!(
            out,
            "time     {} → {}",
            timestamp(s.started_at, offset),
            timestamp(s.updated_at, offset),
        );
        let _ = writeln!(out);

        let mut shown = 0;
        let mut repeats = 0;
        let mut previous: Option<(String, String)> = None;
        let mut tools = ToolRun::default();
        for (index, message) in messages.iter().enumerate().skip(start) {
            if has_content(message) {
                if counted == limit {
                    break;
                }
                counted += 1;
            }
            end = index + 1;
            if tools.absorb(index, message) {
                continue;
            }
            let flushed = tools.flush(&mut out);
            shown += flushed;
            if flushed > 0 {
                previous = None;
            }
            if let Some((role, body)) = self.render_message(message) {
                // Codex records each agent message twice (an event and a response item); show
                // a message identical to the one just before it once.
                if previous.as_ref().is_some_and(|(r, b)| *r == role && *b == body) {
                    repeats += 1;
                } else {
                    // `HH:MM` only: the header carries the date and sessions rarely cross
                    // midnight.
                    let when = timestamp(message.timestamp, offset);
                    let time = when.split_once(' ').map_or(when.as_str(), |(_, t)| t);
                    let _ = writeln!(out, "#{index} {role} {time}");
                    out.push_str(&body);
                    shown += 1;
                    previous = Some((role, body));
                }
            }
            // Stop at a message boundary once the page is big enough, whatever `limit` said:
            // the caller pages on with `start`, and a small-context model is not flooded.
            if out.len() >= PAGE_CHARS {
                break;
            }
        }
        shown += tools.flush(&mut out);

        // Numbering has gaps where harnesses recorded rows with nothing to read (attachments,
        // mode switches); say so, so a gap is not mistaken for a missing page.
        let hidden = (end - start) - shown - repeats;
        let _ = write!(out, "\n[messages {start}–{} of {total}", end.saturating_sub(1));
        if hidden > 0 {
            let _ = write!(out, "; {hidden} empty omitted");
        }
        if repeats > 0 {
            let _ = write!(out, "; {repeats} repeated omitted");
        }
        let _ = write!(out, "]");
        if end < total {
            let _ = write!(out, " More follows: read again with start: {end}.");
        }
        if start > 0 && self.start >= 0 {
            let _ = write!(out, " Earlier messages: start: {}.", start.saturating_sub(limit));
        }
        out.push('\n');
        out
    }

    /// A message's role label and rendered body, or `None` when it has no readable content:
    /// harnesses record plenty of structural rows (attachments, mode switches, usage-only lines).
    fn render_message(&self, m: &Message) -> Option<(String, String)> {
        let mut body = String::new();
        let mut injected = 0;
        for block in &m.content {
            match block {
                Content::Text(text) => {
                    let text = text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    match self.prompt_text(m, text) {
                        Some(text) => {
                            let _ = writeln!(body, "{}", self.clip(&text, TEXT_CHARS));
                        }
                        None => injected += text.chars().count(),
                    }
                }
                Content::Reasoning(text) => {
                    let text = text.trim();
                    if !text.is_empty() {
                        let _ = writeln!(body, "(thinking) {}", self.clip(text, THINKING_CHARS));
                    }
                }
                // A compaction summary stands in for the conversation before it, so it is often
                // the best account of what an earlier stretch of a long session did.
                Content::Summary(text) => {
                    let text = text.trim();
                    if !text.is_empty() {
                        let _ = writeln!(
                            body,
                            "(summary of earlier conversation) {}",
                            self.clip(text, TEXT_CHARS)
                        );
                    }
                }
                Content::Error(text) => {
                    let _ = writeln!(body, "(model error) {}", one_line(text, TOOL_RESULT_CHARS));
                }
                Content::ToolUse(call) if call.input.is_null() => {
                    let _ = writeln!(body, "→ {}", call.name);
                }
                Content::ToolUse(call) => {
                    let _ = writeln!(body, "→ {}: {}", call.name, self.tool_input(&call.input));
                }
                Content::ToolResult(result) => match result_text(&result.output) {
                    None if result.error => {
                        let _ = writeln!(body, "← error");
                    }
                    None => {}
                    Some(content) => {
                        let mark = if result.error {
                            "← error"
                        } else {
                            "←"
                        };
                        let content = if self.full() {
                            self.clip(&content, FULL_CHARS)
                        } else {
                            one_line(&content, TOOL_RESULT_CHARS)
                        };
                        let _ = writeln!(body, "{mark} {content}");
                    }
                },
                // Capture keeps only that reasoning happened, which says nothing to a reader.
                Content::ReasoningSummary { .. } | Content::Other(_) => {}
            }
        }
        if injected > 0 {
            let _ = writeln!(
                body,
                "(harness-injected context, {injected} chars; read this message alone to see it)"
            );
        }
        if body.is_empty() {
            return None;
        }

        let is_tool_result = m.content.iter().all(|b| matches!(b, Content::ToolResult(_)));
        let role = if is_tool_result {
            "tool"
        } else {
            match &m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::System => "system",
                Role::Tool => "tool",
                Role::Other(label) => label.as_str(),
            }
        };
        Some((role.to_owned(), body))
    }

    /// The text to show for a message's text block, or `None` when the harness wrote it rather
    /// than a person or the model (Codex developer prompts and `<environment_context>`, Claude
    /// Code `<system-reminder>`s): bulky, repeated every session, and never what a reader is
    /// after. A single-message read shows everything.
    fn prompt_text(&self, m: &Message, text: &str) -> Option<String> {
        if self.full() {
            return Some(text.to_owned());
        }
        match m.role {
            Role::Assistant => Some(text.to_owned()),
            Role::User => human_prompt(text),
            _ => None,
        }
    }

    fn clip(&self, text: &str, budget: usize) -> String {
        let budget = if self.full() {
            FULL_CHARS
        } else {
            budget
        };
        let chars = text.chars().count();
        if chars <= budget {
            return text.to_owned();
        }
        format!("{} […{} more chars]", truncate(text, budget).trim_end_matches('…'), chars - budget)
    }

    /// A tool call's input, led by the argument that says what it did (the command, file or
    /// pattern) rather than raw JSON, which is mostly noise in the abridged view.
    fn tool_input(&self, input: &Value) -> String {
        const KEYS: [&str; 7] = ["command", "cmd", "file_path", "path", "pattern", "url", "query"];
        let raw = match input {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if self.full() {
            return truncate(&raw, FULL_CHARS);
        }
        if let Value::Object(map) = input
            && let Some(value) = KEYS.iter().find_map(|k| map.get(*k))
        {
            let text = match value {
                Value::String(s) => s.clone(),
                Value::Array(parts) => {
                    parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")
                }
                other => other.to_string(),
            };
            return one_line(&text, TOOL_INPUT_CHARS);
        }
        one_line(&raw, TOOL_INPUT_CHARS)
    }
}

/// Fetch a session and all its messages, decoded into domain types.
async fn read_session(
    client: &mut AiClient,
    handle: HarnessSession,
) -> eyre::Result<(Session, Vec<Message>)> {
    let mut stream = client.get_session(handle).await?;
    let mut session = None;
    let mut messages = Vec::new();
    while let Some(event) = stream.next().await {
        match event?.event {
            Some(Event::Session(s)) => session = Some(Session::try_from(s)?),
            Some(Event::Message(m)) => messages.push(Message::try_from(m)?),
            None => {}
        }
    }
    let session = session.ok_or_else(|| eyre::eyre!("the daemon returned no session header"))?;
    Ok((session, messages))
}

/// A tool result's output as text, or `None` when capture did not keep it (a JSON null) or it is
/// blank. A string output is the raw text; anything else is shown as JSON.
fn result_text(output: &Value) -> Option<String> {
    let text = match output {
        Value::Null => return None,
        Value::String(s) => s.trim().to_owned(),
        other => other.to_string(),
    };
    (!text.is_empty()).then_some(text)
}

/// Whether a message renders as anything: harnesses record plenty of structural rows
/// (attachments, mode switches, usage-only lines) with nothing to read.
fn has_content(m: &Message) -> bool {
    m.content.iter().any(|block| match block {
        Content::Text(text)
        | Content::Reasoning(text)
        | Content::Summary(text)
        | Content::Error(text) => !text.trim().is_empty(),
        Content::ToolUse(_) => true,
        Content::ToolResult(result) => result.error || result_text(&result.output).is_some(),
        Content::ReasoningSummary { .. } | Content::Other(_) => false,
    })
}

/// A run of consecutive messages that are only tool calls and results whose arguments and output
/// were not captured. Rendered as one line (`#13–#24 tools: Grep, Bash, Read`) instead of a
/// numbered entry per call and per result.
#[derive(Default)]
struct ToolRun {
    first: Option<usize>,
    last: usize,
    /// Messages folded into the run, including empty structural rows between tool calls.
    messages: usize,
    names: Vec<String>,
    errors: usize,
}

impl ToolRun {
    /// Fold `m` into the run if it carries nothing but uncaptured tool activity (reasoning
    /// placeholders included), or nothing at all while a run is open; `false` leaves it for
    /// normal rendering.
    fn absorb(&mut self, index: usize, m: &Message) -> bool {
        let mut names = Vec::new();
        let mut errors = 0;
        let mut any_tool = false;
        for block in &m.content {
            match block {
                Content::ToolUse(call) if call.input.is_null() => {
                    any_tool = true;
                    names.push(call.name.clone());
                }
                Content::ToolResult(result) if result_text(&result.output).is_none() => {
                    any_tool = true;
                    errors += usize::from(result.error);
                }
                Content::ReasoningSummary { .. } | Content::Other(_) => {}
                _ => return false,
            }
        }
        if !any_tool && self.first.is_none() {
            return false;
        }
        self.first.get_or_insert(index);
        self.last = index;
        self.messages += 1;
        self.names.extend(names);
        self.errors += errors;
        true
    }

    /// Write the pending run, if any, and reset. Returns how many messages it covered.
    fn flush(&mut self, out: &mut String) -> usize {
        let Some(first) = self.first.take() else {
            return 0;
        };
        let range = if first == self.last {
            format!("#{first}")
        } else {
            format!("#{first}–#{}", self.last)
        };
        let _ = write!(out, "{range} tools: {}", Self::summarise(&self.names));
        match self.errors {
            0 => {}
            1 => out.push_str(" (1 error)"),
            n => {
                let _ = write!(out, " ({n} errors)");
            }
        }
        out.push('\n');
        let messages = self.messages;
        *self = Self::default();
        messages
    }

    /// Tool names in call order, with consecutive repeats collapsed: `Read ×3, Grep, Bash`.
    fn summarise(names: &[String]) -> String {
        if names.is_empty() {
            return "results".to_owned();
        }
        let mut parts: Vec<String> = Vec::new();
        let mut iter = names.iter().peekable();
        while let Some(name) = iter.next() {
            let mut count = 1;
            while iter.next_if(|next| *next == name).is_some() {
                count += 1;
            }
            parts.push(if count == 1 {
                name.clone()
            } else {
                format!("{name} ×{count}")
            });
        }
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::ai_session::{HarnessKind, NativeSessionId, SourceId};
    use atuin_common::harnesstools::session::{ToolCallId, ToolResult, ToolUse};
    use atuin_domain::record::RecordId;
    use rstest::rstest;
    use serde_json::json;
    use time::OffsetDateTime;

    use super::*;

    fn call(args: Value) -> AtuinAiSessionReadToolCall {
        serde_json::from_value(args).unwrap()
    }

    fn blocks(role: Role, content: Vec<Content>) -> Message {
        Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(HarnessSession {
                harness: HarnessKind::ClaudeCode,
                session: NativeSessionId::from("abc".to_owned()),
            })
            .source_id(SourceId::from("s".to_owned()))
            .timestamp(OffsetDateTime::UNIX_EPOCH)
            .role(role)
            .content(content)
            .build()
    }

    fn text(role: Role, text: &str) -> Message {
        blocks(role, vec![Content::Text(text.to_owned())])
    }

    fn empty() -> Message {
        blocks(Role::Assistant, vec![])
    }

    fn session() -> Session {
        super::super::fixtures::session("abc", Some("/work/x"), OffsetDateTime::UNIX_EPOCH)
    }

    fn tool_call(name: &str) -> Content {
        Content::ToolUse(ToolUse {
            id: ToolCallId::from("c".to_owned()),
            name: name.to_owned(),
            input: Value::Null,
        })
    }

    fn tool_result(error: bool) -> Content {
        Content::ToolResult(ToolResult {
            call: ToolCallId::from("c".to_owned()),
            output: Value::Null,
            error,
        })
    }

    fn render(args: Value, msgs: &[Message]) -> String {
        call(args).render(&session(), msgs, time::UtcOffset::UTC)
    }

    #[rstest]
    fn pages_skip_empty_rows_but_keep_their_numbers() {
        let msgs = vec![
            text(Role::User, "hello"),
            empty(),
            text(Role::Assistant, "hi there"),
            text(Role::User, "bye"),
        ];
        let out = render(json!({"session_id": "abc", "limit": 2}), &msgs);
        assert!(out.contains("#0 user"));
        assert!(!out.contains("#1 "));
        assert!(out.contains("#2 assistant"));
        assert!(!out.contains("bye"), "the empty row does not count towards the limit: {out}");
        assert!(out.contains("messages 0–2 of 4; 1 empty omitted"), "{out}");
        assert!(out.contains("start: 3"));
    }

    #[rstest]
    fn uncaptured_tool_activity_folds_into_one_line() {
        let msgs = vec![
            text(Role::User, "go"),
            blocks(Role::Assistant, vec![
                Content::ReasoningSummary { tokens: Some(3) },
                tool_call("Grep"),
            ]),
            blocks(Role::User, vec![tool_result(false)]),
            empty(),
            blocks(Role::Assistant, vec![tool_call("Grep"), tool_call("Bash")]),
            blocks(Role::User, vec![tool_result(true)]),
            text(Role::Assistant, "done"),
        ];
        let out = render(json!({"session_id": "abc"}), &msgs);
        assert!(out.contains("#1–#5 tools: Grep ×2, Bash (1 error)\n#6 assistant"), "{out}");
        assert!(!out.contains("Reasoning"));
        assert!(out.contains("[messages 0–6 of 7]"), "{out}");
    }

    #[rstest]
    fn captured_tool_input_and_output_are_shown_not_folded() {
        let msgs = vec![
            blocks(Role::Assistant, vec![Content::ToolUse(ToolUse {
                id: ToolCallId::from("c".to_owned()),
                name: "Bash".to_owned(),
                input: json!({"command": "cargo test"}),
            })]),
            blocks(Role::User, vec![Content::ToolResult(ToolResult {
                call: ToolCallId::from("c".to_owned()),
                output: json!("test result: ok"),
                error: false,
            })]),
        ];
        let out = render(json!({"session_id": "abc"}), &msgs);
        assert!(out.contains("→ Bash: cargo test"), "{out}");
        assert!(out.contains("← test result: ok"), "{out}");
    }

    #[rstest]
    fn summaries_and_errors_are_shown() {
        let msgs = vec![blocks(Role::Assistant, vec![
            Content::Summary("earlier we fixed the parser".to_owned()),
            Content::Error("rate limited".to_owned()),
        ])];
        let out = render(json!({"session_id": "abc"}), &msgs);
        assert!(out.contains("(summary of earlier conversation) earlier we fixed the parser"));
        assert!(out.contains("(model error) rate limited"));
    }

    #[rstest]
    fn harness_injected_text_collapses_unless_full() {
        let msgs = vec![
            text(Role::Other("developer".to_owned()), "<permissions instructions>sandbox rules"),
            text(Role::User, "<environment_context><cwd>/x</cwd></environment_context>"),
            text(Role::User, "fix this"),
        ];
        let abridged = render(json!({"session_id": "abc"}), &msgs);
        assert!(!abridged.contains("sandbox rules"), "{abridged}");
        assert!(!abridged.contains("<cwd>"), "{abridged}");
        assert!(abridged.contains("#0 developer"), "{abridged}");
        assert!(abridged.contains("harness-injected context"), "{abridged}");
        assert!(abridged.contains("#2 user 00:00\nfix this"), "{abridged}");

        let full = render(json!({"session_id": "abc", "limit": 1}), &msgs);
        assert!(full.contains("sandbox rules"));
    }

    #[rstest]
    fn a_page_ends_early_at_its_character_budget() {
        let msgs: Vec<_> =
            (0..20).map(|i| text(Role::User, &format!("{i}{}", "x".repeat(TEXT_CHARS)))).collect();
        let out = render(json!({"session_id": "abc"}), &msgs);
        let shown = out.matches(" user ").count();
        assert!((1..20).contains(&shown), "{shown} messages shown");
        assert!(out.len() < PAGE_CHARS + TEXT_CHARS + 500, "{}", out.len());
        assert!(
            out.contains(&format!("of 20] More follows: read again with start: {shown}.")),
            "{out}"
        );
    }

    #[rstest]
    fn negative_start_skips_trailing_empty_rows() {
        let msgs =
            vec![text(Role::User, "fix this"), text(Role::Assistant, "done"), empty(), empty()];
        let out = render(json!({"session_id": "abc", "start": -1, "limit": 1}), &msgs);
        assert!(out.contains("#1 assistant 00:00\ndone"), "{out}");
    }

    #[rstest]
    fn a_message_repeating_the_previous_one_is_shown_once() {
        let msgs = vec![
            text(Role::Assistant, "removing the dead helper"),
            text(Role::Assistant, "removing the dead helper"),
            text(Role::User, "removing the dead helper"),
        ];
        let out = render(json!({"session_id": "abc"}), &msgs);
        assert_eq!(out.matches("removing the dead helper").count(), 2, "{out}");
        assert!(out.contains("#0 assistant") && !out.contains("#1 ") && out.contains("#2 user"));
        assert!(out.contains("[messages 0–2 of 3; 1 repeated omitted]"), "{out}");
    }

    #[rstest]
    fn empty_rows_do_not_use_up_the_limit() {
        let mut msgs: Vec<_> = (0..8).map(|_| empty()).collect();
        msgs.push(text(Role::User, "the real prompt"));
        msgs.push(text(Role::Assistant, "the reply"));
        let out = render(json!({"session_id": "abc", "limit": 1}), &msgs);
        assert!(out.contains("#8 user 00:00\nthe real prompt"), "{out}");
        assert!(
            out.contains(
                "[messages 0–8 of 10; 8 empty omitted] More follows: read again with start: 9."
            ),
            "{out}"
        );
    }

    #[rstest]
    fn negative_start_counts_from_the_end() {
        let msgs = vec![text(Role::User, "a"), text(Role::User, "b"), text(Role::User, "c")];
        let out = render(json!({"session_id": "abc", "start": -1}), &msgs);
        assert!(out.contains("#2 user"));
        assert!(!out.contains("#1 user"));
    }

    #[rstest]
    #[case::out_of_range_positive(json!({"session_id": "abc", "start": 99}), "[messages 3–2 of 3]")]
    #[case::out_of_range_negative(json!({"session_id": "abc", "start": -99}), "#0 user")]
    fn out_of_range_starts_clamp(#[case] args: Value, #[case] want: &str) {
        let msgs = vec![text(Role::User, "a"), text(Role::User, "b"), text(Role::User, "c")];
        let out = render(args, &msgs);
        assert!(out.contains(want), "{out}");
    }

    #[rstest]
    #[case::command(json!({"command": "cargo test", "description": "run"}), "cargo test")]
    #[case::file(json!({"file_path": "/a/b.rs", "old_string": "x"}), "/a/b.rs")]
    #[case::codex_argv(json!({"cmd": ["bash", "-lc", "ls"]}), "bash -lc ls")]
    #[case::other(json!({"x": 1}), r#"{"x":1}"#)]
    fn tool_input_leads_with_what_it_did(#[case] input: Value, #[case] want: &str) {
        assert_eq!(call(json!({"session_id": "abc"})).tool_input(&input), want);
    }

    #[rstest]
    fn long_text_is_clipped_unless_full() {
        let long = "x".repeat(TEXT_CHARS + 10);
        let abridged = call(json!({"session_id": "abc"})).clip(&long, TEXT_CHARS);
        assert!(abridged.ends_with("[…10 more chars]"));
        let full = call(json!({"session_id": "abc", "limit": 1})).clip(&long, TEXT_CHARS);
        assert_eq!(full, long);
    }
}
