//! Writing a session out for Mistral Vibe to resume.
//!
//! Vibe's own store is private to its runtime: its files are checksummed and its journal is
//! hash-chained. Vibe still reads the session folders its 2.x versions wrote, and imports one into
//! its store when it is resumed: `<session root>/session_<date>_<time>_<first 8 characters of the
//! id>/`, holding a `meta.json` and a `messages.jsonl` of the messages the model sees (OpenAI-style
//! roles, `tool_calls` and `tool_call_id`). So the writer writes such a folder, and
//! `vibe --resume <id>` imports it as a new session of Vibe's store, which Vibe then resumes
//! whenever the id is asked for again. The folder's `meta.json` carries an `atuin` key, which the
//! reader takes to mean the imported session is this one, and `atuin.jsonl` holds the rows
//! written, as capture holds them (see `session`). Until Vibe imports the folder, more rows can be
//! appended to it (`append`); once it has, the session is Vibe's to write. Vibe takes no lock on
//! the folder as it imports it, so an append can land after Vibe read it: `atuin-revisions.jsonl`
//! says how many of the rows written each version of the folder holds, by the revision Vibe
//! records the import of, and the reader takes only those.
//!
//! The Mistral API refuses a history whose tool calls and results don't pair up: a result must
//! follow the message that made its call, and every call needs a result before the next message.
//! The writer merges an assistant turn's rows into one message, keeps the results that answer its
//! calls, and writes a result saying none was recorded for a call that has none. Vibe's importer
//! drops system messages, so a system row's text goes in as a user message.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::session::{
    ATUIN_KEY, REVISIONS_FILE, WRITTEN_FILE, WrittenRow, held_rows, locate, revision,
};
use crate::harnesstools::rehydrate::{
    Flatten, RehydrateError, RehydrateMessage, RehydrateSession, UNCAPTURED_OUTPUT,
    flatten_uncaptured_calls,
};
use crate::harnesstools::resume::is_plain_name;
use crate::harnesstools::session::{Content, Role};

/// The file in a legacy folder atuin wrote that an append holds locked.
const LOCK_FILE: &str = "atuin.lock";

/// What a call with no recorded result is answered with.
const NO_RESULT: &str = "(no result was recorded for this call)";

/// Write `session` under the session root `root` as a legacy session folder Vibe imports on
/// resume, returning the folder.
pub(crate) async fn rehydrate(
    root: &Path,
    session: &RehydrateSession,
) -> Result<PathBuf, RehydrateError> {
    let (root, session) = (root.to_path_buf(), session.clone());
    tokio::task::spawn_blocking(move || write(&root, &session))
        .await
        .map_err(|e| RehydrateError::Other(e.to_string()))?
}

fn write(root: &Path, session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
    let id = &session.id;
    if !is_plain_name(id) || id.len() < 8 || !id.is_char_boundary(8) {
        return Err(RehydrateError::Other(format!("{id} is not an id Vibe can resume")));
    }
    if let Some(existing) = locate(root, id) {
        return Err(RehydrateError::AlreadyExists(existing));
    }
    let stamp = session.started_at.strftime("%Y%m%d_%H%M%S");
    let folder = root.join(format!("session_{stamp}_{}", &id[..8]));

    let lines = jsonl(&messages(&session.messages))?;
    let written = written(&session.messages)?;
    let ended = session.messages.last().map_or(session.started_at, |m| m.timestamp);
    let meta = json!({
        "session_id": id,
        "parent_session_id": null,
        "start_time": session.started_at.to_string(),
        "end_time": ended.to_string(),
        "git_commit": null,
        "git_branch": session.git_branch,
        "environment": { "working_directory": session.cwd },
        "username": crate::env::var_nonempty("USER")
            .and_then(|user| user.into_string().ok())
            .unwrap_or_else(|| "atuin".to_owned()),
        "title": session.title,
        "title_source": "manual",
        // Vibe's import keeps no parent, so the reader names a fork's from here.
        ATUIN_KEY: { "version": 1, "fork_of": session.fork_of.as_ref().map(|fork| &fork.id) },
    });

    std::fs::create_dir_all(root)?;
    std::fs::create_dir(&folder).map_err(|err| match err.kind() {
        std::io::ErrorKind::AlreadyExists => RehydrateError::AlreadyExists(folder.clone()),
        _ => RehydrateError::Io(err),
    })?;
    let meta = serde_json::to_vec_pretty(&meta).map_err(std::io::Error::other)?;
    // The transcript first: Vibe lists a folder by its `meta.json`.
    crate::fs::write_new(&folder.join(WRITTEN_FILE), &written)?;
    crate::fs::write_new(
        &folder.join(REVISIONS_FILE),
        &revision_line(&meta, &lines, session.messages.len())?,
    )?;
    crate::fs::write_new(&folder.join("messages.jsonl"), &lines)?;
    crate::fs::write_new(&folder.join("meta.json"), &meta)?;
    Ok(folder)
}

/// Append `rows` to the legacy folder atuin wrote at `folder`, which Vibe hasn't imported yet,
/// when its transcript still reads as `stamp` says. Returns the rows appended, by source id.
///
/// The transcript is written again whole, from every row it holds and `rows`, so a result that
/// answers a call written before goes with its call. One append at a time takes the folder's
/// lock. The rows are recorded before the transcript is written, and each version of the
/// transcript says how many of them it holds, so a version Vibe imports, or one a failed append
/// left, reads with its own rows.
pub(crate) fn append(
    folder: &Path,
    stamp: crate::harnesstools::sync::Stamp,
    rows: &[RehydrateMessage],
) -> Result<Vec<String>, crate::harnesstools::sync::SyncError> {
    use std::io::Write as _;

    use crate::fs::lock::{LockError, LockMode, LockOptions};
    use crate::harnesstools::sync::{Stamp, SyncError};

    let other = |e: RehydrateError| SyncError::Other(e.to_string());
    let lock = LockOptions {
        create: true,
        mode: LockMode::Exclusive,
    };
    let _lock = lock.try_open(&folder.join(LOCK_FILE)).map_err(|e| match e {
        LockError::WouldBlock => SyncError::Changed,
        e => SyncError::Other(e.to_string()),
    })?;
    let transcript = folder.join("messages.jsonl");
    if Stamp::of(&std::fs::read(&transcript)?) != stamp {
        return Err(SyncError::Changed);
    }
    let meta = std::fs::read(folder.join("meta.json"))?;
    let parsed: Value =
        serde_json::from_slice(&meta).map_err(|e| SyncError::Other(e.to_string()))?;
    if parsed.get(ATUIN_KEY).is_none() {
        return Err(SyncError::Unsupported("atuin didn't write this Vibe session"));
    }
    let mut all: Vec<RehydrateMessage> =
        held_rows(folder)?.into_iter().map(RehydrateMessage::from).collect();
    all.extend_from_slice(rows);
    let after = jsonl(&messages(&all)).map_err(other)?;
    std::fs::OpenOptions::new()
        .append(true)
        .open(folder.join(REVISIONS_FILE))?
        .write_all(&revision_line(&meta, &after, all.len()).map_err(other)?)?;
    crate::fs::replace(&folder.join(WRITTEN_FILE), &written(&all).map_err(other)?, |_| true)
        .map_err(|e| SyncError::Other(e.to_string()))?;
    crate::fs::replace(&transcript, &after, |now| Stamp::of(now) == stamp).map_err(
        |e| match e {
            crate::fs::ReplaceError::NotReplaced(e)
                if e.kind() == std::io::ErrorKind::Interrupted =>
            {
                SyncError::Changed
            }
            e => SyncError::Other(e.to_string()),
        },
    )?;
    Ok(rows.iter().map(|m| m.source_id.clone()).collect())
}

/// The line of `atuin-revisions.jsonl` saying the folder holds `rows` rows when its `meta.json`
/// is `meta` and its transcript `transcript`.
fn revision_line(meta: &[u8], transcript: &[u8], rows: usize) -> Result<Vec<u8>, RehydrateError> {
    let line = json!({ "revision": revision(meta, transcript), "rows": rows });
    let mut line = serde_json::to_vec(&line).map_err(std::io::Error::other)?;
    line.push(b'\n');
    Ok(line)
}

impl From<WrittenRow> for RehydrateMessage {
    fn from(row: WrittenRow) -> Self {
        Self {
            source_id: row.id,
            parent_source_id: None,
            timestamp: row.time,
            role: row.role,
            content: row.content,
            model: row.model,
            provider: None,
            usage: None,
            stop_reason: row.stop,
            turn_id: None,
            cwd: row.cwd,
            git_branch: row.git_branch,
        }
    }
}

/// `rows` as the lines of `atuin.jsonl`.
fn written(rows: &[RehydrateMessage]) -> Result<Vec<u8>, RehydrateError> {
    let mut lines = Vec::new();
    for row in rows {
        let row = WrittenRow {
            id: row.source_id.clone(),
            role: row.role.clone(),
            time: row.timestamp,
            content: row.content.clone(),
            model: row.model.clone(),
            stop: row.stop_reason.clone(),
            cwd: row.cwd.clone(),
            git_branch: row.git_branch.clone(),
        };
        serde_json::to_writer(&mut lines, &row).map_err(std::io::Error::other)?;
        lines.push(b'\n');
    }
    Ok(lines)
}

fn jsonl(messages: &[Value]) -> Result<Vec<u8>, RehydrateError> {
    let mut lines = Vec::new();
    for message in messages {
        serde_json::to_writer(&mut lines, message).map_err(std::io::Error::other)?;
        lines.push(b'\n');
    }
    Ok(lines)
}

/// The messages Vibe's importer takes for `rows`, with every call answered.
fn messages(rows: &[RehydrateMessage]) -> Vec<Value> {
    let rows = flatten_uncaptured_calls(rows, &Flatten::Runs);
    let mut out = Vec::new();
    // The assistant message being gathered from consecutive assistant rows.
    let mut turn: Option<Turn> = None;
    // The calls the last assistant message made that have no result yet, in order.
    let mut open: Vec<String> = Vec::new();
    for row in &rows {
        if row.role == Role::Assistant {
            close(&mut open, &mut out);
            let turn = turn.get_or_insert_with(Turn::default);
            for block in &row.content {
                turn.absorb(block);
            }
            continue;
        }
        if let Some(done) = turn.take() {
            open.clone_from(&done.calls);
            if let Some(message) = done.message() {
                out.push(message);
            } else {
                open.clear();
            }
        }
        let mut text = Vec::new();
        for block in &row.content {
            match block {
                Content::ToolResult(result) => {
                    let id = result.call.as_ref();
                    if let Some(at) = open.iter().position(|call| call == id) {
                        open.remove(at);
                        let output = result
                            .output_text()
                            .map_or_else(|| UNCAPTURED_OUTPUT.to_owned(), |t| t.into_owned());
                        out.push(json!({ "role": "tool", "tool_call_id": id, "content": output }));
                    }
                }
                Content::Text(t) | Content::Summary(t) if !t.trim().is_empty() => {
                    text.push(t.clone());
                }
                Content::Error(e) if !e.trim().is_empty() => text.push(format!("[error: {e}]")),
                _ => {}
            }
        }
        if !text.is_empty() {
            close(&mut open, &mut out);
            out.push(json!({ "role": "user", "content": text.join("\n\n") }));
        }
    }
    if let Some(done) = turn.take() {
        open.clone_from(&done.calls);
        if let Some(message) = done.message() {
            out.push(message);
        } else {
            open.clear();
        }
    }
    close(&mut open, &mut out);
    out
}

/// Answer every call in `open` with [`NO_RESULT`].
fn close(open: &mut Vec<String>, out: &mut Vec<Value>) {
    for id in open.drain(..) {
        out.push(json!({ "role": "tool", "tool_call_id": id, "content": NO_RESULT }));
    }
}

/// One assistant message, gathered from consecutive assistant rows.
#[derive(Debug, Default)]
struct Turn {
    text: Vec<String>,
    reasoning: Vec<String>,
    tool_calls: Vec<Value>,
    calls: Vec<String>,
    ids: HashSet<String>,
}

impl Turn {
    fn absorb(&mut self, block: &Content) {
        match block {
            Content::Text(t) | Content::Summary(t) if !t.trim().is_empty() => {
                self.text.push(t.clone());
            }
            Content::Error(e) if !e.trim().is_empty() => self.text.push(format!("[error: {e}]")),
            Content::Reasoning(r) if !r.trim().is_empty() => self.reasoning.push(r.clone()),
            Content::ToolUse(call) => {
                let id = call.id.as_ref().to_owned();
                // A call id twice in one message would pair one result with two calls.
                if !self.ids.insert(id.clone()) {
                    return;
                }
                let arguments = match &call.input {
                    Value::String(raw) => raw.clone(),
                    input => input.to_string(),
                };
                self.tool_calls.push(json!({
                    "id": id,
                    "type": "function",
                    "function": { "name": call.name, "arguments": arguments },
                }));
                self.calls.push(id);
            }
            _ => {}
        }
    }

    /// The message, `None` when the rows held nothing to send.
    fn message(self) -> Option<Value> {
        if self.text.is_empty() && self.reasoning.is_empty() && self.tool_calls.is_empty() {
            return None;
        }
        let mut message = json!({ "role": "assistant", "content": self.text.join("\n\n") });
        if !self.reasoning.is_empty() {
            message["reasoning_content"] = Value::String(self.reasoning.join("\n\n"));
        }
        if !self.tool_calls.is_empty() {
            message["tool_calls"] = Value::Array(self.tool_calls);
        }
        Some(message)
    }
}

#[cfg(test)]
pub(crate) mod tests;
