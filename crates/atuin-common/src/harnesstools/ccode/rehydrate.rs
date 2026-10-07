//! Writing a Claude Code session back out from captured messages, as
//! `<projects>/<encoded cwd>/<session>.jsonl`, the transcript `claude --resume <id>` reads.
//!
//! Each captured row becomes one transcript line under its own `uuid` (the row's source id), in
//! the tree `parentUuid` links it into, so re-capturing the file dedups against the synced rows
//! and Claude Code rebuilds the same conversation. What the format (or the capture) can't carry
//! is left out:
//!
//! - **Thinking.** Capture keeps a thinking block only as a marker ([`Content::ReasoningSummary`]):
//!   neither its text nor its `signature` is synced, and the API rejects a thinking block whose
//!   signature is missing or forged. Every reasoning block is dropped instead, the API's own
//!   recovery for a history whose thinking can't be replayed: it accepts a conversation with every
//!   thinking block removed (each turn's text and tool calls kept), and new turns think afresh.
//! - **Server tools** (web search, web fetch, code execution): capture reads their call and
//!   result as an ordinary tool call and result, losing the block types the API needs to replay
//!   them, and a bare `tool_use` with no `tool_result` after it is invalid. Both are dropped
//!   (one captured without its input becomes a note, as below).
//! - **Tool calls captured without their input** (`ai.capture_tools` off, or a policy withheld
//!   it): the API rejects a `tool_use` whose `input` is not an object, so each becomes a note in its turn's
//!   text (`[ran a shell command]`; see [`tool_note`](crate::harnesstools::note::tool_note)) and its `tool_result` is dropped.
//!   The run of assistant lines the dropped results stood between is merged into its first line,
//!   so user and assistant turns still alternate ([`Flatten::Runs`]). Re-captured, that line keeps
//!   its `uuid` (capture already holds it: nothing is pushed), and the lines merged away and the
//!   results are not there to capture again. The line records which synced rows went into it
//!   ([`MERGED_FIELD`](crate::harnesstools::rehydrate::MERGED_FIELD)), as does the line a row
//!   with nothing to write hangs from, so the transcript says which synced rows it holds.
//! - **Tool output**: written back as captured. A call kept with its input but not its output
//!   (withheld by a policy, or over the size limit) gets [`UNCAPTURED_OUTPUT`] as its
//!   `tool_result` content, which says so to the model. An edit's or write's result gets back
//!   the record Claude Code shows its diff from (`toolUseResult.structuredPatch`) where capture
//!   kept the patch, without the file as it was before, which is never synced.
//! - **Pasted images and documents**: capture keeps none of them, so they are not written back.
//! - **Empty lines**: rows with nothing left to write (attachments, hook records, turn timings)
//!   are skipped, and lines under them are linked to their nearest written ancestor.
//! - **Line kinds**: harness-written user-role text (captured as [`Role::System`]) is written
//!   back as an `isMeta` user line, which Claude Code sends to the model without showing it. The
//!   title is written once, as an `ai-title` line.
//! - **Directories**: each line's `cwd` is moved from the original directory to the one the
//!   session resumes in, where that exists.
//!
//! A [fork](crate::harnesstools::fork) is written the same way under its own id, every line
//! naming the line it copies in `forkedFrom`, as `/branch` writes it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use time::OffsetDateTime;
use time::macros::format_description;

use super::session::{default_root, locate};
use crate::harnesstools::rehydrate::{
    Flatten, RehydrateError, RehydrateMessage, RehydrateSession, UNCAPTURED_OUTPUT,
    break_line_cycles, flatten_uncaptured_calls, on_parent_cycles, record_merged,
};
use crate::harnesstools::resume;
use crate::harnesstools::session::{
    Change, Content, Patch, Role, StopReason, ToolResult, ToolUse, Usage,
};

/// The longest project directory name Claude Code writes before shortening it with a hash
/// (`Kne` in CC 2.1.283).
const MAX_PROJECT_NAME: usize = 200;

/// Write `session` under Claude Code's projects directory (`$CLAUDE_CONFIG_DIR/projects`, else
/// `~/.claude/projects`, where [`Harness::locate`] looks), in the project of the directory it
/// resumes in.
///
/// [`Harness::locate`]: crate::harnesstools::Harness::locate
pub async fn rehydrate(session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
    let (root, session) = (default_root(), session.clone());
    tokio::task::spawn_blocking(move || rehydrate_into(&root, &session))
        .await
        .map_err(|e| RehydrateError::Other(e.to_string()))?
}

/// [`rehydrate`] under the projects directory `root`.
pub(crate) fn rehydrate_into(
    root: &Path,
    session: &RehydrateSession,
) -> Result<PathBuf, RehydrateError> {
    if !resume::is_plain_name(&session.id) {
        return Err(RehydrateError::Other(format!("not a session id: {:?}", session.id)));
    }
    // `claude --resume <id>` gives up when more than one project holds the id.
    if let Some(existing) = locate(root, &session.id) {
        return Err(RehydrateError::AlreadyExists(existing));
    }
    let dir = root.join(project_dir_name(&session.cwd));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.jsonl", session.id));
    crate::fs::write_new(&path, transcript(session).as_bytes()).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            RehydrateError::AlreadyExists(path.clone())
        } else {
            e.into()
        }
    })?;
    Ok(path)
}

/// The project directory Claude Code keeps a directory's sessions in (`qx` in CC 2.1.283): every
/// character but an ASCII letter or digit replaced by `-` (per UTF-16 unit, as JavaScript's
/// `replace` does), and a name longer than 200 cut short and suffixed with a hash of the path.
fn project_dir_name(cwd: &Path) -> String {
    // As `process.cwd()` gives it: without a trailing separator.
    let cwd: PathBuf = cwd.components().collect();
    let raw = cwd.to_string_lossy();
    let mut name = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c);
        } else {
            name.extend(std::iter::repeat_n('-', c.len_utf16()));
        }
    }
    if name.len() <= MAX_PROJECT_NAME {
        return name;
    }
    name.truncate(MAX_PROJECT_NAME);
    format!("{name}-{}", base36(js_string_hash(&raw).unsigned_abs()))
}

/// Java's `String.hashCode` over UTF-16 units, the hash Claude Code shortens long names with.
fn js_string_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, unit| h.wrapping_shl(5).wrapping_sub(h).wrapping_add(i32::from(unit)))
}

fn base36(mut n: u32) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// A timestamp as Claude Code writes one: UTC, to the millisecond.
fn timestamp(at: OffsetDateTime) -> String {
    let format =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");
    at.to_offset(time::UtcOffset::UTC).format(&format).unwrap_or_default()
}

/// The whole transcript, one JSON line per written row.
pub(crate) fn transcript(session: &RehydrateSession) -> String {
    let mut out = String::new();
    for line in lines(session, None) {
        out.push_str(&line.to_string());
        out.push('\n');
    }
    if let Some(title) = session.title.as_deref().filter(|t| !t.trim().is_empty()) {
        let line = json!({"type": "ai-title", "aiTitle": title, "sessionId": session.id});
        out.push_str(&line.to_string());
        out.push('\n');
    }
    out
}

/// The lines for `session`'s rows, calls captured without their input flattened into notes
/// ([`Flatten::Runs`]): for a whole transcript, or for rows appended to one ending on the line
/// `after`, which the first of them then hangs from. The rows written as no line of their own are
/// recorded on the lines they went into ([`MERGED_FIELD`](crate::harnesstools::rehydrate::MERGED_FIELD)).
pub(crate) fn lines(session: &RehydrateSession, after: Option<&str>) -> Vec<Value> {
    let session = &RehydrateSession {
        messages: flatten_uncaptured_calls(&session.messages, &Flatten::Runs),
        ..session.clone()
    };
    let mut writer = Writer::new(session, after);
    for message in &session.messages {
        writer.push(message);
    }
    let mut lines = writer.lines;
    break_line_cycles(&mut lines, "uuid", &["parentUuid", "logicalParentUuid"]);
    record_merged(&mut lines, &writer.merged, |l| l["uuid"].as_str().map(str::to_owned));
    lines
}

struct Writer<'a> {
    session: &'a RehydrateSession,
    lines: Vec<Value>,
    /// Every row, by source id.
    by_id: HashMap<&'a str, &'a RehydrateMessage>,
    /// The rows that are written (the rest have nothing a line can carry).
    writable: HashSet<&'a str>,
    /// The rows written so far.
    written: HashSet<&'a str>,
    /// The rows on a parent cycle.
    on_cycle: HashSet<&'a str>,
    /// The last row written.
    last: Option<&'a str>,
    /// Compaction boundaries: the rows a compaction summary hangs from.
    boundaries: HashSet<&'a str>,
    /// The rows written as no line, each with the line it went into.
    merged: Vec<(String, String)>,
    /// Calls of tools the API ran itself: their results came back on an assistant line.
    server_tools: HashSet<&'a str>,
    /// Every call, by id: a result's record of an edit says what the edit was.
    calls: HashMap<&'a str, &'a ToolUse>,
}

impl<'a> Writer<'a> {
    fn new(session: &'a RehydrateSession, after: Option<&'a str>) -> Self {
        let by_id: HashMap<&'a str, &'a RehydrateMessage> =
            session.messages.iter().map(|m| (m.source_id.as_str(), m)).collect();
        let mut boundaries = HashSet::new();
        for summary in session
            .messages
            .iter()
            .filter(|m| m.role == Role::System && m.content.iter().any(is_summary))
        {
            // Newer Claude Code writes attachments between the boundary and its summary. Capture
            // drops the boundary's text, so past them a system line with any is something else.
            let mut at = summary.parent_source_id.as_deref();
            let mut past_attachments = false;
            for _ in 0..by_id.len() {
                let Some(row) = at.and_then(|p| by_id.get(p)) else {
                    break;
                };
                match &row.role {
                    Role::Other(_) => {
                        at = row.parent_source_id.as_deref();
                        past_attachments = true;
                    }
                    Role::System
                        if !row.content.iter().any(is_summary)
                            && (!past_attachments || row.content.is_empty()) =>
                    {
                        boundaries.insert(row.source_id.as_str());
                        break;
                    }
                    _ => break,
                }
            }
        }

        let server_tools = session
            .messages
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .flat_map(|m| &m.content)
            .filter_map(|c| match c {
                Content::ToolResult(r) => Some(r.call.as_ref()),
                _ => None,
            })
            .collect();
        let calls = session
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|c| match c {
                Content::ToolUse(u) => Some((u.id.as_ref(), u)),
                _ => None,
            })
            .collect();
        let mut writer = Self {
            session,
            lines: Vec::new(),
            on_cycle: on_parent_cycles(&by_id),
            by_id,
            writable: HashSet::new(),
            written: HashSet::new(),
            last: after,
            boundaries,
            server_tools,
            calls,
            merged: Vec::new(),
        };
        writer.writable = session
            .messages
            .iter()
            .filter(|m| writer.line(m, None).is_some())
            .map(|m| m.source_id.as_str())
            .collect();
        writer
    }

    /// The line `m` hangs from: its nearest written ancestor, however the rows are ordered;
    /// `None` at the root. A parent that was never synced stands for the line before, and so
    /// does a parent cycle (a corrupt transcript's) holding no line written yet.
    fn parent(&self, m: &'a RehydrateMessage) -> Option<&'a str> {
        let candidates = if self.on_cycle.contains(m.source_id.as_str()) {
            &self.written
        } else {
            &self.writable
        };
        let mut parent = m.parent_source_id.as_deref();

        // At most one step per row, and never back to `m`.
        for _ in 0..=self.by_id.len() {
            let p = parent?;
            if p == m.source_id {
                break;
            }

            if candidates.contains(p) {
                return Some(p);
            }

            match self.by_id.get(p) {
                Some(row) => parent = row.parent_source_id.as_deref(),
                None => break,
            }
        }

        self.last
    }

    fn push(&mut self, m: &'a RehydrateMessage) {
        let parent = self.parent(m);
        let Some(mut line) = self.line(m, parent) else {
            // Before any line (settings entries at the start): merged into none.
            self.merged.push((m.source_id.clone(), parent.unwrap_or_default().to_owned()));
            return;
        };
        let session = self.session;
        let fields = line.as_object_mut().expect("a line is an object");
        if !fields.contains_key("parentUuid") {
            fields.insert("parentUuid".to_owned(), json!(parent));
        }
        fields.insert("isSidechain".to_owned(), json!(false));
        fields.insert("userType".to_owned(), json!("external"));
        fields.insert("cwd".to_owned(), json!(self.cwd(m)));
        fields.insert("sessionId".to_owned(), json!(session.id));
        if let Some(branch) = m.git_branch.as_ref().or(session.git_branch.as_ref()) {
            fields.insert("gitBranch".to_owned(), json!(branch));
        }
        fields.insert("uuid".to_owned(), json!(m.source_id));
        fields.insert("timestamp".to_owned(), json!(timestamp(m.timestamp)));
        // A fork's lines are the original's under its own session, as `/branch` copies them.
        if let Some(of) = &session.fork_of {
            let from = json!({"sessionId": of.id, "messageUuid": m.source_id});
            fields.insert("forkedFrom".to_owned(), from);
        }
        self.lines.push(line);
        self.last = Some(&m.source_id);
        self.written.insert(&m.source_id);
    }

    /// The directory a line ran in, moved from the original directory to the resumed one: a
    /// subdirectory keeps its place when it exists here. Always without a trailing separator, as
    /// Claude Code writes it (`Path::join("")` would add one for the directory itself).
    fn cwd(&self, m: &RehydrateMessage) -> PathBuf {
        let session = self.session;
        let cwd = m
            .cwd
            .as_deref()
            .zip(session.original_cwd.as_deref())
            .and_then(|(cwd, original)| cwd.strip_prefix(original).ok())
            .map(|rest| session.cwd.join(rest))
            .filter(|cwd| cwd.is_dir())
            .unwrap_or_else(|| session.cwd.clone());
        cwd.components().collect()
    }

    /// The line for `m` without its common fields, or `None` when nothing of it can be written.
    fn line(&self, m: &RehydrateMessage, parent: Option<&str>) -> Option<Value> {
        match &m.role {
            Role::User => user_line(&m.content),
            Role::Tool => tool_line(&m.content, &self.calls),
            Role::System if self.boundaries.contains(m.source_id.as_str()) => Some(json!({
                "parentUuid": null,
                "logicalParentUuid": parent,
                "type": "system",
                "subtype": "compact_boundary",
                "content": "Conversation compacted",
                "isMeta": false,
                "level": "info",
                "compactMetadata": {"trigger": "manual", "preTokens": 0},
            })),
            Role::System => system_line(&m.content),
            Role::Assistant => self.assistant_line(m),
            Role::Other(_) => None,
        }
    }

    fn assistant_line(&self, m: &RehydrateMessage) -> Option<Value> {
        let id = m.turn_id.clone().unwrap_or_else(|| format!("msg_atuin_{}", m.source_id));
        let has_reply = m.content.iter().any(|c| match c {
            Content::Text(t) | Content::Summary(t) => !t.is_empty(),
            Content::ToolUse(_) => true,
            _ => false,
        });
        if !has_reply {
            // A failed call Claude Code recorded as its own synthetic reply.
            let error = m.content.iter().find_map(|c| match c {
                Content::Error(e) => Some(e.as_str()),
                _ => None,
            })?;
            return Some(json!({
                "type": "assistant",
                "isApiErrorMessage": true,
                "message": {
                    "id": id,
                    "type": "message",
                    "role": "assistant",
                    "model": "<synthetic>",
                    "content": [{"type": "text", "text": error}],
                    "stop_reason": "stop_sequence",
                    "stop_sequence": "",
                },
            }));
        }
        let blocks: Vec<Value> = m
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Text(t) | Content::Summary(t) if !t.is_empty() => {
                    Some(json!({"type": "text", "text": t}))
                }
                Content::ToolUse(u) if !self.server_tools.contains(u.id.as_ref()) => Some(json!({
                    "type": "tool_use",
                    "id": u.id,
                    "name": u.name,
                    "input": u.input,
                })),
                _ => None,
            })
            .collect();
        if blocks.is_empty() {
            return None;
        }
        let mut message = json!({"id": id, "type": "message", "role": "assistant"});
        let fields = message.as_object_mut().expect("an object");
        if let Some(model) = &m.model {
            fields.insert("model".to_owned(), json!(model));
        }
        fields.insert("content".to_owned(), Value::Array(blocks));
        fields
            .insert("stop_reason".to_owned(), json!(m.stop_reason.as_ref().and_then(stop_reason)));
        fields.insert("stop_sequence".to_owned(), Value::Null);
        if let Some(usage) = &m.usage {
            fields.insert("usage".to_owned(), usage_json(usage));
        }
        Some(json!({"type": "assistant", "message": message}))
    }
}

fn is_summary(c: &Content) -> bool {
    matches!(c, Content::Summary(_))
}

fn joined_text(content: &[Content]) -> Option<String> {
    let texts: Vec<&str> = content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) | Content::Summary(t) if !t.trim().is_empty() => Some(t.as_str()),
            _ => None,
        })
        .collect();
    (!texts.is_empty()).then(|| texts.join("\n"))
}

/// A user-role text block for `c`, if it is text.
fn user_block(c: &Content) -> Option<Value> {
    match c {
        Content::Text(t) | Content::Summary(t) if !t.is_empty() => {
            Some(json!({"type": "text", "text": t}))
        }
        _ => None,
    }
}

fn tool_result_block(r: &ToolResult) -> Value {
    let mut block = json!({"type": "tool_result", "tool_use_id": r.call});
    block["content"] = if r.output.is_null() {
        json!(UNCAPTURED_OUTPUT)
    } else {
        r.output.clone()
    };
    block["is_error"] = json!(r.error);
    block
}

/// A prompt: its text as a plain string, like Claude Code writes one, or blocks. A tool result
/// on a user's line is a rejected call: its text is what the user said when rejecting it.
fn user_line(content: &[Content]) -> Option<Value> {
    let results: Vec<Value> = content
        .iter()
        .filter_map(|c| match c {
            Content::ToolResult(r) => Some(tool_result_block(r)),
            _ => None,
        })
        .collect();
    if !results.is_empty() {
        let mut line = json!({
            "type": "user",
            "message": {"role": "user", "content": results},
            "toolUseResult": "",
        });
        if let Some(feedback) = joined_text(content) {
            line["userFeedback"] = json!(feedback);
        }
        return Some(line);
    }
    let blocks: Vec<Value> = content.iter().filter_map(user_block).collect();
    let content = match blocks.as_slice() {
        [] => return None,
        [only] if only["type"] == "text" => only["text"].clone(),
        _ => Value::Array(blocks),
    };
    Some(json!({"type": "user", "message": {"role": "user", "content": content}}))
}

/// Tool results, sent in the user's turn with the tool's own record of them (`toolUseResult`):
/// an edit's ([`edit_record`]) where capture kept its patch, else the result's text.
fn tool_line(content: &[Content], calls: &HashMap<&str, &ToolUse>) -> Option<Value> {
    let blocks: Vec<Value> = content
        .iter()
        .filter_map(|c| match c {
            Content::ToolResult(r) => Some(tool_result_block(r)),
            _ => None,
        })
        .collect();
    if blocks.is_empty() {
        return None;
    }
    let edit = content.iter().find_map(|c| match c {
        Content::Patch(patch) => edit_record(patch, calls.get(patch.call.as_ref()).copied()),
        _ => None,
    });
    let record = edit.unwrap_or_else(|| {
        let text = content.iter().find_map(|c| match c {
            Content::ToolResult(r) => r.output_text().map(|t| t.into_owned()),
            _ => None,
        });
        json!(text.unwrap_or_default())
    });
    Some(json!({
        "type": "user",
        "message": {"role": "user", "content": blocks},
        "toolUseResult": record,
    }))
}

/// The record Claude Code keeps of an edit's (or older `MultiEdit`'s) or write's result, from its
/// patch and its `call`:
/// what it shows the change with, on resume and when rewinding. The file as it was before is not
/// synced (`originalFile` is null, as Claude Code writes it for a file too large to keep).
fn edit_record(patch: &Patch, call: Option<&ToolUse>) -> Option<Value> {
    let file = patch.files.first()?;
    let hunks: Vec<Value> = file
        .hunks
        .iter()
        .map(|h| {
            json!({
                "oldStart": h.old_start,
                "oldLines": h.old_lines,
                "newStart": h.new_start,
                "newLines": h.new_lines,
                "lines": h.lines,
            })
        })
        .collect();
    let input = call.map_or(&Value::Null, |c| &c.input);
    let written = call.is_some_and(|c| c.name == "Write") || file.change == Change::Add;
    // Older Claude Code's `MultiEdit`, several edits of one file.
    if call.is_some_and(|c| c.name == "MultiEdit") {
        return Some(json!({
            "filePath": file.path,
            "edits": input["edits"],
            "originalFileContents": null,
            "structuredPatch": hunks,
            "userModified": false,
        }));
    }
    Some(if written {
        let kind = if file.change == Change::Add {
            "create"
        } else {
            "update"
        };
        json!({
            "type": kind,
            "filePath": file.path,
            "content": input["content"].as_str().unwrap_or_default(),
            "structuredPatch": hunks,
            "originalFile": null,
        })
    } else {
        json!({
            "filePath": file.path,
            "oldString": input["old_string"].as_str().unwrap_or_default(),
            "newString": input["new_string"].as_str().unwrap_or_default(),
            "originalFile": null,
            "structuredPatch": hunks,
            "userModified": false,
            "replaceAll": input["replace_all"].as_bool().unwrap_or(false),
        })
    })
}

/// A compaction summary, or text the harness put in the user's turn itself (`isMeta`).
fn system_line(content: &[Content]) -> Option<Value> {
    if let Some(summary) = content.iter().find_map(|c| match c {
        Content::Summary(s) if !s.trim().is_empty() => Some(s),
        _ => None,
    }) {
        return Some(json!({
            "type": "user",
            "message": {"role": "user", "content": summary},
            "isCompactSummary": true,
            "isVisibleInTranscriptOnly": true,
        }));
    }
    let text = joined_text(content)?;
    Some(json!({"type": "user", "message": {"role": "user", "content": text}, "isMeta": true}))
}

/// The API's name for a stop reason; `None` for those that aren't one (an abort, an error).
fn stop_reason(reason: &StopReason) -> Option<String> {
    Some(
        match reason {
            StopReason::EndTurn => "end_turn",
            StopReason::MaxTokens => "max_tokens",
            StopReason::ToolUse => "tool_use",
            StopReason::StopSequence => "stop_sequence",
            StopReason::Refusal => "refusal",
            StopReason::Other(other) => other.as_str(),
            StopReason::Aborted | StopReason::Error => return None,
        }
        .to_owned(),
    )
}

fn usage_json(usage: &Usage) -> Value {
    let mut out = serde_json::Map::new();
    let mut put = |key: &str, n: Option<u64>| {
        if let Some(n) = n {
            out.insert(key.to_owned(), json!(n));
        }
    };
    put("input_tokens", usage.input);
    put("cache_creation_input_tokens", usage.cache_write);
    put("cache_read_input_tokens", usage.cache_read);
    put("output_tokens", usage.output);
    if let Some(thinking) = usage.reasoning {
        out.insert("output_tokens_details".to_owned(), json!({"thinking_tokens": thinking}));
    }
    Value::Object(out)
}

#[cfg(test)]
pub(crate) mod tests;
