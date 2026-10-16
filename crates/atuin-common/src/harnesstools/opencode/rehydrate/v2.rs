//! The session opencode 2 imports (`opencode session import`): the JSON its `opencode session
//! export` writes (`SessionTransfer.Data`), the session's `info` and its `messages`, each one of
//! its `Session.Message.Info` whole.
//!
//! opencode 2 keeps a session as messages of its own kinds (its `session_message` rows): a
//! `user` prompt, text it injected (`synthetic`), each model call (`assistant`: its text and tool
//! calls, and how it ended), a `compaction`, and how a turn that did not succeed ended (`idle`).
//! Import inserts them under their own ids, in the order given, and refuses a session the
//! database holds already.
//!
//! # What survives
//!
//! - A row capture read from opencode 2, keyed on its message, is written back as that message,
//!   under its id: re-captured, it reads back as itself.
//! - Other rows (opencode 1.x's parts, another harness's lines) are grouped into messages as
//!   opencode 2 lays them out: a prompt's rows into one `user` message, each row of text the
//!   harness injected into a `synthetic` one, and an assistant's rows into one message per model
//!   call (the rows of one turn, up to the one that reports its usage). Their message ids are
//!   minted, and each keeps the source ids of the rows it was written from in its `metadata`
//!   ([`SOURCE`]): re-captured, it reads back as the first of them, and sync knows it holds the
//!   rest. So does a message rows no message is written for (a result joined to its call, a
//!   retry) went with.
//! - A fork's messages all get fresh ids (opencode's are keys of its whole database), and it
//!   starts with a `synthetic` message holding its [marker](continuation::fork_marker_text),
//!   which capture reads as its link.
//! - Reasoning (kept only as a marker), retries, and what capture keeps nothing of are left out.
//!   A call without a result (one opencode was stopped in) is written failed, `aborted`, as
//!   opencode 2 ends one.
//! - The session takes the directory it is imported into, its project, and its title. Its model
//!   is left for opencode to pick, as for a new session: a model another harness ran on is no
//!   provider's of opencode's.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};

use super::{
    Turn, is_failure, is_session_row, join_results, millis, mint, step_tokens, title_of, turn_base,
};
use crate::harnesstools::opencode::session::SOURCE;
use crate::harnesstools::rehydrate::{
    Flatten, RehydrateMessage, RehydrateSession, UNCAPTURED_OUTPUT, flatten_uncaptured_calls,
};
use crate::harnesstools::session::{Change, Content, Patch, Role, StopReason, ToolResult, ToolUse};
use crate::harnesstools::{AnyHarness, continuation};

/// What kind of opencode 2 message a draft is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    User,
    Synthetic,
    Assistant,
    Compaction,
    Idle,
}

/// A message being built from its rows.
struct Draft {
    kind: Kind,
    /// Its own id, for a row of opencode 2 (keyed on its message).
    id: Option<String>,
    /// The source ids of its rows, in order.
    sources: Vec<String>,
    /// The row a prompt's rows follow on from, which tells one prompt's rows from the next's.
    parent: Option<String>,
    /// The rows' turn (without step counts), which tells an assistant's rows from the next call's.
    turn: Option<String>,
    created: i64,
    completed: i64,
    model: Option<String>,
    /// Who served `model`, when capture knows.
    provider: Option<String>,
    /// A prompt's or injected text's paragraphs; a compaction's summary.
    text: Vec<String>,
    /// An assistant's content (`text` and `tool` items).
    content: Vec<Value>,
    /// The model call reported its usage: the call is over.
    tokens: Option<Value>,
    finish: Option<&'static str>,
    error: Option<Value>,
    /// An `idle` message's outcome.
    outcome: &'static str,
}

impl Draft {
    fn new(kind: Kind, row: &RehydrateMessage) -> Self {
        let at = millis(row.timestamp);
        Self {
            kind,
            id: own_id(row),
            sources: Vec::new(),
            parent: row.parent_source_id.clone(),
            turn: turn_base(row),
            created: at,
            completed: at,
            model: row.model.clone(),
            provider: row.provider.clone(),
            text: Vec::new(),
            content: Vec::new(),
            tokens: None,
            finish: None,
            error: None,
            outcome: "failed",
        }
    }

    /// Whether `row`, of `kind`, is more of this message.
    fn holds(&self, kind: Kind, row: &RehydrateMessage) -> bool {
        if self.kind != kind || self.id.is_some() || own_id(row).is_some() {
            return false;
        }
        match kind {
            Kind::User => self.parent == row.parent_source_id,
            Kind::Assistant => self.tokens.is_none() && self.turn == turn_base(row),
            Kind::Synthetic | Kind::Compaction | Kind::Idle => false,
        }
    }
}

/// The id of the opencode 2 message `row` is the whole of: a row capture read from opencode 2,
/// or a failed 1.x assistant message, both keyed on the message.
fn own_id(row: &RehydrateMessage) -> Option<String> {
    let whole = row.source_id.starts_with("msg_") && !row.source_id.contains('/');
    whole.then(|| row.source_id.clone())
}

/// What kind of message `row` goes into, and `None` for a row no message carries.
fn kind_of(row: &RehydrateMessage) -> Option<Kind> {
    match (&row.role, row.content.as_slice()) {
        (Role::User, content) if content.iter().any(|c| matches!(c, Content::Text(_))) => {
            Some(Kind::User)
        }
        (Role::System, [Content::Text(_), ..]) => Some(Kind::Synthetic),
        // how an opencode 2 turn that did not succeed ended
        (Role::System, [])
            if matches!(row.stop_reason, Some(StopReason::Error | StopReason::Aborted)) =>
        {
            Some(Kind::Idle)
        }
        (Role::Assistant, [Content::Summary(_)]) => Some(Kind::Compaction),
        // a retry's error: the call went on
        (Role::Assistant, [Content::Error(_)]) if !is_failure(row) => None,
        (Role::Assistant, _) => Some(Kind::Assistant),
        _ => None,
    }
}

/// The JSON `opencode session export` writes for `session`, which `opencode session import`
/// reads back.
pub fn export(session: &RehydrateSession) -> Value {
    // A note joins a text part of its own model call.
    let rows = flatten_uncaptured_calls(&session.messages, &Flatten::Notes {
        host: &|host, row| {
            host.parent_source_id == row.parent_source_id && turn_base(host) == turn_base(row)
        },
        own: &|_| true,
    });
    let mut drafts: Vec<Draft> = Vec::new();
    let joined = join_results(rows.clone());
    let mut joined = joined.iter().peekable();
    // rows no message is written for (a result joined to its call, a retry): held all the same,
    // merged into the message they went with
    let mut unwritten: Vec<String> = Vec::new();
    // the draft each call went into, which its result's row goes with
    let mut calls: HashMap<String, usize> = HashMap::new();
    for original in &rows {
        let Some(row) = joined.next_if(|row| row.source_id == original.source_id) else {
            let call = match original.content.first() {
                Some(Content::ToolResult(result)) => calls.get(result.call.as_ref()),
                _ => None,
            };
            match call {
                Some(&at) => drafts[at].sources.push(original.source_id.clone()),
                None => unwritten.push(original.source_id.clone()),
            }
            continue;
        };
        if is_session_row(session, row) {
            continue;
        }
        let Some(kind) = kind_of(row) else {
            unwritten.push(row.source_id.clone());
            continue;
        };
        if !drafts.last().is_some_and(|draft| draft.holds(kind, row)) {
            drafts.push(Draft::new(kind, row));
        }
        let draft = drafts.last_mut().expect("a draft was just ensured");
        draft.completed = draft.completed.max(millis(row.timestamp));
        if draft.model.is_none() {
            draft.model.clone_from(&row.model);
        }
        if draft.provider.is_none() {
            draft.provider.clone_from(&row.provider);
        }
        draft.sources.push(row.source_id.clone());
        draft.sources.append(&mut unwritten);
        add(draft, row);
        for content in &row.content {
            if let Content::ToolUse(call) = content {
                calls.insert(call.id.to_string(), drafts.len() - 1);
            }
        }
    }
    if let Some(last) = drafts.last_mut() {
        last.sources.append(&mut unwritten);
    }

    let mut used: HashSet<String> = HashSet::new();
    // each message written, whether it kept its own id, and the rows it holds
    let mut written: Vec<(Value, bool, Vec<String>)> = Vec::new();
    // the rows of drafts no message is written for (a turn of reasoning alone), before any is
    let mut orphaned: Vec<String> = Vec::new();
    for (n, draft) in drafts.iter().enumerate() {
        let id = match &draft.id {
            Some(id) if session.fork_of.is_none() && used.insert(id.clone()) => id.clone(),
            _ => mint("msg", draft.created, &format!("{}/message/{n}", session.id)),
        };
        let Some(message) = message(draft, &id) else {
            // held all the same: by the message before it, else the one after
            match written.last_mut() {
                Some((_, _, sources)) => sources.extend(draft.sources.iter().cloned()),
                None => orphaned.extend(draft.sources.iter().cloned()),
            }
            continue;
        };
        let mut sources = draft.sources.clone();
        let after_first = sources.len().min(1);
        sources.splice(after_first..after_first, std::mem::take(&mut orphaned));
        written.push((message, draft.id.as_ref() == Some(&id), sources));
    }
    let mut messages: Vec<Value> = written
        .into_iter()
        .map(|(mut message, keyed, sources)| {
            // keyed on the rows it was written from, so capture reads it back as them (see
            // `SOURCE`); a fork's are new rows of a new session
            if (!keyed || sources.len() > 1)
                && session.fork_of.is_none()
                && let Some((first, merged)) = sources.split_first()
            {
                message["metadata"] =
                    json!({SOURCE: {"session": session.id, "source": first, "merged": merged}});
            }
            message
        })
        .collect();
    if let Some(of) = &session.fork_of {
        let source = AnyHarness::from_name("opencode").expect("opencode is a harness");
        let created = messages
            .first()
            .and_then(|m| m["time"]["created"].as_i64())
            .unwrap_or_else(|| millis(session.started_at));
        messages.insert(
            0,
            json!({
                "id": mint("msg", created.saturating_sub(1), &format!("{}/fork", session.id)),
                "type": "synthetic",
                "time": {"created": created},
                "text": continuation::fork_marker_text(source, &of.id, of.atuin_id.as_deref()),
            }),
        );
    }

    // how the last turn ended: failed or interrupted when its idle row says so
    let outcome = match drafts.last() {
        Some(draft) if draft.kind == Kind::Idle => draft.outcome,
        _ => "succeeded",
    };
    let started = millis(session.started_at);
    let updated = session.messages.iter().map(|m| millis(m.timestamp)).max().unwrap_or(started);
    let updated = updated.max(started);
    json!({
        "info": {
            "id": session.id,
            // Import puts the session in the project of the directory it is imported into.
            "projectID": "",
            "title": session.title.clone().unwrap_or_else(|| title_of(session)),
            "cost": 0,
            "tokens": zero_tokens(),
            // idle: a session that is not is one opencode waits on to finish its turn
            "time": {"created": started, "updated": updated, "idle": updated},
            "outcome": outcome,
            "location": {"directory": session.cwd},
        },
        "messages": messages,
    })
}

fn zero_tokens() -> Value {
    json!({"input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}})
}

/// Add what `row` says to the message `draft` stands for.
fn add(draft: &mut Draft, row: &RehydrateMessage) {
    let at = millis(row.timestamp);
    for content in &row.content {
        match content {
            Content::Text(text) if draft.kind == Kind::Assistant => {
                draft.content.push(json!({"type": "text", "text": text}));
            }
            Content::Text(text) | Content::Summary(text) => draft.text.push(text.clone()),
            Content::ToolUse(call) => {
                let result = row.content.iter().find_map(|c| match c {
                    Content::ToolResult(result) if result.call == call.id => Some(result),
                    _ => None,
                });
                let patch = row.content.iter().find_map(|c| match c {
                    Content::Patch(patch) if patch.call == call.id => Some(patch),
                    _ => None,
                });
                draft.content.push(tool(call, result, patch, at));
            }
            Content::Error(why) => draft.error = Some(error(why, row.stop_reason.as_ref())),
            _ => {}
        }
    }
    if draft.kind == Kind::Idle {
        draft.outcome = match row.stop_reason {
            Some(StopReason::Aborted) => "interrupted",
            _ => "failed",
        };
    }
    if let Some(usage) = &row.usage {
        draft.tokens = Some(step_tokens(row, usage));
    }
    if row.usage.is_some() || row.stop_reason.is_some() {
        draft.finish = Some(finish_of(row.stop_reason.as_ref()));
    }
    // opencode ends a call it was stopped in with an error saying so
    if draft.kind == Kind::Assistant
        && row.stop_reason == Some(StopReason::Aborted)
        && draft.error.is_none()
    {
        draft.error = Some(error("aborted", row.stop_reason.as_ref()));
    }
}

/// A model call's `finish` as opencode 2 takes it, which is one of its own few: an aborted call
/// finished in an `error`, and one of a reason it doesn't know `unknown`.
fn finish_of(stop: Option<&StopReason>) -> &'static str {
    match stop {
        None | Some(StopReason::EndTurn | StopReason::StopSequence) => "stop",
        Some(StopReason::MaxTokens) => "length",
        Some(StopReason::ToolUse) => "tool-calls",
        Some(StopReason::Refusal) => "content-filter",
        Some(StopReason::Error | StopReason::Aborted) => "error",
        Some(StopReason::Other(other)) => match other.as_str() {
            "stop" => "stop",
            "length" => "length",
            "tool-calls" => "tool-calls",
            "content-filter" => "content-filter",
            "error" => "error",
            _ => "unknown",
        },
    }
}

/// The message `draft` is, under `id`; `None` for one with nothing to say.
fn message(draft: &Draft, id: &str) -> Option<Value> {
    let created = draft_created(draft);
    let text = draft.text.join("\n\n");
    Some(match draft.kind {
        Kind::User => {
            json!({"id": id, "type": "user", "time": {"created": created}, "text": text, "files": []})
        }
        Kind::Synthetic => {
            json!({"id": id, "type": "synthetic", "time": {"created": created}, "text": text})
        }
        Kind::Compaction => {
            let mut message = json!({
                "id": id,
                "type": "compaction",
                "time": {"created": created},
                "status": "completed",
                "reason": "auto",
                "summary": text,
                "recent": "",
                "cost": 0,
                "tokens": draft.tokens.clone().unwrap_or_else(zero_tokens),
            });
            // the model its call ran on, which its turn is keyed on
            if let Some(Turn::Call {
                provider, model, ..
            }) = draft.turn.as_deref().and_then(Turn::of)
            {
                message["model"] = json!({"id": model, "providerID": provider});
            }
            message
        }
        Kind::Idle => {
            json!({"id": id, "type": "idle", "time": {"created": created}, "outcome": draft.outcome})
        }
        Kind::Assistant => {
            if draft.content.is_empty() && draft.error.is_none() && draft.tokens.is_none() {
                return None;
            }
            let (provider, model) = provider_model(draft);
            let mut message = json!({
                "id": id,
                "type": "assistant",
                // opencode imports only a call that ended
                "time": {"created": created, "completed": draft.completed.max(created)},
                "agent": "build",
                "model": {"id": model, "providerID": provider},
                "content": draft.content,
                "cost": 0,
                "tokens": draft.tokens.clone().unwrap_or_else(zero_tokens),
            });
            if let Some(finish) = &draft.finish {
                message["finish"] = json!(finish);
            }
            if let Some(error) = &draft.error {
                message["error"] = error.clone();
            }
            message
        }
    })
}

/// When a message was created: an assistant's, as its turn says (the call's start, which the turn
/// key of every row of it repeats), else its first row's time.
fn draft_created(draft: &Draft) -> i64 {
    match draft.turn.as_deref().and_then(Turn::of) {
        Some(Turn::Call { created, .. }) if draft.kind == Kind::Assistant => created,
        _ => draft.created,
    }
}

/// The provider and model an assistant message ran on: its turn's, else what its rows say. A
/// model is kept whole when its provider was captured (OpenRouter's `anthropic/claude-sonnet-4`),
/// else names its provider before a `/`, else is opencode's own.
fn provider_model(draft: &Draft) -> (String, String) {
    if let Some(Turn::Call {
        provider, model, ..
    }) = draft.turn.as_deref().and_then(Turn::of)
    {
        return (provider.to_owned(), model.to_owned());
    }
    if let (Some(provider), Some(model)) = (&draft.provider, &draft.model) {
        return (provider.clone(), model.clone());
    }
    match draft.model.as_deref().map(|model| model.split_once('/').unwrap_or(("opencode", model))) {
        Some((provider, model)) => (provider.to_owned(), model.to_owned()),
        None => ("opencode".to_owned(), "unknown".to_owned()),
    }
}

/// An error as opencode 2 reports one (`Session.StructuredError`), saying `why`.
fn error(why: &str, stop: Option<&StopReason>) -> Value {
    let kind = match stop {
        Some(StopReason::Aborted) => "aborted",
        Some(StopReason::Refusal) => "provider.content-filter",
        _ => "unknown",
    };
    json!({"type": kind, "message": why})
}

/// A tool item of an assistant message: `completed` or `error` with its result, failed as
/// `aborted` without one. A completed call whose patch capture kept gets the metadata opencode 2
/// keeps the diff in (`files[]`, `{file, patch, status}`).
fn tool(call: &ToolUse, result: Option<&ToolResult>, patch: Option<&Patch>, at: i64) -> Value {
    // opencode's tool input is always an object.
    let input = match &call.input {
        Value::Object(_) => call.input.clone(),
        other => Value::Object(Map::from_iter([("input".to_owned(), other.clone())])),
    };
    let state = match result {
        // a command opencode 2 ran that failed: completed, its exit code in the metadata
        Some(result) if result.error && call.name == "shell" && result.output.is_array() => json!({
            "status": "completed",
            "input": input,
            "content": content_of(&result.output),
            "metadata": {"exit": exit_code(&result.output)},
        }),
        Some(result) if result.error => json!({
            "status": "error",
            "input": input,
            "error": {"type": "tool.execution", "message": text_of(&result.output)},
        }),
        Some(result) => {
            let mut state = json!({
                "status": "completed",
                "input": input,
                "content": content_of(&result.output),
            });
            if let Some(patch) = patch {
                state["metadata"] = files(patch);
            }
            state
        }
        None => json!({
            "status": "error",
            "input": input,
            "error": {"type": "aborted", "message": "Tool execution was interrupted"},
        }),
    };
    json!({
        "type": "tool",
        "id": call.id.as_ref(),
        "name": call.name,
        "state": state,
        "time": {"created": at, "ran": at, "completed": at},
    })
}

/// A tool's output as opencode 2 keeps it: a list of content items, `{type: "text", text}` or
/// `{type: "file", uri, ..}`. A list read from opencode 2 is kept as it is; another harness's
/// blocks are made text (a text block's own, else a note of what it was: `[image]`), since
/// opencode refuses an import holding any item of another shape.
fn content_of(output: &Value) -> Value {
    let Value::Array(items) = output else {
        return json!([{"type": "text", "text": text_of(output)}]);
    };
    let items =
        items.iter().map(|item| match (item["type"].as_str(), &item["text"], &item["uri"]) {
            (Some("file"), _, Value::String(_)) => item.clone(),
            // `text`, and another harness's text by another name (Codex's `input_text`)
            (_, Value::String(text), _) => json!({"type": "text", "text": text}),
            (kind, ..) => {
                json!({"type": "text", "text": format!("[{}]", kind.unwrap_or("output"))})
            }
        });
    Value::Array(items.collect())
}

/// The exit code a failed command's output reports (`Exited with code 2`, the shell tool's last
/// item), else 1.
fn exit_code(output: &Value) -> i64 {
    output
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item["text"].as_str()?.strip_prefix("Exited with code ")?.trim().parse().ok()
        })
        .next_back()
        .unwrap_or(1)
}

/// A tool's output as one text.
fn text_of(output: &Value) -> String {
    match output {
        Value::String(text) => text.clone(),
        Value::Null => UNCAPTURED_OUTPUT.to_owned(),
        Value::Array(items) if items.iter().all(|item| item["text"].is_string()) => {
            items.iter().filter_map(|item| item["text"].as_str()).collect::<Vec<_>>().join("\n")
        }
        other => other.to_string(),
    }
}

/// The metadata opencode 2 keeps of a call that changed files (`edit`, `write`, `apply_patch`).
fn files(patch: &Patch) -> Value {
    let files: Vec<Value> = patch
        .files
        .iter()
        .map(|file| {
            let status = match file.change {
                Change::Add => "added",
                Change::Delete => "deleted",
                Change::Update => "modified",
            };
            json!({
                "file": file.moved_to.as_deref().unwrap_or(&file.path),
                "patch": file.unified(),
                "status": status,
                "additions": file.additions(),
                "deletions": file.deletions(),
            })
        })
        .collect();
    json!({"files": files})
}

#[cfg(test)]
mod tests;
