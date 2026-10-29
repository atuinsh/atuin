//! Writing a Copilot session back out from captured messages, as
//! `<sessions>/<session>/events.jsonl` with its `workspace.yaml` beside it, the log
//! `copilot --resume=<id>` reads. Copilot indexes a session it finds there itself, and appends
//! to the log as the session goes on (verified against Copilot CLI 1.0.104, which has no
//! published format: see the reader's module docs for the events).
//!
//! Each captured row becomes one event under its own `id` (the row's source id), each chained to
//! the event before it as Copilot chains them (`parentId`), so re-capturing the log dedups
//! against the synced rows. A prompt is a `user.message`, a reply an `assistant.message` (its
//! text and its tool calls as `toolRequests`), a result a `tool.execution_complete`, a compaction
//! summary a `session.compaction_complete`, and text the harness put in the user's turn itself
//! (a continuation's marker) a `user.message` the user didn't type (`source: "atuin"`), whose
//! text Copilot gives the model as `transformedContent`, the way it gives its own memory. The
//! usage of a reply is a `session.usage_record` just before it, as Copilot writes one, under the
//! same `apiCallId`. What the format (or the capture) can't carry is left out:
//!
//! - **Reasoning.** Capture keeps it only as a marker; Copilot keeps the model's reasoning in a
//!   form only it can read back (`reasoningOpaque`, `encryptedContent`), and resumes a log
//!   without any (its own, under some models, has none in the clear).
//! - **Tool calls captured without their input** (`threads.capture_tools` off, or a policy
//!   withheld it) become a note in their turn's text ([`tool_note`]), their result dropped, and
//!   the run of assistant rows the results stood between is merged into its last row
//!   ([`Flatten::Runs`]), as the Claude Code writer does. The event records which synced rows
//!   went into it ([`MERGED_FIELD`]), as does the event a row with nothing to write hangs from.
//! - **Tool output** is written back as captured; a call kept with its input but not its output
//!   gets [`UNCAPTURED_OUTPUT`]. An edit's result gets the files it changed (`fileEdits`) and
//!   its diff (`detailedContent`) where capture kept the patch.
//! - **A row of several results** (Claude Code sends a turn's results together) is several
//!   events, the first under the row's id and the rest under ids derived from it: such a row
//!   reads back as several. Rows a writer of its own makes for Copilot (a continuation's) hold
//!   one result each.
//! - **Rows with nothing to write** (a reader's title rows, attachments) are skipped: the title
//!   is the sidecar's `name`.
//! - **A call id too long for the model API** (Copilot sends it on with a prefix, and the API
//!   takes 64 characters; a Codex id is 64) is written as one derived from it, on the call and
//!   its result alike (`TOOL_CALL_ID_MAX`).
//!
//! A [fork](crate::harnesstools::fork) names the session it was forked from in its
//! `session.start`, under `atuin` (Copilot keeps fields it doesn't know), out of the model's
//! sight; the reader links it as the fork it is.
//!
//! [`tool_note`]: crate::harnesstools::note::tool_note
//! [`Flatten::Runs`]: crate::harnesstools::rehydrate::Flatten::Runs
//! [`MERGED_FIELD`]: crate::harnesstools::rehydrate::MERGED_FIELD

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use time::OffsetDateTime;
use time::macros::format_description;
use uuid::Uuid;

use super::session::{EVENTS_FILE, SIDECAR_FILE, default_root, locate};
use crate::harnesstools::rehydrate::{
    RehydrateError, RehydrateMessage, RehydrateSession, UNCAPTURED_OUTPUT, merge_uncaptured_runs,
    record_merged,
};
use crate::harnesstools::resume;
use crate::harnesstools::session::{Change, Content, Patch, Role, ToolResult, Usage};
use crate::path::PathBufExt as _;

/// Who wrote the log, as its `session.start` says (`producer`).
pub(crate) const PRODUCER: &str = "atuin";

/// The Copilot version a `session.start` says wrote the log (`copilotVersion`), which Copilot
/// requires of a log it opens: the oldest the format written here was verified against, as a
/// newer Copilot reads an older one's logs.
pub(crate) const COPILOT_VERSION: &str = "1.0.104";

/// The source of a `user.message` the user didn't type: text atuin put in the user's turn (a
/// continuation's marker), which Copilot gives the model as the message's `transformedContent`.
pub(crate) const ATUIN_SOURCE: &str = "atuin";

/// Write `session` under Copilot's sessions directory (`$COPILOT_HOME/session-state`, else
/// `~/.copilot/session-state`, where [`Harness::locate`] looks).
///
/// [`Harness::locate`]: crate::harnesstools::Harness::locate
pub async fn rehydrate(session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
    let (root, session) = (default_root(), session.clone());
    tokio::task::spawn_blocking(move || rehydrate_into(&root, &session))
        .await
        .map_err(|e| RehydrateError::Other(e.to_string()))?
}

/// [`rehydrate`] under the sessions directory `root`: the log's path.
pub(crate) fn rehydrate_into(
    root: &Path,
    session: &RehydrateSession,
) -> Result<PathBuf, RehydrateError> {
    if !resume::is_plain_name(&session.id) {
        return Err(RehydrateError::Other(format!("not a session id: {:?}", session.id)));
    }
    if let Some(existing) = locate(root, &session.id) {
        return Err(RehydrateError::AlreadyExists(existing));
    }
    let dir = root.join(&session.id);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(EVENTS_FILE);
    crate::fs::write_new(&path, log(session).as_bytes()).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            RehydrateError::AlreadyExists(path.clone())
        } else {
            e.into()
        }
    })?;
    std::fs::write(dir.joined(SIDECAR_FILE), sidecar(session))?;
    Ok(path)
}

/// The whole log: its `session.start`, then one line per written row.
pub(crate) fn log(session: &RehydrateSession) -> String {
    let mut out = String::new();
    for line in lines(session, None) {
        out.push_str(&line.to_string());
        out.push('\n');
    }
    out
}

/// The sidecar beside the log, as Copilot writes one: where the session works and its name.
/// The name is the user's (`user_named`), so Copilot keeps it rather than generating one.
pub(crate) fn sidecar(session: &RehydrateSession) -> String {
    let cwd = yaml_text(&session.cwd.to_string_lossy());
    let at = timestamp(session.started_at);
    let mut out = format!("id: {}\ncwd: {cwd}\ngit_root: {cwd}\n", session.id);
    if let Some(branch) = &session.git_branch {
        out.push_str(&format!("branch: {}\n", yaml_text(branch)));
    }
    out.push_str(&format!("client_name: {PRODUCER}\n"));
    if let Some(title) = session.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        out.push_str(&format!("name: {}\nuser_named: true\n", yaml_text(title)));
    }
    out.push_str(&format!("summary_count: 0\nfork_count: 0\ncreated_at: {at}\nupdated_at: {at}\n"));
    out
}

/// `text` as a YAML scalar: double-quoted, since a name can hold anything.
fn yaml_text(text: &str) -> String {
    let mut out = String::from('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A timestamp as Copilot writes one: UTC, to the millisecond.
fn timestamp(at: OffsetDateTime) -> String {
    let format =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");
    at.to_offset(time::UtcOffset::UTC).format(&format).unwrap_or_default()
}

/// The events for `session`'s rows, calls captured without their input flattened into notes
/// ([`Flatten::Runs`]): for a whole log, opened by its `session.start`; or for rows appended to
/// one whose last event is `after`, which the first of them is then chained to. The rows written
/// as no event of their own are recorded on the events they went into ([`MERGED_FIELD`]).
///
/// [`Flatten::Runs`]: crate::harnesstools::rehydrate::Flatten::Runs
/// [`MERGED_FIELD`]: crate::harnesstools::rehydrate::MERGED_FIELD
pub(crate) fn lines(session: &RehydrateSession, after: Option<&str>) -> Vec<Value> {
    let (messages, hosts) = merge_uncaptured_runs(&session.messages);
    let session = &RehydrateSession {
        messages,
        ..session.clone()
    };
    let mut writer = Writer::new(session, after);
    if after.is_none() {
        writer.start();
    }
    for message in &session.messages {
        writer.push(message);
    }
    for (row, into) in &mut writer.merged {
        if let Some(host) = hosts.get(row) {
            into.clone_from(host);
        }
    }
    let mut lines = writer.lines;
    record_merged(&mut lines, &writer.merged, |l| l["id"].as_str().map(str::to_owned));
    lines
}

struct Writer<'a> {
    session: &'a RehydrateSession,
    lines: Vec<Value>,
    /// The event written last, which the next is chained to.
    last: Option<String>,
    /// The interaction the rows being written are in: Copilot opens one per prompt.
    interaction: String,
    /// The replies since the prompt, which number the interaction's turns.
    turn: u32,
    /// The rows written as no event, each with the event it went into.
    merged: Vec<(String, String)>,
}

impl<'a> Writer<'a> {
    fn new(session: &'a RehydrateSession, after: Option<&str>) -> Self {
        Self {
            session,
            lines: Vec::new(),
            last: after.map(str::to_owned),
            interaction: derived(&session.id, "interaction"),
            turn: 0,
            merged: Vec::new(),
        }
    }

    /// The `session.start` a log opens with: where the session works, and what it was forked
    /// from, if anything.
    fn start(&mut self) {
        let session = self.session;
        let cwd = &session.cwd;
        let mut data = json!({
            "sessionId": session.id,
            "version": 1,
            "producer": PRODUCER,
            "copilotVersion": COPILOT_VERSION,
            "startTime": timestamp(session.started_at),
            "context": {"cwd": cwd, "gitRoot": cwd, "branch": session.git_branch},
        });
        if let Some(of) = &session.fork_of {
            data["atuin"] = json!({"forkedFrom": of.id, "atuinId": of.atuin_id});
        }
        self.event("session.start", derived(&session.id, "start"), session.started_at, data);
    }

    /// Write `kind` with `data` as the event `id`, chained to the last.
    fn event(&mut self, kind: &str, id: String, at: OffsetDateTime, data: Value) {
        let mut line = json!({
            "type": kind,
            "data": Value::Null,
            "id": id,
            "parentId": self.last,
            "timestamp": timestamp(at),
        });
        line["data"] = data;
        self.lines.push(line);
        self.last = Some(id);
    }

    fn push(&mut self, m: &'a RehydrateMessage) {
        let events = self.events(m);
        if events.is_empty() {
            self.merged.push((m.source_id.clone(), self.last.clone().unwrap_or_default()));
            return;
        }
        // The row's id on the event the row reads back as (a reply's usage record goes before
        // it, as Copilot writes them); the rest under ids derived from it.
        let own = events.iter().position(|(kind, _)| *kind != "session.usage_record").unwrap_or(0);
        for (n, (kind, data)) in events.into_iter().enumerate() {
            let id = if n == own {
                m.source_id.clone()
            } else {
                derived(&m.source_id, &n.to_string())
            };
            self.event(kind, id, m.timestamp, data);
        }
    }

    /// The events for `m`, in order: none when nothing of it can be written.
    fn events(&mut self, m: &RehydrateMessage) -> Vec<(&'static str, Value)> {
        let mut out = Vec::new();
        match &m.role {
            Role::User => {
                if let Some(text) = joined_text(&m.content) {
                    self.interaction = derived(&self.session.id, &m.source_id);
                    self.turn = 0;
                    out.push(("user.message", self.prompt(&m.source_id, &text, None)));
                }
                // A result on the user's line: a call the user rejected.
                out.extend(self.results(m));
            }
            Role::Assistant => {
                if let Some((usage, reply)) = self.reply(m) {
                    out.extend(usage.map(|usage| ("session.usage_record", usage)));
                    out.push(("assistant.message", reply));
                }
                // Results of tools the API ran itself, which Claude Code keeps on the reply.
                out.extend(self.results(m));
            }
            Role::Tool => out.extend(self.results(m)),
            Role::System => {
                if let Some(summary) = m.content.iter().find_map(|c| match c {
                    Content::Summary(s) if !s.trim().is_empty() => Some(s),
                    _ => None,
                }) {
                    out.push((
                        "session.compaction_complete",
                        json!({
                            "success": true,
                            "outcome": "applied",
                            "summaryContent": summary,
                        }),
                    ));
                } else if let Some(text) = joined_text(&m.content) {
                    let injected = self.prompt(&m.source_id, "", Some(text));
                    out.push(("user.message", injected));
                }
            }
            Role::Other(_) => {}
        }
        out
    }

    /// A `user.message`: what the user typed as `content`, or what atuin put in the user's
    /// turn as `transformed`, which Copilot gives the model in the user's place.
    fn prompt(&self, source_id: &str, content: &str, transformed: Option<String>) -> Value {
        let mut data = json!({
            "content": content,
            "messageId": derived(source_id, "message"),
            "interactionId": self.interaction,
            "turnId": self.turn.to_string(),
        });
        if let Some(text) = transformed {
            data["transformedContent"] = json!(text);
            data["source"] = json!(ATUIN_SOURCE);
        }
        data
    }

    /// A reply's usage record and `assistant.message`, or `None` for a reply with nothing the
    /// model said or called (an API error Claude Code wrote itself).
    fn reply(&mut self, m: &RehydrateMessage) -> Option<(Option<Value>, Value)> {
        let text = joined_text(&m.content);
        let requests: Vec<Value> = m
            .content
            .iter()
            .filter_map(|c| match c {
                Content::ToolUse(u) => Some(json!({
                    "toolCallId": tool_call_id(u.id.as_ref()),
                    "name": u.name,
                    "arguments": u.input,
                    "type": "function",
                })),
                _ => None,
            })
            .collect();
        if text.is_none() && requests.is_empty() {
            return None;
        }
        let call = m.turn_id.clone().unwrap_or_else(|| format!("atuin-{}", m.source_id));
        let mut reply = json!({
            "messageId": derived(&m.source_id, "message"),
            "content": text.unwrap_or_default(),
            "toolRequests": requests,
            "interactionId": self.interaction,
            "turnId": self.turn.to_string(),
            "apiCallId": call,
        });
        if let Some(model) = &m.model {
            reply["model"] = json!(model);
        }
        if let Some(provider) = &m.provider {
            reply["providerId"] = json!(provider);
        }
        let usage = m.usage.as_ref().map(|usage| {
            let mut record = usage_json(usage);
            record["apiCallId"] = json!(call);
            if let Some(model) = &m.model {
                record["model"] = json!(model);
            }
            json!({"usage": record})
        });
        self.turn += 1;
        Some((usage, reply))
    }

    /// A `tool.execution_complete` for each result among `m`'s content, with the files an edit
    /// changed and its diff where capture kept the patch.
    fn results(&self, m: &RehydrateMessage) -> Vec<(&'static str, Value)> {
        m.content
            .iter()
            .filter_map(|c| match c {
                Content::ToolResult(r) => Some(r),
                _ => None,
            })
            .map(|r| {
                let patch = m.content.iter().find_map(|c| match c {
                    Content::Patch(patch) if patch.call == r.call => Some(patch),
                    _ => None,
                });
                ("tool.execution_complete", self.result(r, patch))
            })
            .collect()
    }

    fn result(&self, r: &ToolResult, patch: Option<&Patch>) -> Value {
        let output =
            r.output_text().map_or_else(|| UNCAPTURED_OUTPUT.to_owned(), |t| t.into_owned());
        let mut result = json!({"content": output});
        let mut data = json!({
            "toolCallId": tool_call_id(r.call.as_ref()),
            "interactionId": self.interaction,
            "turnId": self.turn.saturating_sub(1).to_string(),
            "success": !r.error,
        });
        if let Some(patch) = patch.filter(|p| !p.files.is_empty()) {
            data["fileEdits"] = patch
                .files
                .iter()
                .map(|file| {
                    let kind = match file.change {
                        Change::Add => "create",
                        Change::Update => "edit",
                        Change::Delete => "delete",
                    };
                    json!({"path": file.path, "kind": kind})
                })
                .collect();
            result["detailedContent"] = json!(format!("\n{}", patch.unified()));
        }
        data["result"] = result;
        data
    }
}

/// The longest tool call id written as it is. Copilot hands a call's id to the model API with a
/// prefix of its own, and the API takes at most 64 characters: a Codex id
/// (`call_<32 hex>_fc_<23 hex>`) is 64 already, so carried whole it fails the session's next
/// turn. Copilot's own are 29.
const TOOL_CALL_ID_MAX: usize = 48;

/// A call's id as the log carries it: its own, or one derived from it when it is too long for
/// the API. A call and its result derive the same id, so they still pair up.
fn tool_call_id(id: &str) -> String {
    if id.len() <= TOOL_CALL_ID_MAX {
        id.to_owned()
    } else {
        derived(id, "call")
    }
}

/// An id for what a row's or session's `base` id derives: the same every time it is written.
fn derived(base: &str, what: &str) -> String {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{PRODUCER}:{base}:{what}").as_bytes()).to_string()
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

/// A `session.usage_record`'s `usage` from `usage`: Copilot counts the cached input in
/// `inputTokens`, which the reader takes out again.
fn usage_json(usage: &Usage) -> Value {
    let mut out = serde_json::Map::new();
    let mut put = |key: &str, n: Option<u64>| {
        if let Some(n) = n {
            out.insert(key.to_owned(), json!(n));
        }
    };
    put("inputTokens", usage.total_input());
    put("outputTokens", usage.output);
    put("cacheReadTokens", usage.cache_read);
    put("cacheWriteTokens", usage.cache_write);
    put("reasoningTokens", usage.reasoning);
    Value::Object(out)
}

#[cfg(test)]
pub(crate) mod tests;
