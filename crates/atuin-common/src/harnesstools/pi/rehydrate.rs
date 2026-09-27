//! Writing a pi session back out from captured messages, as
//! `<session dir>/<started>_<session>.jsonl`: a `session` header naming the directory it resumes
//! in, then one entry per captured row under the row's own id, `parentId`-linked into the same
//! tree, so re-capturing the file dedups against the synced rows and pi rebuilds the same
//! context (pi-mono coding-agent `session-manager.ts`).
//!
//! What the format (or the capture) can't carry is left out:
//!
//! - **Thinking.** Capture keeps only a marker ([`Content::ReasoningSummary`]), no text or
//!   signature, so it is dropped; pi itself drops a thinking block it can't replay.
//! - **Pasted images**: capture keeps their type, not their bytes; each becomes a text
//!   placeholder saying so.
//! - **Compactions** become `compaction` entries keeping every entry since the one before
//!   (capture doesn't record which entries pi kept): the model sees the summary and the full
//!   history since the previous compaction. A branch summary is written the same way.
//! - **Settings entries** (model and thinking-level changes, labels, extension state) carry no
//!   content and are skipped, so pi starts on its configured thinking level; the model is named
//!   by each assistant turn, its provider guessed from the model's name, and pi falls back to its
//!   default model when it can't use it. A system-role text entry is skipped too.
//! - **Tool results** keep their output; their tool's name comes from the call they answer.
//! - A `!command` keeps its command, output and whether it failed, not its exit code; one from a
//!   v1 file (no entry ids) has its result renamed after the id it is written under.
//! - The title is written as a `session_info` entry, under the id of the row that set it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use time::OffsetDateTime;
use time::macros::format_description;

use super::session::{default_root, locate, new_session_dir};
use crate::harnesstools::rehydrate::{RehydrateError, RehydrateMessage, RehydrateSession};
use crate::harnesstools::resume;
use crate::harnesstools::session::{Content, Role, StopReason, ToolResult, Usage};

/// The session file format version this writes (session-manager.ts `CURRENT_SESSION_VERSION`).
const VERSION: u32 = 3;

/// Write `session` where pi keeps new sessions for the directory it resumes in, after checking
/// every session directory [`Harness::locate`] searches for one already holding it.
///
/// [`Harness::locate`]: crate::harnesstools::Harness::locate
pub async fn rehydrate(session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
    let session = session.clone();
    tokio::task::spawn_blocking(move || {
        rehydrate_into(&default_root(), &new_session_dir(&session.cwd), &session)
    })
    .await
    .map_err(|e| RehydrateError::Other(e.to_string()))?
}

/// [`rehydrate`], looking for an existing copy under `root` and writing into `dir`.
pub(crate) fn rehydrate_into(
    root: &Path,
    dir: &Path,
    session: &RehydrateSession,
) -> Result<PathBuf, RehydrateError> {
    if !resume::is_plain_name(&session.id) {
        return Err(RehydrateError::Other(format!("not a session id: {:?}", session.id)));
    }
    if let Some(existing) = locate(root, &session.id) {
        return Err(RehydrateError::AlreadyExists(existing));
    }
    std::fs::create_dir_all(dir)?;
    // pi's own name: the start time with `:` and `.` made file-safe, then the id.
    let started = timestamp(session.started_at).replace([':', '.'], "-");
    let path = dir.join(format!("{started}_{}.jsonl", session.id));
    crate::fs::write_new(&path, transcript(session).as_bytes()).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            RehydrateError::AlreadyExists(path.clone())
        } else {
            e.into()
        }
    })?;
    Ok(path)
}

/// A timestamp as pi writes one (`Date.toISOString`): UTC, to the millisecond.
fn timestamp(at: OffsetDateTime) -> String {
    let format =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");
    at.to_offset(time::UtcOffset::UTC).format(&format).unwrap_or_default()
}

/// Milliseconds since the epoch, as pi times its messages.
fn millis(at: OffsetDateTime) -> i64 {
    i64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap_or_default()
}

fn transcript(session: &RehydrateSession) -> String {
    let header = json!({
        "type": "session",
        "version": VERSION,
        "id": session.id,
        "timestamp": timestamp(session.started_at),
        "cwd": session.cwd,
    });
    let mut writer = Writer::new(session);
    for message in &session.messages {
        writer.push(message);
    }
    writer.title();
    let mut out = header.to_string();
    out.push('\n');
    for line in writer.lines {
        out.push_str(&line.to_string());
        out.push('\n');
    }
    out
}

struct Writer<'a> {
    session: &'a RehydrateSession,
    lines: Vec<Value>,
    written: HashSet<&'a str>,
    /// Rows skipped, each with the written entry standing in for it as a parent.
    skipped: HashMap<&'a str, Option<&'a str>>,
    last: Option<&'a str>,
    /// The first entry since the last compaction written: the next compaction keeps from it.
    kept_from: Option<&'a str>,
    /// Each tool call's name, by call id.
    tools: HashMap<&'a str, &'a str>,
}

impl<'a> Writer<'a> {
    fn new(session: &'a RehydrateSession) -> Self {
        let tools = session
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|c| match c {
                Content::ToolUse(u) => Some((u.id.as_ref(), u.name.as_str())),
                _ => None,
            })
            .collect();
        Self {
            session,
            lines: Vec::new(),
            written: HashSet::new(),
            skipped: HashMap::new(),
            last: None,
            kept_from: None,
            tools,
        }
    }

    fn parent(&self, m: &'a RehydrateMessage) -> Option<&'a str> {
        match m.parent_source_id.as_deref() {
            Some(p) if self.written.contains(p) => Some(p),
            Some(p) if self.skipped.contains_key(p) => self.skipped[p],
            _ => self.last,
        }
    }

    fn push(&mut self, m: &'a RehydrateMessage) {
        let parent = self.parent(m);
        let Some(mut entry) = self.entry(m) else {
            self.skipped.insert(&m.source_id, parent);
            return;
        };
        let compaction = entry["type"] == "compaction";
        let fields = entry.as_object_mut().expect("an entry is an object");
        let kind = fields.remove("type").unwrap_or_default();
        let mut line = serde_json::Map::new();
        line.insert("type".to_owned(), kind);
        line.insert("id".to_owned(), json!(m.source_id));
        line.insert("parentId".to_owned(), json!(parent));
        line.insert("timestamp".to_owned(), json!(timestamp(m.timestamp)));
        line.append(fields);
        self.lines.push(Value::Object(line));
        self.written.insert(&m.source_id);
        self.last = Some(&m.source_id);
        if compaction {
            self.kept_from = None;
        } else if self.kept_from.is_none() {
            self.kept_from = Some(&m.source_id);
        }
    }

    /// The session's title, under the id of the last row that named it (so a re-capture
    /// dedups), else a new one.
    fn title(&mut self) {
        let session = self.session;
        let Some(title) = session.title.as_deref().filter(|t| !t.trim().is_empty()) else {
            return;
        };
        let named = session
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Other("session_info".to_owned()))
            .filter(|m| !self.written.contains(m.source_id.as_str()));
        let (id, at) = named.map_or_else(
            || {
                (
                    format!("atuin-{}", crate::utils::uuid_v7().as_simple()),
                    OffsetDateTime::now_utc(),
                )
            },
            |m| (m.source_id.clone(), m.timestamp),
        );
        self.lines.push(json!({
            "type": "session_info",
            "id": id,
            "parentId": self.last,
            "timestamp": timestamp(at),
            "name": title,
        }));
    }

    /// The entry for `m`, without its tree fields; `None` when nothing of it can be written.
    fn entry(&self, m: &RehydrateMessage) -> Option<Value> {
        let at = millis(m.timestamp);
        match &m.role {
            Role::User => {
                if let Some(bash) = bash_execution(m, at) {
                    return Some(bash);
                }
                let content = user_blocks(&m.content);
                (!content.is_empty()).then(|| {
                    json!({
                        "type": "message",
                        "message": {"role": "user", "content": content, "timestamp": at},
                    })
                })
            }
            Role::Assistant => assistant_entry(m, at),
            Role::Tool => {
                let result = m.content.iter().find_map(|c| match c {
                    Content::ToolResult(r) => Some(r),
                    _ => None,
                })?;
                let name = self.tools.get(result.call.as_ref()).copied().unwrap_or_default();
                Some(json!({
                    "type": "message",
                    "message": {
                        "role": "toolResult",
                        "toolCallId": result.call,
                        "toolName": name,
                        "content": if result.output.is_null() { json!([]) } else { result.output.clone() },
                        "isError": result.error,
                        "timestamp": at,
                    },
                }))
            }
            Role::System => {
                let summary = m.content.iter().find_map(|c| match c {
                    Content::Summary(s) if !s.is_empty() => Some(s),
                    _ => None,
                })?;
                let mut entry = json!({
                    "type": "compaction",
                    "summary": summary,
                    "firstKeptEntryId": self.kept_from,
                    "tokensBefore": 0,
                });
                if let Some(usage) = &m.usage {
                    entry["usage"] = usage_json(usage);
                }
                Some(entry)
            }
            Role::Other(kind) if kind == "custom" => {
                let content = user_blocks(&m.content);
                (!content.is_empty()).then(|| {
                    json!({
                        "type": "custom_message",
                        "customType": "atuin-restored",
                        "content": content,
                        "display": true,
                    })
                })
            }
            Role::Other(_) => None,
        }
    }
}

/// Text blocks for user-role content, with a placeholder for media whose bytes were never
/// captured.
fn user_blocks(content: &[Content]) -> Vec<Value> {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) | Content::Summary(t) if !t.is_empty() => {
                Some(json!({"type": "text", "text": t}))
            }
            Content::Other(raw) if raw["type"] == "image" => {
                Some(json!({"type": "text", "text": "[image not restored]"}))
            }
            _ => None,
        })
        .collect()
}

/// A `!command` the user ran in pi's shell: captured as its command line and a result named
/// after the entry (a v1 entry, which had no id, by its time: it is named after its id now).
fn bash_execution(m: &RehydrateMessage, at: i64) -> Option<Value> {
    let [
        Content::Text(line),
        Content::ToolResult(ToolResult {
            call,
            output,
            error,
        }),
    ] = m.content.as_slice()
    else {
        return None;
    };
    if call.as_ref() != m.source_id && !call.as_ref().starts_with("bash:") {
        return None;
    }
    let (command, hidden) = match line.strip_prefix("!!") {
        Some(command) => (command, true),
        None => (line.strip_prefix('!')?, false),
    };
    Some(json!({
        "type": "message",
        "message": {
            "role": "bashExecution",
            "command": command,
            "output": output,
            "exitCode": i32::from(*error),
            "cancelled": false,
            "truncated": false,
            "excludeFromContext": hidden,
            "timestamp": at,
        },
    }))
}

fn assistant_entry(m: &RehydrateMessage, at: i64) -> Option<Value> {
    let content: Vec<Value> = m
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) | Content::Summary(t) => Some(json!({"type": "text", "text": t})),
            Content::ToolUse(u) => Some(json!({
                "type": "toolCall",
                "id": u.id,
                "name": u.name,
                "arguments": u.input,
            })),
            _ => None,
        })
        .collect();
    let error = m.content.iter().find_map(|c| match c {
        Content::Error(e) => Some(e.as_str()),
        _ => None,
    });
    if content.is_empty() && error.is_none() {
        return None;
    }
    let mut message = json!({"role": "assistant", "content": content});
    if let Some(provider) = m.model.as_deref().and_then(provider_of) {
        message["provider"] = json!(provider);
    }
    if let Some(model) = &m.model {
        message["model"] = json!(model);
    }
    message["usage"] = usage_json(&m.usage.unwrap_or_default());
    message["stopReason"] = json!(stop_reason(m.stop_reason.as_ref(), error.is_some()));
    message["timestamp"] = json!(at);
    if let Some(error) = error {
        message["errorMessage"] = json!(error);
    }
    Some(json!({"type": "message", "message": message}))
}

/// The provider a model most likely came from, by its name: pi restores a session's model by
/// provider and id, and falls back to its default when that isn't one it can use.
fn provider_of(model: &str) -> Option<&'static str> {
    let model = model.to_ascii_lowercase();
    if model.starts_with("claude") {
        Some("anthropic")
    } else if model.starts_with("gpt") || model.starts_with("codex") || model.starts_with('o') {
        Some("openai")
    } else if model.starts_with("gemini") {
        Some("google")
    } else {
        None
    }
}

/// pi's name for a stop reason (pi-ai `StopReason`).
fn stop_reason(reason: Option<&StopReason>, failed: bool) -> String {
    match reason {
        Some(StopReason::ToolUse) => "toolUse",
        Some(StopReason::MaxTokens) => "length",
        Some(StopReason::Aborted) => "aborted",
        Some(StopReason::Error) => "error",
        Some(StopReason::Other(other)) => other.as_str(),
        _ if failed => "error",
        _ => "stop",
    }
    .to_owned()
}

/// Usage as pi records it (pi-ai `Usage`), costs unknown.
fn usage_json(usage: &Usage) -> Value {
    let n = |v: Option<u64>| v.unwrap_or_default();
    let mut out = json!({
        "input": n(usage.input),
        "output": n(usage.output),
        "cacheRead": n(usage.cache_read),
        "cacheWrite": n(usage.cache_write),
        "totalTokens": n(usage.input) + n(usage.output) + n(usage.cache_read) + n(usage.cache_write),
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0},
    });
    if let Some(reasoning) = usage.reasoning {
        out["reasoning"] = json!(reasoning);
    }
    out
}

#[cfg(test)]
mod tests;
