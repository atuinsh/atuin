//! `atuin_ai_session_read`: page through one captured AI-agent session's transcript.

use std::fmt::Write as _;

use atuin_client::ai_session::{HarnessSession, Message, Session};
use atuin_client::settings::Settings;
use atuin_common::harnesstools::session::{Content, Role};
use atuin_common::range::Clamped;
use atuin_common::string::{NonBlankString, TruncateCharsExt};
use atuin_common::time::UtcOffsetExt;
use atuin_daemon::AiClient;
use atuin_daemon::grpc::ai::session::pb::get_session_event::Event;
use futures::StreamExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::caller::{Caller, is_own};
use super::{connect, is_subagent, label, timestamp};
use crate::commands::session::{SelectError, harness_name, message_role, one_line, select_session};
use crate::tools::ToolOutcome;

/// Roughly how much a multi-message page may hold (about 4k tokens) before it ends early.
const PAGE_CHARS: usize = 16_000;

/// How much of one message a single-message read returns at a time: a long message (a pasted
/// blob, say) comes in windows the reader walks with `offset`, so no response floods the context
/// and nothing is out of reach.
const WINDOW_CHARS: usize = 20_000;

/// Per-part character budgets on a multi-message page: tool output dominates transcripts by
/// volume, and a reader mostly needs what was asked, said and decided. A single-message read
/// (`full`) cuts nothing per part; it is windowed as a whole instead.
const TEXT_CHARS: usize = 2_000;
const THINKING_CHARS: usize = 600;
const TOOL_INPUT_CHARS: usize = 300;
const TOOL_RESULT_CHARS: usize = 400;
/// Text a harness wrote into the conversation (standing context such as AGENTS.md or a sandbox
/// policy, but also one-off notes like a subagent's report) gets a line, not its bulk, on a page.
const HARNESS_CHARS: usize = 200;

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
    /// abridged on a multi-message page; read a single message (limit: 1) to see it in full.
    #[serde(default)]
    pub limit: Clamped<u32, 1, 200, 40>,
    /// With limit: 1, where to start within the message, in characters. A message longer than
    /// 20,000 characters comes in parts, each ending with the offset to read the next from.
    /// Ignored on multi-message pages.
    #[serde(default)]
    pub offset: usize,
}

impl AtuinAiSessionReadToolCall {
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
            let own = caller.own_in(&sessions);
            sessions.into_iter().filter(|s| !is_own(s, own.as_deref()) && !is_subagent(s)).collect()
        } else {
            sessions
        };
        let handle = match select_session(sessions, selector) {
            Ok(handle) => handle,
            Err(e @ SelectError::NotFound(_)) => {
                return ToolOutcome::Error(format!(
                    "{e}. Find one with atuin_ai_session_list or atuin_ai_session_search."
                ));
            }
            Err(e) => return ToolOutcome::Error(e.to_string()),
        };

        // A forward page needs the messages up to it plus one with content beyond (to know more
        // follows), not the rest of a possibly huge transcript; a negative start counts back
        // from the end, so it needs them all.
        let page = usize::try_from(self.start).ok().map(|start| Page {
            start,
            content: self.limit.get() as usize + 1,
        });
        let (session, messages, complete) = match read_session(&mut client, handle, page).await {
            Ok(read) => read,
            Err(e) => return ToolOutcome::Error(format!("Reading the AI session failed: {e}")),
        };
        ToolOutcome::Success(self.render(
            &session,
            &messages,
            complete,
            time::UtcOffset::local_or_utc(),
        ))
    }

    /// Render a page of `messages`: the whole transcript when `complete`, otherwise at least
    /// everything up to the page and one message with content past it.
    fn render(
        &self,
        s: &Session,
        messages: &[Message],
        complete: bool,
        offset: time::UtcOffset,
    ) -> String {
        let total = messages.len();
        let limit = self.limit.get() as usize;
        let full = self.limit.get() == 1;
        // Out-of-range starts (either sign) clamp to the ends of the transcript. A negative start
        // counts back over messages with something to read: sessions often end in metadata-only
        // rows (Codex), and `-1` landing on one would show an empty page.
        let magnitude = usize::try_from(self.start.unsigned_abs()).unwrap_or(usize::MAX);
        let start = if self.start < 0 {
            messages
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, m)| has_content(m))
                .nth(magnitude.saturating_sub(1))
                .map_or(0, |(i, _)| i)
        } else {
            magnitude.min(total)
        };

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

        // `limit` counts messages with something to read, so a stretch of structural rows (a
        // session's opening attachments, say) cannot fill a page with nothing.
        let mut end = start;
        let mut counted = 0;
        let mut shown = 0;
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
            shown += tools.flush(&mut out);
            if let Some((role, body)) = render_message(message, full) {
                let body = if full {
                    self.window(&body)
                } else {
                    body
                };
                // `HH:MM` only: the header carries the date and sessions rarely cross midnight.
                let time = message
                    .timestamp
                    .checked_to_offset(offset)
                    .map(|t| format!(" {:02}:{:02}", t.hour(), t.minute()))
                    .unwrap_or_default();
                let _ = writeln!(out, "#{index} {role}{time}");
                out.push_str(&body);
                shown += 1;
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
        let hidden = (end - start) - shown;
        let _ = write!(out, "\n[messages {start}–{}", end.saturating_sub(1));
        if complete {
            let _ = write!(out, " of {total}");
        }
        if hidden > 0 {
            let _ = write!(out, "; {hidden} empty omitted");
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
}

impl AtuinAiSessionReadToolCall {
    /// The part of a single message's rendered `body` starting at `offset`, saying where the
    /// next part starts when there is more.
    fn window(&self, body: &str) -> String {
        let total = body.chars().count();
        if self.offset == 0 && total <= WINDOW_CHARS {
            return body.to_owned();
        }
        if self.offset >= total {
            return format!(
                "(offset {} is past the end of this message: {total} chars)\n",
                self.offset
            );
        }
        let end = (self.offset + WINDOW_CHARS).min(total);
        let mut part: String = body.chars().skip(self.offset).take(end - self.offset).collect();
        if !part.ends_with('\n') {
            part.push('\n');
        }
        let more = if end < total {
            format!(" Read the rest with offset: {end}.")
        } else {
            String::new()
        };
        let _ = writeln!(part, "[chars {}–{end} of {total}.{more}]", self.offset);
        part
    }
}

/// A message's role label and rendered body, or `None` when it has nothing to read.
fn render_message(m: &Message, full: bool) -> Option<(String, String)> {
    let mut body = String::new();
    for block in &m.content {
        match (classify(block), block) {
            (Block::Empty, _) => {}
            (
                Block::UncapturedTool {
                    name: Some(name), ..
                },
                _,
            ) => {
                let _ = writeln!(body, "→ {name}");
            }
            (Block::UncapturedTool { name: None, error }, _) => {
                if error {
                    let _ = writeln!(body, "← error");
                }
            }
            (Block::Readable, Content::Text(text)) => {
                // Only people and the model write user and assistant text; the parsers file
                // what a harness wrote (Codex developer prompts, Claude Code reminders, subagent
                // reports) under other roles. Often bulky and repeated every session, it gets a
                // line on a page.
                let text = text.trim();
                if full || matches!(m.role, Role::User | Role::Assistant) {
                    let _ = writeln!(body, "{}", clip(text, TEXT_CHARS, full));
                } else {
                    let _ = writeln!(body, "(harness) {}", one_line(text, HARNESS_CHARS));
                }
            }
            (Block::Readable, Content::Reasoning(text)) => {
                let _ = writeln!(body, "(thinking) {}", clip(text.trim(), THINKING_CHARS, full));
            }
            // A compaction summary stands in for the conversation before it, so it is often the
            // best account of what an earlier stretch of a long session did.
            (Block::Readable, Content::Summary(text)) => {
                let _ = writeln!(
                    body,
                    "(summary of earlier conversation) {}",
                    clip(text.trim(), TEXT_CHARS, full)
                );
            }
            (Block::Readable, Content::Error(text)) => {
                let _ = writeln!(body, "(model error) {}", one_line(text, TOOL_RESULT_CHARS));
            }
            (Block::Readable, Content::ToolUse(call)) => {
                let _ = writeln!(body, "→ {}: {}", call.name, tool_input(&call.input, full));
            }
            (Block::Readable, Content::ToolResult(result)) => {
                let output = result.output_text().unwrap_or_default();
                let output = output.trim();
                let mark = if result.error {
                    "← error"
                } else {
                    "←"
                };
                let output = if full {
                    output.to_owned()
                } else {
                    one_line(output, TOOL_RESULT_CHARS)
                };
                let _ = writeln!(body, "{mark} {output}");
            }
            (Block::Readable, Content::ReasoningSummary { .. } | Content::Other(_)) => {}
        }
    }
    if body.is_empty() {
        return None;
    }

    // A tool result is labelled `tool` whatever the envelope role: some harnesses model tool
    // output as a user turn.
    let role = if m.content.iter().all(|b| matches!(b, Content::ToolResult(_))) {
        "tool".to_owned()
    } else {
        message_role(m)
    };
    Some((role, body))
}

/// `text` cut to `budget` chars, saying how much was left out; whole when `full`.
fn clip(text: &str, budget: usize, full: bool) -> String {
    let head = if full {
        text
    } else {
        text.truncate_chars(budget)
    };
    if head.len() == text.len() {
        return text.to_owned();
    }
    format!("{head} […{} more chars]", text.chars().count() - budget)
}

/// A tool call's input, led by the argument that says what it did (the command, file or
/// pattern) rather than raw JSON, which is mostly noise in the abridged view.
fn tool_input(input: &Value, full: bool) -> String {
    const KEYS: [&str; 7] = ["command", "cmd", "file_path", "path", "pattern", "url", "query"];
    let raw = match input {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if full {
        return raw;
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

/// How much of a transcript a forward read needs: every message before `start`, then messages
/// until `content` of them (from `start` on) have something to read.
pub struct Page {
    start: usize,
    content: usize,
}

/// Fetch a session and its messages, decoded into domain types: all of them, or only as many as
/// `page` needs, dropping the stream there so the daemon stops too. The flag says whether the
/// transcript was read to its end.
pub async fn read_session(
    client: &mut AiClient,
    handle: HarnessSession,
    page: Option<Page>,
) -> eyre::Result<(Session, Vec<Message>, bool)> {
    let mut stream = client.get_session(handle).await?;
    let mut session = None;
    let mut messages = Vec::new();
    let mut content = 0;
    let mut complete = true;
    while let Some(event) = stream.next().await {
        match event?.event {
            Some(Event::Session(s)) => session = Some(Session::try_from(s)?),
            Some(Event::Message(m)) => {
                let m = Message::try_from(m)?;
                let past_start = page.as_ref().is_some_and(|p| messages.len() >= p.start);
                if past_start && has_content(&m) {
                    content += 1;
                }
                messages.push(m);
                if page.as_ref().is_some_and(|p| past_start && content >= p.content) {
                    complete = false;
                    break;
                }
            }
            None => {}
        }
    }
    let session = session.ok_or_else(|| eyre::eyre!("the daemon returned no session header"))?;
    Ok((session, messages, complete))
}

/// What a content block contributes to a transcript page. The one place that decides it, for
/// counting (`has_content`), folding (`ToolRun`) and rendering.
enum Block<'a> {
    /// Nothing to read: blank text, a reasoning placeholder, harness bookkeeping.
    Empty,
    /// A tool call or result whose arguments or output capture did not keep: a call is named, a
    /// result says only whether it failed.
    UncapturedTool {
        name: Option<&'a str>,
        error: bool,
    },
    Readable,
}

fn classify(block: &Content) -> Block<'_> {
    match block {
        Content::Text(text)
        | Content::Reasoning(text)
        | Content::Summary(text)
        | Content::Error(text) => {
            if text.trim().is_empty() {
                Block::Empty
            } else {
                Block::Readable
            }
        }
        Content::ToolUse(call) if call.input.is_null() => Block::UncapturedTool {
            name: Some(&call.name),
            error: false,
        },
        Content::ToolResult(result)
            if result.output_text().is_none_or(|output| output.trim().is_empty()) =>
        {
            Block::UncapturedTool {
                name: None,
                error: result.error,
            }
        }
        Content::ToolUse(_) | Content::ToolResult(_) => Block::Readable,
        // Capture keeps only that reasoning happened, which says nothing to a reader.
        Content::ReasoningSummary { .. } | Content::Other(_) => Block::Empty,
    }
}

/// Whether a message renders as anything: harnesses record plenty of structural rows
/// (attachments, mode switches, usage-only lines) with nothing to read.
fn has_content(m: &Message) -> bool {
    m.content.iter().any(|block| match classify(block) {
        Block::Readable => true,
        Block::UncapturedTool { name, error } => name.is_some() || error,
        Block::Empty => false,
    })
}

/// A run of consecutive messages that are only tool calls and results whose arguments and output
/// were not captured. Rendered as one line (`#13–#24 tools: Grep, Bash, Read`) instead of a
/// numbered entry per call and per result.
#[derive(Default)]
struct ToolRun {
    first: Option<usize>,
    last: usize,
    names: Vec<String>,
    errors: usize,
}

impl ToolRun {
    /// Fold `m` into the run if it carries nothing but uncaptured tool activity, or nothing at
    /// all while a run is open; `false` leaves it for normal rendering.
    fn absorb(&mut self, index: usize, m: &Message) -> bool {
        let mut names = Vec::new();
        let mut errors = 0;
        let mut any_tool = false;
        for block in &m.content {
            match classify(block) {
                Block::Readable => return false,
                Block::UncapturedTool { name, error } => {
                    any_tool = true;
                    names.extend(name.map(str::to_owned));
                    errors += usize::from(error);
                }
                Block::Empty => {}
            }
        }
        if !any_tool && self.first.is_none() {
            return false;
        }
        self.first.get_or_insert(index);
        self.last = index;
        self.names.extend(names);
        self.errors += errors;
        true
    }

    /// Write the pending run, if any, and reset. Returns how many messages it covered: a run is
    /// contiguous, since any message it does not absorb flushes it first.
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
        let messages = self.last - first + 1;
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
        call(args).render(&session(), msgs, true, time::UtcOffset::UTC)
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
    fn harness_text_gets_a_line_unless_full() {
        let msgs = vec![
            text(
                Role::Other("developer".to_owned()),
                &format!("<permissions instructions>sandbox rules{}", " policy".repeat(2_000)),
            ),
            // The Codex parser files `<environment_context>` under the system role.
            text(Role::System, "<environment_context><cwd>/x</cwd></environment_context>"),
            text(Role::User, "fix this"),
        ];
        let abridged = render(json!({"session_id": "abc"}), &msgs);
        assert!(abridged.contains("#0 developer"), "{abridged}");
        assert!(
            abridged.contains("(harness) <permissions instructions>sandbox rules"),
            "{abridged}"
        );
        assert!(abridged.len() < 2_000, "a long harness blob is one line: {}", abridged.len());
        assert!(abridged.contains("#2 user 00:00\nfix this"), "{abridged}");

        let full = render(json!({"session_id": "abc", "limit": 1}), &msgs);
        assert!(full.matches(" policy").count() >= 2_000, "a single-message read shows it all");
    }

    #[rstest]
    fn a_page_ends_early_at_its_character_budget() {
        let msgs: Vec<_> =
            (0..20).map(|i| text(Role::User, &format!("{i}{}", "x".repeat(2_000)))).collect();
        let out = render(json!({"session_id": "abc"}), &msgs);
        let shown = out.matches(" user ").count();
        assert!((1..20).contains(&shown), "{shown} messages shown");
        assert!(out.len() < PAGE_CHARS + 2_500, "{}", out.len());
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
    fn a_partial_transcript_omits_the_total() {
        let msgs = vec![text(Role::User, "a"), text(Role::User, "b"), text(Role::User, "c")];
        let out = call(json!({"session_id": "abc", "limit": 2})).render(
            &session(),
            &msgs,
            false,
            time::UtcOffset::UTC,
        );
        assert!(out.contains("[messages 0–1] More follows: read again with start: 2."), "{out}");
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
        assert_eq!(tool_input(&input, false), want);
    }

    #[rstest]
    fn a_long_single_message_comes_in_windows() {
        let long = format!("start{}end", "x".repeat(45_000));
        let msgs = vec![text(Role::User, &long)];
        let first = render(json!({"session_id": "abc", "limit": 1}), &msgs);
        assert!(first.contains("start") && !first.contains("end\n"), "the first window is cut");
        assert!(first.contains("Read the rest with offset: 20000."), "{first}");
        let last = render(json!({"session_id": "abc", "limit": 1, "offset": 40_000}), &msgs);
        assert!(last.contains("end\n") && !last.contains("start"), "{last}");
        assert!(!last.contains("Read the rest"), "{last}");
        let past = render(json!({"session_id": "abc", "limit": 1, "offset": 90_000}), &msgs);
        assert!(past.contains("past the end of this message"), "{past}");
    }

    #[rstest]
    fn long_text_is_clipped_unless_full() {
        let long = "é".repeat(2_010);
        let abridged = clip(&long, TEXT_CHARS, false);
        assert!(abridged.ends_with("[…10 more chars]"), "{abridged}");
        assert_eq!(clip(&long, TEXT_CHARS, true), long);
    }
}
