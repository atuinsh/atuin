//! `atuin_ai_session_read`: page through one captured AI-agent session's transcript.

use std::fmt::Write as _;
use std::ops::Range;

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
use strum::IntoStaticStr;

use super::caller::{Caller, is_own};
use super::{connect, is_subagent, label, link_lines, parent_line, timestamp};
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
const TOOL_INPUT_CHARS: usize = 300;
const TOOL_RESULT_CHARS: usize = 400;
/// How much of a message around a query match a filtered page shows, when the abridged message
/// leaves the match out.
const SNIPPET_CHARS: usize = 240;

/// The labels a page puts on blocks, so a query match can tell them from what was said.
const SUMMARY_LABEL: &str = "(summary of earlier conversation)";
const MODEL_ERROR_LABEL: &str = "(model error)";
const TOOL_ERROR_MARK: &str = "← error";
const LABELS: [&str; 3] = [SUMMARY_LABEL, MODEL_ERROR_LABEL, TOOL_ERROR_MARK];

// Doc comments on the fields are the descriptions the model reads in the tool schema.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AtuinAiSessionReadToolCall {
    /// The session to read: a full session id or atuin id, or a unique prefix of one, as returned by
    /// atuin_ai_session_list or atuin_ai_session_search. 'latest' reads the most recent session
    /// before this one.
    pub session_id: NonBlankString,
    /// Message number to start from (0-based). Negative values count back from the end over
    /// messages with content, so -10 shows the last ten things said. To see a search hit in
    /// context, start a few messages before its message number. Numbers count every row the
    /// harness recorded, including ones with nothing to read, so they run past the message count
    /// atuin_ai_session_list gives; to reach the end, use a negative start.
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
    /// Only messages containing every one of these words (case-insensitive, anywhere in the
    /// text, tool input or output), to find where something was said in a long session without
    /// paging through it. Matches keep their numbers; to see one in context, read again from a
    /// few messages before it without query.
    #[serde(default)]
    pub query: Option<String>,
    /// Only messages from these roles: 'user' (what the person wrote), 'assistant' (the agent's
    /// replies and tool calls), 'tool' (tool output), 'harness' (context the harness injected).
    /// ["user"] skims what the person said, often the corrections and decisions that matter
    /// most. Omit for every role.
    #[serde(default)]
    pub roles: Option<Vec<RoleArg>>,
}

/// Who a message is from, as `roles` filters it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema, IntoStaticStr)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum RoleArg {
    User,
    Assistant,
    Tool,
    Harness,
}

impl RoleArg {
    /// Tool output is `tool` whatever the envelope role, as on a page; whatever is neither the
    /// person, the model nor a tool was written by the harness.
    fn of(m: &Message) -> Self {
        if is_tool_output(m) {
            return Self::Tool;
        }
        match m.role {
            Role::User => Self::User,
            Role::Assistant => Self::Assistant,
            Role::Tool => Self::Tool,
            Role::System | Role::Other(_) => Self::Harness,
        }
    }
}

/// Which messages a read shows: those from `roles` (any, when empty) containing every term.
struct Filter {
    terms: Vec<String>,
    roles: Vec<RoleArg>,
}

impl Filter {
    fn new(query: Option<&str>, roles: Option<&[RoleArg]>) -> Self {
        Self {
            terms: query.map(|q| q.split_whitespace().map(fold).collect()).unwrap_or_default(),
            roles: roles.map(<[RoleArg]>::to_vec).unwrap_or_default(),
        }
    }

    const fn is_active(&self) -> bool {
        !self.terms.is_empty() || !self.roles.is_empty()
    }

    /// Whether the page shows `m`: it has something to read and passes the filter.
    fn keeps(&self, m: &Message) -> bool {
        if !has_content(m) || (!self.roles.is_empty() && !self.roles.contains(&RoleArg::of(m))) {
            return false;
        }
        if self.terms.is_empty() {
            return true;
        }
        let text = fold(&searchable(m));
        self.terms.iter().all(|term| text.contains(term.as_str()))
    }

    /// The text around a term the abridged `shown` body cut off, so a match deep in a long
    /// message or tool output is visible on the page. A term only in the page's own markup (a
    /// role label, a clip marker) does not count as shown.
    fn snippet(&self, m: &Message, shown: &str) -> Option<String> {
        let shown = fold(&without_markup(shown));
        let term = self.terms.iter().find(|term| !shown.contains(term.as_str()))?;
        let text = searchable(m);
        // Folding can lengthen a char (`İ` becomes two), so map the match in the folded text
        // back to the original char it came from.
        let mut lower = String::with_capacity(text.len());
        let mut origin = Vec::with_capacity(text.len());
        for (index, ch) in text.chars().enumerate() {
            lower.extend(fold_char(ch));
            origin.resize(lower.len(), index);
        }
        let at = origin[lower.find(term.as_str())?];
        let text: String =
            text.chars().skip(at.saturating_sub(SNIPPET_CHARS / 2)).take(SNIPPET_CHARS).collect();
        Some(format!("(match) …{}…\n", one_line(&text, SNIPPET_CHARS)))
    }

    /// The footer of a filtered page: how many messages matched and where the next match is.
    /// `kept` flags the messages the filter keeps; on a `complete` transcript the counts are
    /// exact, on a partial one (read up to the first match past the page) only that more follow.
    fn footer(
        &self,
        out: &mut String,
        kept: &[bool],
        complete: bool,
        page: Range<usize>,
        n: usize,
    ) {
        let describe = self.describe();
        let Range { start, end } = page;
        let Some(last) = kept.len().checked_sub(1) else {
            let _ = writeln!(out, "[The session has no messages.]");
            return;
        };
        let span = format!("the session runs #0–#{last}");
        if n == 0 {
            let _ = writeln!(out, "[No messages from #{start} on match {describe}; {span}.]");
            return;
        }
        let messages = plural(n, "message", "messages");
        if complete {
            let _ = write!(out, "\n[{n} {messages} matching {describe} from #{start}; {span}.]");
            let more = kept[end..].iter().filter(|kept| **kept).count();
            if more > 0 {
                let matches = plural(more, "match", "matches");
                let _ = write!(out, " {more} more {matches}: read again with start: {end}.");
            }
        } else {
            let _ = write!(
                out,
                "\n[{n} {messages} matching {describe} from #{start}.] More match: read again \
                 with start: {end}."
            );
        }
        let _ = writeln!(
            out,
            " To see one in context, read again without the filter from a few messages before it."
        );
    }

    /// `query "a b", roles user` for the footer.
    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.terms.is_empty() {
            parts.push(format!("query {:?}", self.terms.join(" ")));
        }
        if !self.roles.is_empty() {
            let roles: Vec<&str> = self.roles.iter().map(|r| r.into()).collect();
            parts.push(format!("roles {}", roles.join(", ")));
        }
        parts.join(", ")
    }
}

/// `text` with case folded the same way everywhere a query compares it: char by char, so a
/// match found in the folded text maps back to the original, and with final sigma as plain
/// sigma, which whole-string lowercasing would otherwise choose by position.
fn fold(text: &str) -> String {
    text.chars().flat_map(fold_char).collect()
}

fn fold_char(ch: char) -> impl Iterator<Item = char> {
    ch.to_lowercase().map(|c| {
        if c == 'ς' {
            'σ'
        } else {
            c
        }
    })
}

const fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 {
        one
    } else {
        many
    }
}

/// What a query searches in a message: what was written, tool names, the values of tool
/// arguments (not their JSON keys) and tool output. Not the page's markup, so a word like
/// `error` or `thinking` matches only where it was said.
fn searchable(m: &Message) -> String {
    fn values(value: &Value, out: &mut String) {
        match value {
            Value::String(s) => {
                out.push_str(s);
                out.push('\n');
            }
            Value::Array(items) => items.iter().for_each(|v| values(v, out)),
            Value::Object(map) => map.values().for_each(|v| values(v, out)),
            Value::Number(n) => {
                let _ = writeln!(out, "{n}");
            }
            Value::Bool(b) => {
                let _ = writeln!(out, "{b}");
            }
            Value::Null => {}
        }
    }
    let mut out = String::new();
    for block in &m.content {
        match block {
            Content::Text(text)
            | Content::Reasoning(text)
            | Content::Summary(text)
            | Content::Error(text) => {
                out.push_str(text);
                out.push('\n');
            }
            Content::ToolUse(call) => {
                out.push_str(&call.name);
                out.push('\n');
                values(&call.input, &mut out);
            }
            Content::ToolResult(result) => {
                out.push_str(&result.output_text().unwrap_or_default());
                out.push('\n');
            }
            // The files an edit touched and the lines it changed.
            Content::Patch(patch) => {
                for file in &patch.files {
                    out.push_str(&file.path);
                    out.push('\n');
                    if let Some(to) = &file.moved_to {
                        out.push_str(to);
                        out.push('\n');
                    }
                    for line in file.hunks.iter().flat_map(|hunk| &hunk.lines) {
                        out.push_str(line);
                        out.push('\n');
                    }
                }
            }
            Content::ReasoningSummary { .. } | Content::Other(_) => {}
        }
    }
    out
}

/// A rendered body without the labels and clip markers a page adds to it.
fn without_markup(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    // Clip markers: `[…N more chars]`.
    while let Some(at) = rest.find("[…") {
        out.push_str(&rest[..at]);
        rest = rest[at..].find(']').map_or("", |close| &rest[at + close + 1..]);
    }
    out.push_str(rest);
    for label in LABELS {
        out = out.replace(label, "");
    }
    out
}

impl AtuinAiSessionReadToolCall {
    fn filter(&self) -> Filter {
        Filter::new(self.query.as_deref(), self.roles.as_deref())
    }

    pub(crate) async fn execute(&self, settings: &Settings, caller: &Caller<'_>) -> ToolOutcome {
        let mut client = match connect(settings).await {
            Ok(client) => client,
            Err(outcome) => return outcome,
        };

        let selector = self.session_id.trim();
        let sessions = match super::list_sessions(&mut client, None, None).await {
            Ok(sessions) => sessions,
            Err(e) => return ToolOutcome::Error(format!("Listing AI sessions failed: {e}")),
        };
        // `latest` means the last session a person ran before this one: not the caller's own
        // live session, and not a subagent fragment.
        let sessions = if selector.eq_ignore_ascii_case("latest") {
            let own = caller.own_in(&sessions);
            sessions.into_iter().filter(|s| !is_own(s, own.as_ref()) && !is_subagent(s)).collect()
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

        // A forward page needs the messages up to it plus one it would show beyond (to know more
        // follows), not the rest of a possibly huge transcript; a negative start counts back
        // from the end, so it needs them all.
        let filter = self.filter();
        let keep = |m: &Message| filter.keeps(m);
        let page = usize::try_from(self.start).ok().map(|start| Page {
            start,
            content: self.limit.get() as usize + 1,
            keep: &keep,
        });
        let (session, messages, complete) = match read_session(&mut client, handle, page).await {
            Ok(read) => read,
            Err(e) => return ToolOutcome::Error(format!("Reading the AI session failed: {e}")),
        };
        ToolOutcome::Success(self.render(
            &session,
            &messages,
            complete,
            &filter,
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
        filter: &Filter,
        offset: time::UtcOffset,
    ) -> String {
        let total = messages.len();
        let limit = self.limit.get() as usize;
        let full = self.limit.get() == 1;
        // Decided once per message: a query searches each one's whole text.
        let kept: Vec<bool> = messages.iter().map(|m| filter.keeps(m)).collect();
        // Out-of-range starts (either sign) clamp to the ends of the transcript. A negative start
        // counts back over messages with something to read (that the filter keeps): sessions
        // often end in metadata-only rows (Codex), and `-1` landing on one would show an empty
        // page.
        let magnitude = usize::try_from(self.start.unsigned_abs()).unwrap_or(usize::MAX);
        let start = if self.start < 0 {
            messages
                .iter()
                .enumerate()
                .rev()
                .filter(|(i, _)| kept[*i])
                .nth(magnitude.saturating_sub(1))
                .map_or(0, |(i, _)| i)
        } else {
            magnitude.min(total)
        };

        let mut out = String::new();
        let _ = writeln!(out, "session  {} [{}]", s.handle.session, harness_name(s.handle.harness));
        let _ = writeln!(out, "atuin id {}", s.atuin_id);
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
            let _ = writeln!(out, "parent   {}", parent_line(s, parent));
        }
        for (label, ids) in link_lines(s) {
            let _ = writeln!(out, "{label:<8} {ids}");
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
            let wanted = kept[index];
            if filter.is_active() && !wanted {
                // Not part of the page, so it ends any run of tool calls around it.
                end = index + 1;
                shown += tools.flush(&mut out);
                continue;
            }
            if wanted {
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
                if !full && let Some(snippet) = filter.snippet(message, &body) {
                    out.push_str(&snippet);
                }
                shown += 1;
            }
            // Stop at a message boundary once the page is big enough, whatever `limit` said:
            // the caller pages on with `start`, and a small-context model is not flooded.
            if out.len() >= PAGE_CHARS {
                break;
            }
        }
        shown += tools.flush(&mut out);

        if filter.is_active() {
            filter.footer(&mut out, &kept, complete, start..end, counted);
            return out;
        }

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
                    let _ = writeln!(body, "{TOOL_ERROR_MARK}");
                }
            }
            (Block::Readable, Content::Text(text)) => {
                let _ = writeln!(body, "{}", clip(text.trim(), TEXT_CHARS, full));
            }
            // A compaction summary stands in for the conversation before it, so it is often the
            // best account of what an earlier stretch of a long session did.
            (Block::Readable, Content::Summary(text)) => {
                let _ = writeln!(body, "{SUMMARY_LABEL} {}", clip(text.trim(), TEXT_CHARS, full));
            }
            (Block::Readable, Content::Error(text)) => {
                let text = if full {
                    text.trim().to_owned()
                } else {
                    one_line(text, TOOL_RESULT_CHARS)
                };
                let _ = writeln!(body, "{MODEL_ERROR_LABEL} {text}");
            }
            (Block::Readable, Content::ToolUse(call)) => {
                let _ = writeln!(body, "→ {}: {}", call.name, tool_input(&call.input, full));
            }
            (Block::Readable, Content::ToolResult(result)) => {
                let output = result.output_text().unwrap_or_default();
                let output = output.trim();
                let mark = if result.error {
                    TOOL_ERROR_MARK
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
            // The files a call changed, and with `full` how: the diff is the call's input again,
            // with line numbers and context.
            (Block::Readable, Content::Patch(patch)) => {
                let _ = writeln!(body, "± {}", patch.summary());
                if full {
                    for file in patch.files.iter().filter(|file| !file.hunks.is_empty()) {
                        body.push_str(&file.unified());
                    }
                }
            }
            // Empty (see `classify`).
            (
                Block::Readable,
                Content::Reasoning(_) | Content::ReasoningSummary { .. } | Content::Other(_),
            ) => {}
        }
    }
    if body.is_empty() {
        return None;
    }

    let role = if is_tool_output(m) {
        "tool".to_owned()
    } else {
        message_role(m)
    };
    Some((role, body))
}

/// A message of tool results (and the patches they made) alone is tool output whatever the
/// envelope role: some harnesses model tool output as a user turn.
fn is_tool_output(m: &Message) -> bool {
    !m.content.is_empty()
        && m.content.iter().all(|b| matches!(b, Content::ToolResult(_) | Content::Patch(_)))
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
/// until `content` of them (from `start` on) are ones the page would `keep`.
pub struct Page<'a> {
    start: usize,
    content: usize,
    keep: &'a (dyn Fn(&Message) -> bool + Sync),
}

/// Fetch a session and its messages, decoded into domain types: all of them, or only as many as
/// `page` needs, dropping the stream there so the daemon stops too. The flag says whether the
/// transcript was read to its end.
pub async fn read_session(
    client: &mut AiClient,
    handle: HarnessSession,
    page: Option<Page<'_>>,
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
                if past_start && page.as_ref().is_some_and(|p| (p.keep)(&m)) {
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
        Content::Text(text) | Content::Summary(text) | Content::Error(text) => {
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
        Content::ToolUse(_) | Content::ToolResult(_) | Content::Patch(_) => Block::Readable,
        // Capture keeps only that reasoning happened, which says nothing to a reader, and never
        // stores reasoning text or raw blocks.
        Content::ReasoningSummary { .. } | Content::Reasoning(_) | Content::Other(_) => {
            Block::Empty
        }
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
    use atuin_client::ai_session::{AtuinSessionId, HarnessKind, NativeSessionId, SourceId};
    use atuin_common::harnesstools::session::{
        Change, FilePatch, Hunk, Patch, ToolCallId, ToolResult, ToolUse,
    };
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
        render_on(args, &session(), msgs, true)
    }

    fn render_on(args: Value, s: &Session, msgs: &[Message], complete: bool) -> String {
        let call = call(args);
        call.render(s, msgs, complete, &call.filter(), time::UtcOffset::UTC)
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

    /// The header names a few children and counts the rest, so a session with many subagents
    /// cannot push the header past the page budget.
    #[rstest]
    fn the_header_bounds_the_children_it_names() {
        let id = |n: u128| AtuinSessionId::from(uuid::Uuid::from_u128(n));
        let mut s = session();
        s.child_atuin_ids = (0..1_000).map(id).collect();
        let out = render_on(json!({"session_id": "abc"}), &s, &[text(Role::User, "hi")], true);
        let children = out.lines().find(|l| l.starts_with("children ")).expect(&out);
        assert_eq!(children, format!("children {}, {}, {}, and 997 more", id(0), id(1), id(2)));
        assert!(!out.contains(&id(3).to_string()), "{out}");
        assert!(out.chars().count() < 1_000, "{out}");
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
    fn a_single_message_read_shows_the_whole_error() {
        let long = format!("overloaded\n{}", "detail ".repeat(200));
        let msgs = vec![blocks(Role::Assistant, vec![Content::Error(long)])];
        let page = render(json!({"session_id": "abc"}), &msgs);
        assert!(page.matches("detail").count() < 200, "a page abridges it");
        let one = render(json!({"session_id": "abc", "limit": 1}), &msgs);
        assert!(one.matches("detail").count() == 200, "{one}");
        assert!(one.contains("(model error) overloaded\ndetail"), "{one}");
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
        let out = render_on(json!({"session_id": "abc", "limit": 2}), &session(), &msgs, false);
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

    fn conversation() -> Vec<Message> {
        vec![
            text(Role::User, "how big is the store?"),
            text(Role::Assistant, "Querying the postgres database."),
            empty(),
            blocks(Role::User, vec![Content::ToolResult(ToolResult {
                call: ToolCallId::from("c".to_owned()),
                output: json!("no tables in postgres"),
                error: false,
            })]),
            text(Role::User, "it's called Records!"),
            text(Role::Assistant, "Thanks, querying records."),
            text(Role::User, "and the other one is hub"),
        ]
    }

    #[rstest]
    fn a_query_shows_only_messages_with_every_word() {
        let out = render(json!({"session_id": "abc", "query": "CALLED records"}), &conversation());
        assert!(out.contains("#4 user 00:00\nit's called Records!"), "{out}");
        assert!(!out.contains("#5 ") && !out.contains("#0 "), "{out}");
        assert!(
            out.contains(
                "[1 message matching query \"called records\" from #0; the session runs #0–#6.]"
            ),
            "{out}"
        );
    }

    #[rstest]
    fn roles_skim_what_the_person_said() {
        let out = render(json!({"session_id": "abc", "roles": ["user"]}), &conversation());
        let numbers: Vec<_> = out.lines().filter(|l| l.starts_with('#')).collect();
        assert_eq!(numbers, ["#0 user 00:00", "#4 user 00:00", "#6 user 00:00"], "{out}");

        let tool = render(json!({"session_id": "abc", "roles": ["tool"]}), &conversation());
        assert!(tool.contains("#3 tool"), "tool output is not the user's: {tool}");
    }

    /// The limit and a negative start count matching messages, and the footer says where the
    /// next match is.
    #[rstest]
    fn a_filtered_page_counts_and_pages_by_matches() {
        let msgs = conversation();
        let first = render(json!({"session_id": "abc", "roles": ["user"], "limit": 2}), &msgs);
        assert!(first.contains("1 more match: read again with start: 6."), "{first}");
        let next =
            render(json!({"session_id": "abc", "roles": ["user"], "limit": 2, "start": 6}), &msgs);
        assert!(next.contains("#6 user") && !next.contains("more match"), "{next}");
        let last = render(json!({"session_id": "abc", "roles": ["user"], "start": -2}), &msgs);
        let numbers: Vec<_> = last.lines().filter(|l| l.starts_with('#')).collect();
        assert_eq!(numbers, ["#4 user 00:00", "#6 user 00:00"], "{last}");
    }

    #[rstest]
    fn a_query_matches_tool_output_and_says_when_nothing_does() {
        let out = render(json!({"session_id": "abc", "query": "tables"}), &conversation());
        assert!(out.contains("#3 tool 00:00\n← no tables in postgres"), "{out}");
        let none = render(json!({"session_id": "abc", "query": "planetscale"}), &conversation());
        assert!(
            none.contains(
                "[No messages from #0 on match query \"planetscale\"; the session runs #0–#6.]"
            ),
            "{none}"
        );
    }

    #[rstest]
    fn a_match_cut_from_a_long_message_gets_a_snippet() {
        let long = format!("{} the database is records {}", "x".repeat(3_000), "y".repeat(500));
        let msgs = vec![text(Role::User, &long)];
        let out = render(json!({"session_id": "abc", "query": "records"}), &msgs);
        assert!(out.contains("(match) …"), "{out}");
        assert!(out.contains("the database is records"), "{out}");
    }

    /// A query searches what was said and the values of tool arguments, not JSON keys or the
    /// page's own labels.
    #[rstest]
    #[case::json_key("description")]
    #[case::thinking_label("thinking")]
    #[case::error_mark("error")]
    fn a_query_ignores_markup(#[case] query: &str) {
        let msgs = vec![
            blocks(Role::Assistant, vec![
                Content::Reasoning("pondering".to_owned()),
                Content::ToolUse(ToolUse {
                    id: ToolCallId::from("c".to_owned()),
                    name: "Bash".to_owned(),
                    input: json!({"command": "pscale shell", "description": "open a shell"}),
                }),
            ]),
            blocks(Role::User, vec![Content::ToolResult(ToolResult {
                call: ToolCallId::from("c".to_owned()),
                output: json!("denied"),
                error: true,
            })]),
        ];
        let out = render(json!({"session_id": "abc", "query": query}), &msgs);
        assert!(out.contains("[No messages from #0 on match"), "{out}");
        let value = render(json!({"session_id": "abc", "query": "pscale"}), &msgs);
        assert!(value.contains("#0 assistant"), "argument values are searched: {value}");
    }

    /// A word that appears on the page only in a clip marker still gets a snippet.
    #[rstest]
    fn a_clip_marker_does_not_hide_a_cut_match() {
        let long = format!("{} and more besides", "x".repeat(3_000));
        let out = render(json!({"session_id": "abc", "query": "more"}), &[text(Role::User, &long)]);
        assert!(out.contains("more chars]"), "the message is clipped: {out}");
        assert!(out.contains("(match) …") && out.contains("and more besides"), "{out}");
    }

    /// A query finds the files an edit touched and the lines it changed, and the message
    /// counts as tool output.
    #[rstest]
    #[case::path("database.rs")]
    #[case::changed_line("ranking_query")]
    fn a_query_finds_what_an_edit_changed(#[case] query: &str) {
        let patch = Patch {
            call: ToolCallId::from("c".to_owned()),
            files: vec![FilePatch {
                path: "src/database.rs".to_owned(),
                change: Change::Update,
                moved_to: None,
                hunks: vec![Hunk {
                    old_start: 1,
                    old_lines: 1,
                    new_start: 1,
                    new_lines: 1,
                    lines: vec!["-old".to_owned(), "+pub fn ranking_query()".to_owned()],
                }],
            }],
        };
        let msgs = vec![blocks(Role::User, vec![Content::Patch(patch)])];
        let out = render(json!({"session_id": "abc", "query": query, "roles": ["tool"]}), &msgs);
        assert!(out.contains("#0 tool"), "{out}");
    }

    /// Lowercasing `İ` doubles it, which must not push the snippet's window past the match.
    #[rstest]
    fn a_snippet_finds_the_match_after_chars_that_grow_when_lowercased() {
        let long = format!("{} the database is records", "İ".repeat(3_000));
        let out =
            render(json!({"session_id": "abc", "query": "records"}), &[text(Role::User, &long)]);
        assert!(out.contains("the database is records…"), "{out}");
    }

    /// Whole-string lowercasing makes a word-final `Σ` into `ς`; a query and the snippet must
    /// fold it the same way, whichever sigma was typed.
    #[rstest]
    #[case::upper("ΟΣ")]
    #[case::final_sigma("ος")]
    #[case::medial_sigma("οσ")]
    fn a_greek_match_gets_its_snippet(#[case] query: &str) {
        let long = format!("{} ΛΟΓΟΣ end", "x".repeat(3_000));
        let out = render(json!({"session_id": "abc", "query": query}), &[text(Role::User, &long)]);
        assert!(out.contains("#0 user") && out.contains("ΛΟΓΟΣ end…"), "{out}");
    }

    #[rstest]
    fn a_query_finds_boolean_tool_arguments() {
        let msgs = vec![blocks(Role::Assistant, vec![Content::ToolUse(ToolUse {
            id: ToolCallId::from("c".to_owned()),
            name: "Configure".to_owned(),
            input: json!({"enabled": true}),
        })])];
        let out = render(json!({"session_id": "abc", "query": "true"}), &msgs);
        assert!(out.contains("#0 assistant"), "{out}");
    }

    #[rstest]
    fn a_filtered_read_of_an_empty_session_says_so() {
        let out = render(json!({"session_id": "abc", "roles": ["user"]}), &[]);
        assert!(out.contains("[The session has no messages.]"), "{out}");
        assert!(!out.contains("#0"), "{out}");
    }

    /// A forward filtered page reads only up to the first match past it, so it says more
    /// follow without counting them.
    #[rstest]
    fn a_partial_filtered_page_says_more_match() {
        let out = render_on(
            json!({"session_id": "abc", "roles": ["user"], "limit": 1}),
            &session(),
            &conversation()[..5],
            false,
        );
        assert!(
            out.contains(
                "[1 message matching roles user from #0.] More match: read again with start: 4."
            ),
            "{out}"
        );
    }

    #[rstest]
    #[case::null(json!({"session_id": "abc", "query": null, "roles": null}))]
    #[case::blank(json!({"session_id": "abc", "query": "  ", "roles": []}))]
    fn blank_filters_are_no_filter(#[case] args: Value) {
        let out = render(args, &conversation());
        assert!(out.contains("[messages 0–6 of 7; 1 empty omitted]"), "{out}");
    }

    #[rstest]
    fn rejects_an_unknown_role() {
        let args = json!({"session_id": "abc", "roles": ["robot"]});
        assert!(serde_json::from_value::<AtuinAiSessionReadToolCall>(args).is_err());
    }

    #[rstest]
    fn long_text_is_clipped_unless_full() {
        let long = "é".repeat(2_010);
        let abridged = clip(&long, TEXT_CHARS, false);
        assert!(abridged.ends_with("[…10 more chars]"), "{abridged}");
        assert_eq!(clip(&long, TEXT_CHARS, true), long);
    }
}
