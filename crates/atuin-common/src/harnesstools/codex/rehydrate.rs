//! A Codex rollout written back out from captured messages (see [`rehydrate`]).
//!
//! # What survives
//!
//! Every row capture keys on a native id is written back under that id, so re-capturing the
//! rollout finds the rows already synced:
//!
//! - messages (`response_item` `message`), under their `id`; a system row (a continuation's
//!   marker) is written as a `developer` message;
//! - tool calls and their outputs (`function_call`, `custom_tool_call`, `local_shell_call`,
//!   `web_search_call`, `image_generation_call`, `tool_search_call` and their `*_output`s), under
//!   their `id`, or with none when capture keyed them on their `call_id` (`<call_id>#out` for an
//!   output);
//! - the files an applied patch changed, as the `item_completed` event of a `FileChange` item
//!   (under the patch's call, in the turn it was applied in), which Codex shows the diff from on
//!   resume; capture reads it back as the same row, whether it first read a legacy rollout's
//!   `patch_apply_end` or a paginated one's `item_completed`;
//! - usage, as the line it was captured from: a `token_usage_record` under its response (or the
//!   thread total it was keyed on), a `token_count` under the running total its key spells out;
//! - the session itself, as the rollout's `session_meta` (under the session id).
//!
//! Rows capture keyed on their content (`syn-<hash>`: a line without an id) are written back
//! with the very fields that content hash covers, so they hash the same again: `turn_context`
//! (its model and original cwd), `compacted` summaries, and a turn's failure (`turn_aborted`, or
//! a `task_complete` carrying an error). A line that had no timestamp of its own (only rollouts
//! from before Codex 0.32 have those) now has one, so it would hash differently: a `message`
//! among them (those rollouts' prompts, which had no id) is written under its `syn-` source id,
//! which capture reads back as it is; any other such line is captured again as a new row.
//!
//! # The format
//!
//! The rollout is written in Codex's paginated history mode, as Codex writes one since 0.156
//! (`session_meta.history_mode: "paginated"`): every line numbered (`ordinal`) from 0 on, which
//! Codex keeps numbering from when it continues the rollout.
//!
//! Codex shows a paginated rollout's history from its turn and item events, which the rows are
//! written between (see `rows`): `task_started`, `item_completed` for each prompt and answer,
//! `task_complete`. Capture keeps no row of any of them.
//!
//! # What is dropped
//!
//! - **Reasoning.** Capture keeps only that a model reasoned ([`Content::ReasoningSummary`]),
//!   never its summary or its `encrypted_content`, which only the model can read. A `reasoning`
//!   item without that ciphertext is worse than none: resuming sends it back to the Responses
//!   API by `id`, which a store-less request cannot resolve. The model's earlier reasoning is not
//!   something it needs to continue, so these items are left out, and re-capture has no row to
//!   compare them with.
//! - **Tool calls captured without their input** (`ai.capture_tools` off, or a policy withheld
//!   it). A
//!   `function_call`'s `arguments` must be a string of JSON, a `custom_tool_call`'s `input` a
//!   string, a `local_shell_call`'s `action` an object: Codex cannot read the line back, let alone
//!   send it. Each becomes a note (`[ran a shell command]`; see [`tool_note`](crate::harnesstools::note::tool_note)) and its
//!   output is dropped ([`Flatten::Notes`]). A note joins the text of the assistant `message`
//!   before it in the turn (the same note several times in a row counted, `×3`), written under
//!   that message's id; with none, it is an assistant `message` of its own under the call's id
//!   (its `id`, or its `call_id` when capture keyed it on that). Re-captured, either reads back
//!   under a source id capture already holds, so nothing is pushed, and the outputs are not there
//!   to capture again. A line keyed on its content (`syn-`) would hash differently once changed:
//!   a note never joins one, and a call keyed so (none in practice: every call has a `call_id`)
//!   is left out. The line of the written row before them records the rows written as no line
//!   ([`MERGED_FIELD`](crate::harnesstools::rehydrate::MERGED_FIELD)), so the rollout says which
//!   synced rows it holds.
//! - **Tool output**: written back as captured. A call kept with its input but not its output
//!   (withheld by a policy, or over the size limit) gets [`UNCAPTURED_OUTPUT`] as its output (an
//!   empty `tools` list for a tool search).
//! - Rows whose line carries nothing Codex needs back: thread names (Codex keeps those in its
//!   `session_index.jsonl` now) and other events.
//! - Content kinds a line of the kind cannot carry (text inside a tool call, and so on).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

use super::session::{self, archive_of, session_id_of};
use crate::harnesstools::rehydrate::{
    Flatten, RehydrateError, RehydrateMessage, RehydrateSession, UNCAPTURED_OUTPUT,
    flatten_uncaptured_calls, record_merged,
};
use crate::harnesstools::resume::is_plain_name;
use crate::harnesstools::session::{
    Change, Content, Patch, Role, StopReason, ToolResult, ToolUse, Usage,
};

/// What Codex puts ahead of a compaction summary (codex-rs
/// `prompts/templates/compact/summary_prefix.md`), which its reader strips again.
const COMPACTION_PREAMBLE: &str =
    "Another language model started to solve this problem and produced a summary of its thinking \
     process. You also have access to the state of the tools that were used by that language \
     model. Use this to build on the work that has already been done and avoid duplicating work. \
     Here is the summary produced by the other language model, use the information in this \
     summary to assist with your own analysis:\n";

/// Write `session` as a Codex rollout in Codex's own sessions directory (`$CODEX_HOME/sessions`,
/// else `~/.codex/sessions`, the one `locate` searches), under
/// `<yyyy>/<mm>/<dd>/rollout-<yyyy-mm-ddThh-mm-ss>-<id>.jsonl` from its start (UTC), and hand back
/// its path. `codex resume <id>` finds it from any directory.
///
/// Fails with [`RehydrateError::AlreadyExists`] when Codex already has a rollout of the session,
/// live or archived; the file is written whole under a temporary name and moved into place, so
/// no reader ever sees half of it and an existing file is never replaced.
///
/// Where Codex's thread index still names a rollout of the session that is gone (the transcript
/// was deleted), the index is pointed at the new one, or Codex would find none (see
/// `codex::state_db`). Where that fails, the rollout is left where it is, as Codex may have
/// indexed it itself meanwhile (it is visible from the moment it is written): a stale file is
/// better than a thread whose rollout is gone. A later restore finds that rollout and points the
/// index at it again.
pub async fn rehydrate(session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
    let root = session::default_root();
    let home = root.parent().map(Path::to_path_buf).ok_or(RehydrateError::NoDataDir)?;
    rehydrate_into(&root, &home, session).await
}

/// [`rehydrate`] under the sessions directory `root` of Codex home `home`.
pub(crate) async fn rehydrate_into(
    root: &Path,
    home: &Path,
    session: &RehydrateSession,
) -> Result<PathBuf, RehydrateError> {
    let (sessions, session) = (root.to_path_buf(), session.clone());
    let thread = session::resume_id(&session.id).to_owned();
    let written = tokio::task::spawn_blocking(move || write(&sessions, &session))
        .await
        .map_err(|err| RehydrateError::Other(err.to_string()))?;
    let path = match written {
        Ok(path) => path,
        // A live rollout an earlier restore left behind when pointing the index at it failed:
        // point it again, or Codex keeps resuming from the gone rollout the index names. An index
        // naming another rollout that exists means Codex has moved the thread on: leave it.
        Err(RehydrateError::AlreadyExists(existing)) if existing.starts_with(root) => {
            return match super::state_db::point_at(home, &thread, None, &existing).await {
                Ok(_) | Err(super::state_db::StateDbError::Elsewhere(_)) => {
                    Err(RehydrateError::AlreadyExists(existing))
                }
                Err(err) => Err(RehydrateError::Other(format!(
                    "{err} (the rollout is at {})",
                    existing.display()
                ))),
            };
        }
        Err(err) => return Err(err),
    };
    if let Err(err) = super::state_db::point_at(home, &thread, None, &path).await {
        return Err(RehydrateError::Other(format!(
            "{err} (the rollout is left at {})",
            path.display()
        )));
    }
    Ok(path)
}

/// [`rehydrate`] under the sessions directory `root`.
pub(crate) fn write(root: &Path, session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
    if !is_plain_name(&session.id) {
        return Err(RehydrateError::Other(format!("{:?} is not a session id", session.id)));
    }
    let started = session.started_at.to_offset(UtcOffset::UTC);
    let name = format!(
        "rollout-{:04}-{:02}-{:02}T{:02}-{:02}-{:02}-{}.jsonl",
        started.year(),
        u8::from(started.month()),
        started.day(),
        started.hour(),
        started.minute(),
        started.second(),
        session.id,
    );
    // The reader takes the session id from the file name; one it would read back differently
    // (not a UUID) would be a rollout of some other session.
    let stem = name.trim_end_matches(".jsonl");
    if session_id_of(stem).as_ref() != session.id {
        return Err(RehydrateError::Other(format!("{:?} is not a Codex session id", session.id)));
    }
    let existing = session::locate(root, &session.id)
        .or_else(|| archive_of(root).and_then(|archive| session::locate(&archive, &session.id)));
    if let Some(existing) = existing {
        return Err(RehydrateError::AlreadyExists(existing));
    }
    let dir = root
        .join(format!("{:04}", started.year()))
        .join(format!("{:02}", u8::from(started.month())))
        .join(format!("{:02}", started.day()));
    std::fs::create_dir_all(&dir)?;
    let base = inherited_history(root, session);
    let mut text = String::new();
    for line in rollout(session, base) {
        text.push_str(&line.to_string());
        text.push('\n');
    }
    let path = dir.join(&name);
    create_new(&path, text.as_bytes())?;
    Ok(path)
}

/// Write `path` whole or not at all, and never over an existing file (see
/// [`crate::fs::write_new`]): the temporary file it is written as first is named so that no
/// watcher takes it for a rollout.
pub(crate) fn create_new(path: &Path, bytes: &[u8]) -> Result<(), RehydrateError> {
    crate::fs::write_new(path, bytes).map_err(|err| {
        if err.kind() == std::io::ErrorKind::AlreadyExists {
            RehydrateError::AlreadyExists(path.to_path_buf())
        } else {
            err.into()
        }
    })
}

/// The time as Codex stamps a line: RFC 3339 in UTC, as precise as it was captured.
pub(crate) fn stamp(at: OffsetDateTime) -> String {
    at.to_offset(UtcOffset::UTC).format(&Rfc3339).unwrap_or_default()
}

/// A rollout line (codex-rs `RolloutLine`).
pub(crate) fn line(at: &str, kind: &str, payload: Value) -> Value {
    let mut line = Map::new();
    line.insert("timestamp".to_owned(), json!(at));
    line.insert("type".to_owned(), json!(kind));
    line.insert("payload".to_owned(), payload);
    Value::Object(line)
}

/// Which kind of call a tool call was, so that its output is written as the matching kind.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CallKind {
    Function,
    Custom,
    ToolSearch,
}

/// The rollout's lines: its `session_meta`, then each message's, numbered on from it: from 0, or
/// for a rollout continuing the history `base` names, from where that ends (its
/// `end_ordinal_exclusive`, as Codex numbers such a rollout: codex-rs `ordinal.rs`).
fn rollout(session: &RehydrateSession, base: Option<Value>) -> Vec<Value> {
    let mut header = header(session);
    let first = base.as_ref().and_then(|b| b["end_ordinal_exclusive"].as_u64()).unwrap_or(0);
    if let Some(base) = base {
        header["payload"]["history_base"] = base;
    }
    let mut lines = vec![header];
    for row in rows(session, &session.messages) {
        lines.extend(row.before.into_iter().chain(row.lines));
    }
    for (ordinal, line) in (first..).zip(lines.iter_mut()) {
        line["ordinal"] = json!(ordinal);
    }
    lines
}

/// The history a fork's rollout continues, as its original's does: the `history_base` of the
/// original's own rollout here ([`ForkOf::path`](crate::harnesstools::rehydrate::ForkOf::path)),
/// when it has one and the rollout it names is here too, live or archived.
///
/// A rollout a thread was reverted into (a `<thread>_<rollout>` session), and one Codex forked in
/// its paginated mode, holds only the history since then, and names the rest in its
/// `history_base`: the immutable rollout it continues, and where in it that history ends
/// (codex-rs `rollout_lineage.rs`). Capture keeps none of that, so a fork names the same base,
/// and resumes with the whole history its original resumes with. Without the original's rollout
/// here (another host's), where its base ends can't be known: the fork holds what restoring the
/// original would, the rows since.
fn inherited_history(root: &Path, session: &RehydrateSession) -> Option<Value> {
    let of = session.fork_of.as_ref()?;
    let base = session::own_meta_at(of.path.as_deref()?, &of.id)?["history_base"].clone();
    let rollout = base["thread_id"].as_str()?;
    base["end_ordinal_exclusive"].as_u64()?;
    base["end_byte_offset"].as_u64()?;
    // The base's session, as capture names it (see `session::session_id_of`).
    let thread = session::resume_id(&of.id);
    let id = if rollout == thread {
        thread.to_owned()
    } else {
        format!("{thread}_{rollout}")
    };
    let here = session::locate(root, &id)
        .or_else(|| archive_of(root).and_then(|archive| session::locate(&archive, &id)));
    here.map(|_| base)
}

/// A new rollout's `session_meta` line (without its ordinal), in Codex's paginated history mode:
/// every line numbered (`ordinal`), which Codex keeps numbering from the last line's when it
/// continues the rollout.
///
/// The meta is the session as it is resumed here: its cwd is this machine's. It names no
/// `model_provider`, which capture does not keep: Codex takes a rollout without one for its
/// configured provider, where one naming another would be hidden from `codex resume`'s picker.
/// Nor does it carry `base_instructions`, which Codex then renders from its current model's.
/// Its `cli_version` is a development build's, which capture (like Codex) reads as a Codex
/// older than per-call usage records, so that `token_count` lines count as usage again when that
/// is what they were captured as; `token_usage_record` lines are read as records either way.
pub(crate) fn header(session: &RehydrateSession) -> Value {
    let thread = session::resume_id(&session.id);
    let mut meta = json!({
        "session_id": thread,
        "id": thread,
        "timestamp": stamp(session.started_at),
        "cwd": session.cwd,
        "originator": "codex_cli_rs",
        "cli_version": "0.0.0",
        "source": "cli",
        "history_mode": "paginated",
    });
    if let Some(branch) = &session.git_branch {
        meta["git"] = json!({"branch": branch});
    }
    // A fork names the thread it was forked from, as Codex's own do. Codex reads the field as a
    // thread id: never the `<thread>_<rollout>` capture names a segment of a thread by. A fork
    // of a segment names that in a field of atuin's own too, which Codex's reader leaves be, and
    // capture links the fork to the segment by it.
    if let Some(of) = &session.fork_of {
        let thread = session::resume_id(&of.id);
        meta["forked_from_id"] = json!(thread);
        if thread != of.id {
            meta[session::ATUIN_FORKED_FROM] = json!(of.id);
        }
    }
    line(&stamp(session.started_at), "session_meta", meta)
}

/// The lines one captured row is written as (none for a row the rollout cannot carry): `lines`,
/// the first of them the one capture reads the row back from, and `before` it, events that go
/// ahead of it.
pub(crate) struct Row {
    pub source_id: String,
    pub before: Vec<Value>,
    pub lines: Vec<Value>,
}

/// The lines `messages` are written as, row by row, unnumbered. Calls captured without their
/// input are flattened into notes first (see the module docs).
///
/// The rows written as no line are recorded on the line of the written row before them as merged
/// into it ([`MERGED_FIELD`](crate::harnesstools::rehydrate::MERGED_FIELD)).
///
/// Besides each row's own line, the events Codex shows a paginated rollout's history from
/// (`task_started`, `item_completed`, `task_complete`: codex-rs `thread_history_projection.rs`),
/// none of which capture keeps a row of. Those are turns: each user prompt's
/// turn starts (`task_started`) at the context and `turn_context` rows just before the prompt,
/// holds the prompt and the model's answers as items (`item_completed`, the prompt's after it
/// and each answer's ahead of it, as Codex writes them), and ends (`task_complete`, or the
/// turn's own failure, which gets its turn id) after its last row. Capture keeps no turn ids,
/// so each turn is named after its prompt's row, the same every time.
pub(crate) fn rows(session: &RehydrateSession, messages: &[RehydrateMessage]) -> Vec<Row> {
    let mut calls = HashMap::new();
    // A line keyed on its content would be captured again as a new row once its content changes.
    let keyed = |m: &RehydrateMessage| !m.source_id.starts_with("syn-");
    let messages = flatten_uncaptured_calls(messages, &Flatten::Notes {
        host: &|host, _| keyed(host),
        own: &keyed,
    });
    let mut rows: Vec<Row> = messages
        .iter()
        .map(|message| Row {
            source_id: message.source_id.clone(),
            before: Vec::new(),
            lines: message_lines(session, message, &mut calls),
        })
        .collect();
    record_rows_merged(&mut rows);
    turns(session, &messages, &mut rows);
    rows
}

/// Record the rows written as no line on the line capture reads back the written row before them
/// (after them, for those before any), as merged into it ([`MERGED_FIELD`](crate::harnesstools::rehydrate::MERGED_FIELD)): Codex places rows
/// by number, not by what they hang from. Never on a line keyed on its content, which would read
/// back as another row.
fn record_rows_merged(rows: &mut [Row]) {
    let keyed = |row: &Row| !row.lines.is_empty() && !row.source_id.starts_with("syn-");
    let first = rows.iter().position(keyed);
    let mut into = None;
    let mut merged: Vec<(usize, String)> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        if keyed(row) {
            into = Some(i);
        } else if row.lines.is_empty()
            && let Some(at) = into.or(first)
        {
            merged.push((at, row.source_id.clone()));
        }
    }
    for (at, row) in merged {
        let into = rows[at].source_id.clone();
        record_merged(&mut rows[at].lines[..1], &[(row, into)], |_| None);
    }
}

/// The turn events (see [`rows`]).
fn turns(session: &RehydrateSession, messages: &[RehydrateMessage], rows: &mut [Row]) {
    let thread = session::resume_id(&session.id);
    let written: Vec<bool> = rows.iter().map(|row| !row.lines.is_empty()).collect();
    let written = |i: usize| written[i];
    let prompts: Vec<usize> = (0..messages.len())
        .filter(|&i| messages[i].role == Role::User && written(i) && said(&messages[i]).is_some())
        .collect();
    // Where each prompt's turn starts: the context rows and `turn_context` right before it.
    let mut starts = Vec::with_capacity(prompts.len());
    let mut floor = 0;
    for &prompt in &prompts {
        let mut start = prompt;
        for i in (floor..prompt).rev() {
            let opens = matches!(&messages[i].role, Role::System)
                || messages[i].role == Role::Other("turn_context".to_owned());
            if !written(i) {
                continue;
            }
            if !opens {
                break;
            }
            start = i;
        }
        starts.push(start);
        floor = prompt + 1;
    }
    for (n, (&start, &prompt)) in starts.iter().zip(&prompts).enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(messages.len());
        let Some(last) = (start..end).rev().find(|&i| written(i)) else {
            continue;
        };
        let id = turn_id(thread, &messages[prompt].source_id);
        let begun = messages[start].timestamp;
        rows[start].before.push(line(
            &stamp(begun),
            "event_msg",
            json!({"type": "task_started", "turn_id": id, "root_turn_id": id,
                   "started_at": begun.unix_timestamp(), "model_context_window": null}),
        ));
        let item = |message: &RehydrateMessage, item: Value| {
            let millis = i64::try_from(message.timestamp.unix_timestamp_nanos() / 1_000_000)
                .unwrap_or_default();
            line(
                &stamp(message.timestamp),
                "event_msg",
                json!({"type": "item_completed", "thread_id": thread, "turn_id": id,
                       "item": item, "started_at_ms": millis, "completed_at_ms": millis}),
            )
        };
        // The patches applied in the turn are its file changes.
        for i in start..end {
            if matches!(messages[i].content.as_slice(), [Content::Patch(_)])
                && let Some(change) = rows[i].lines.first_mut()
            {
                change["payload"]["turn_id"] = json!(id);
            }
        }
        let mut answer = None;
        for i in prompt..end {
            let Some(said) = said(&messages[i]).filter(|_| written(i)) else {
                continue;
            };
            let message = &messages[i];
            if i == prompt {
                rows[i].lines.push(item(
                    message,
                    json!({"type": "UserMessage",
                    "id": message.source_id,
                    "content": [{"type": "text", "text": said, "text_elements": []}]}),
                ));
            } else if message.role == Role::Assistant {
                rows[i].before.push(item(
                    message,
                    json!({"type": "AgentMessage",
                    "id": message.source_id, "content": [{"type": "Text", "text": said}]}),
                ));
                answer = Some(said);
            }
        }
        // A turn that failed ends on its failure: that event is the turn's end.
        let ended = &messages[last];
        let failed = matches!(ended.content.as_slice(), [Content::Error(_)]);
        if failed && let Some(end) = rows[last].lines.first_mut() {
            end["payload"]["turn_id"] = json!(id);
            continue;
        }
        rows[last].lines.push(line(
            &stamp(ended.timestamp),
            "event_msg",
            json!({"type": "task_complete", "turn_id": id, "last_agent_message": answer,
                   "started_at": begun.unix_timestamp(),
                   "completed_at": ended.timestamp.unix_timestamp()}),
        ));
    }
}

/// The id of a turn written for the prompt `prompt` of thread `thread`: a UUID drawn from both,
/// so the same rows name their turns the same every time.
fn turn_id(thread: &str, prompt: &str) -> String {
    let hash = xxhash_rust::xxh3::xxh3_128(format!("{thread}\0{prompt}").as_bytes());
    uuid::Uuid::from_u128(hash).to_string()
}

/// The id a line is written under: capture's source id, unless it keyed the line on something
/// else (`keyed_on`: the `call_id` it falls back to) or on its content (`syn-`), when the line
/// is written without one.
fn item_id(message: &RehydrateMessage, keyed_on: Option<&str>) -> Option<String> {
    let id = &message.source_id;
    (!id.starts_with("syn-") && keyed_on != Some(id.as_str())).then(|| id.clone())
}

fn with_id(mut payload: Value, id: Option<String>) -> Value {
    if let Some(id) = id {
        payload["id"] = Value::String(id);
    }
    payload
}

/// The lines one captured message is written as; none for what the rollout cannot carry.
fn message_lines(
    session: &RehydrateSession,
    message: &RehydrateMessage,
    calls: &mut HashMap<String, CallKind>,
) -> Vec<Value> {
    let at = stamp(message.timestamp);
    if let Role::Other(kind) = &message.role {
        return match kind.as_str() {
            // The first line already says it.
            "session_meta" => Vec::new(),
            "turn_context" => vec![line(&at, "turn_context", turn_context(session, message))],
            _ => usage_line(message, &at).into_iter().collect(),
        };
    }
    match message.content.as_slice() {
        [Content::Summary(summary)] => {
            vec![line(
                &at,
                "compacted",
                json!({"message": format!("{COMPACTION_PREAMBLE}{summary}")}),
            )]
        }
        [Content::Error(why)] => {
            let payload = match message.stop_reason {
                Some(StopReason::Aborted) => json!({"type": "turn_aborted", "reason": why}),
                _ => json!({"type": "task_complete", "error": {"message": why}}),
            };
            vec![line(&at, "event_msg", payload)]
        }
        [Content::ToolUse(call)] => {
            vec![line(&at, "response_item", tool_call(message, call, calls))]
        }
        [Content::ToolResult(result)] => {
            vec![line(&at, "response_item", tool_output(message, result, calls))]
        }
        [Content::Patch(patch)] => vec![file_change(session, message, patch, &at)],
        content if !content.is_empty() && content.iter().all(is_message_block) => {
            text_message(message, &at)
        }
        // Reasoning (see the module docs), and shapes no Codex line has.
        _ => Vec::new(),
    }
}

/// The `item_completed` event of a `FileChange` item, which Codex shows an applied patch's diff
/// from on resume: each file as Codex records it (`{"type": "add", "content"}`, `delete` the
/// same, `{"type": "update", "unified_diff", "move_path"}`). Its turn is filled in by [`turns`].
fn file_change(
    session: &RehydrateSession,
    message: &RehydrateMessage,
    patch: &Patch,
    at: &str,
) -> Value {
    let changes: Map<String, Value> = patch
        .files
        .iter()
        .map(|file| {
            let change = match file.change {
                Change::Add => json!({"type": "add", "content": file.side('+')}),
                Change::Delete => json!({"type": "delete", "content": file.side('-')}),
                Change::Update => json!({"type": "update", "unified_diff": file.hunks_text(),
                    "move_path": file.moved_to}),
            };
            (file.path.clone(), change)
        })
        .collect();
    let millis =
        i64::try_from(message.timestamp.unix_timestamp_nanos() / 1_000_000).unwrap_or_default();
    line(
        at,
        "event_msg",
        json!({"type": "item_completed", "thread_id": session::resume_id(&session.id),
            "turn_id": "",
            "item": {"type": "FileChange", "id": patch.call, "changes": changes,
                "status": "completed"},
            "started_at_ms": millis, "completed_at_ms": millis}),
    )
}

fn is_message_block(content: &Content) -> bool {
    matches!(content, Content::Text(_))
}

/// A `message` item, and for what the user said or the model answered, the event Codex shows it
/// from on resume.
fn text_message(message: &RehydrateMessage, at: &str) -> Vec<Value> {
    let (role, block) = match &message.role {
        Role::User => ("user", "input_text"),
        Role::Assistant => ("assistant", "output_text"),
        _ => ("developer", "input_text"),
    };
    let blocks: Vec<Value> = message
        .content
        .iter()
        .filter_map(|content| match content {
            Content::Text(text) => Some(json!({"type": block, "text": text})),
            _ => None,
        })
        .collect();
    // Under its source id even when capture keyed it on its content (`syn-`: a message from
    // before Codex 0.32 had no id): the line now has a timestamp, so its content would hash
    // differently, but capture reads an id the line carries back as it is.
    let item = with_id(
        json!({"type": "message", "role": role, "content": blocks}),
        Some(message.source_id.clone()),
    );
    vec![line(at, "response_item", item)]
}

/// What the user said, or the model answered, in `message`: the text Codex shows it as.
fn said(message: &RehydrateMessage) -> Option<String> {
    if !matches!(message.role, Role::User | Role::Assistant) {
        return None;
    }
    let texts: Vec<&str> = message
        .content
        .iter()
        .map(|content| match content {
            Content::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Option<_>>()?;
    let texts: Vec<&str> = texts.into_iter().filter(|t| !t.is_empty()).collect();
    (!texts.is_empty()).then(|| texts.join("\n"))
}

/// A tool call as the item Codex wrote it as, told apart by the name capture gave the tools
/// Codex runs itself and, for the rest, by its input: a `function_call`'s is a JSON object
/// encoded as a string, a `custom_tool_call`'s (`apply_patch`'s patch) free text.
fn tool_call(
    message: &RehydrateMessage,
    call: &ToolUse,
    calls: &mut HashMap<String, CallKind>,
) -> Value {
    let call_id = call.id.as_ref();
    match call.name.as_str() {
        "local_shell" => with_id(
            json!({"type": "local_shell_call", "call_id": call_id, "status": "completed",
                   "action": call.input}),
            item_id(message, Some(call_id)),
        ),
        // Keyed on their `id`, which is the call's.
        "web_search" => json!({"type": "web_search_call", "id": call_id, "status": "completed",
                               "action": call.input}),
        "image_generation" => json!({"type": "image_generation_call", "id": call_id,
                                     "status": "completed", "revised_prompt": call.input}),
        "tool_search" => {
            calls.insert(call_id.to_owned(), CallKind::ToolSearch);
            with_id(
                json!({"type": "tool_search_call", "call_id": call_id, "status": "completed",
                       "arguments": call.input}),
                item_id(message, Some(call_id)),
            )
        }
        name => {
            let function = call
                .input
                .as_str()
                .is_some_and(|input| serde_json::from_str::<Map<String, Value>>(input).is_ok());
            let (kind, field, item) = if function {
                (CallKind::Function, "arguments", "function_call")
            } else {
                (CallKind::Custom, "input", "custom_tool_call")
            };
            calls.insert(call_id.to_owned(), kind);
            let mut payload = json!({"type": item, "call_id": call_id, "name": name});
            payload[field] = call.input.clone();
            if kind == CallKind::Custom {
                payload["status"] = json!("completed");
            }
            with_id(payload, item_id(message, Some(call_id)))
        }
    }
}

/// A tool's output, as the output item of the kind of call it answers.
fn tool_output(
    message: &RehydrateMessage,
    result: &ToolResult,
    calls: &HashMap<String, CallKind>,
) -> Value {
    let call_id = result.call.as_ref();
    let fallback = format!("{call_id}#out");
    let id = item_id(message, Some(&fallback));
    // Codex reads an output as a string or content items, and a tool search's as a list.
    let output = if result.output.is_null() {
        json!(UNCAPTURED_OUTPUT)
    } else {
        result.output.clone()
    };
    match calls.get(call_id) {
        Some(CallKind::ToolSearch) => with_id(
            json!({"type": "tool_search_output", "call_id": call_id,
                   "status": if result.error { "failed" } else { "completed" },
                   "execution": "client",
                   "tools": if result.output.is_null() { json!([]) } else { output }}),
            id,
        ),
        Some(CallKind::Custom) => with_id(
            json!({"type": "custom_tool_call_output", "call_id": call_id, "output": output}),
            id,
        ),
        _ => with_id(
            json!({"type": "function_call_output", "call_id": call_id, "output": output}),
            id,
        ),
    }
}

/// A `turn_context` line under the model and cwd the captured one had: both are part of the
/// hash capture keyed it on (see the module docs). The policies are not captured; the strictest
/// stand in, and Codex applies the user's current ones to the turns resumed.
fn turn_context(session: &RehydrateSession, message: &RehydrateMessage) -> Value {
    let mut payload = json!({
        "cwd": message.cwd.as_ref().unwrap_or(&session.cwd),
        "approval_policy": "on-request",
        "sandbox_policy": {"type": "read-only"},
        "summary": "auto",
    });
    if let Some(model) = &message.model {
        payload["model"] = json!(model);
    }
    payload
}

/// A usage row as the line it was captured from, which its turn id names (see
/// `CodexMessage::usage_turn`): `token_count:<total key>` or `token_count:<timestamp>:<last
/// key>` a `token_count` event, anything else a `token_usage_record`'s response, or
/// `thread:<total key>` for a record that named none.
fn usage_line(message: &RehydrateMessage, at: &str) -> Option<Value> {
    let usage = message.usage.as_ref()?;
    let turn = message.turn_id.as_deref()?;
    if !message.content.is_empty() {
        return None;
    }
    if let Some(key) = turn.strip_prefix("token_count:") {
        let last = token_usage(usage);
        return Some(match usage_key_fields(key) {
            Some(total) => line(
                at,
                "event_msg",
                json!({"type": "token_count",
                       "info": {"total_token_usage": total, "last_token_usage": last}}),
            ),
            // Keyed on the line's own timestamp: written verbatim, it keys the same again.
            None => {
                let (stamped, _) = key.rsplit_once(':')?;
                line(
                    stamped,
                    "event_msg",
                    json!({"type": "token_count", "info": {"last_token_usage": last}}),
                )
            }
        });
    }
    let mut payload = json!({"usage": token_usage(usage)});
    match turn.strip_prefix("thread:").and_then(usage_key_fields) {
        Some(total) => {
            payload["response_id"] = json!("");
            payload["thread_token_usage"] = total;
        }
        None => payload["response_id"] = json!(turn),
    }
    Some(line(at, "token_usage_record", payload))
}

/// The `TokenUsage` a usage key (`input.cached.output.reasoning.total`) spells out.
fn usage_key_fields(key: &str) -> Option<Value> {
    let fields: Vec<u64> = key.split('.').map(str::parse).collect::<Result<_, _>>().ok()?;
    let [input, cached, output, reasoning, total] = fields.as_slice() else {
        return None;
    };
    Some(json!({
        "input_tokens": input,
        "cached_input_tokens": cached,
        "output_tokens": output,
        "reasoning_output_tokens": reasoning,
        "total_tokens": total,
    }))
}

/// A Codex `TokenUsage` for our [`Usage`], the inverse of capture's: Codex counts cached input
/// and cache writes inside `input_tokens`. Only what was captured is written, so what reads back
/// is what was captured.
fn token_usage(usage: &Usage) -> Value {
    let mut out = Map::new();
    let cached = usage.cache_read.unwrap_or(0) + usage.cache_write.unwrap_or(0);
    let input = usage.input.map(|input| input + cached);
    for (name, value) in [
        ("input_tokens", input),
        ("cached_input_tokens", usage.cache_read),
        ("cache_write_input_tokens", usage.cache_write),
        ("output_tokens", usage.output),
        ("reasoning_output_tokens", usage.reasoning),
    ] {
        if let Some(value) = value {
            out.insert(name.to_owned(), json!(value));
        }
    }
    out.insert("total_tokens".to_owned(), json!(input.unwrap_or(0) + usage.output.unwrap_or(0)));
    Value::Object(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;

    use futures::TryStreamExt;
    use rstest::rstest;

    use super::*;
    use crate::harnesstools::codex::session::{CodexMessage, CodexSession};
    use crate::harnesstools::rehydrate::testing;
    use crate::harnesstools::session::{Message, Session, SessionId};
    use crate::sync::BlockingPool;

    pub fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex").join(name)
    }

    pub async fn read(id: &str, path: PathBuf) -> Vec<CodexMessage> {
        let pool = BlockingPool::new(std::num::NonZeroUsize::MIN);
        CodexSession::open(SessionId::from(id.to_owned()), path, pool)
            .read()
            .try_collect()
            .await
            .unwrap()
    }

    /// The rows capture makes of a rollout's lines, keyed as the daemon keys them
    /// (`message_enricher`): a line's own id, else a hash of every field the row takes from it,
    /// told apart from an identical earlier line by an ordinal. Lines no row is made of are left
    /// out, and a line without a timestamp takes the one before it.
    pub fn captured(session: &str, lines: &[CodexMessage]) -> Vec<RehydrateMessage> {
        let mut last = None;
        let mut seen: HashMap<u64, u32> = HashMap::new();
        let mut rows = Vec::new();
        for m in lines {
            if let Some(at) = m.timestamp() {
                last = Some(at);
            }
            let (content, usage, model, cwd) = (m.content(), m.usage(), m.model(), m.cwd());
            if m.id().is_none()
                && content.is_empty()
                && usage.is_none()
                && m.stop_reason().is_none()
                && model.is_none()
                && cwd.is_none()
                && m.git_branch().is_none()
                && m.title().is_none()
            {
                continue;
            }
            let source_id = match m.id() {
                Some(id) => String::from(id),
                None => {
                    let canonical = json!([
                        session,
                        m.timestamp().map(|at| at.unix_timestamp_nanos().to_string()),
                        m.role(),
                        content,
                        m.title(),
                        model,
                        usage,
                        m.stop_reason(),
                        cwd,
                        m.git_branch(),
                        m.parent_id(),
                        m.parent_session(),
                        m.turn_id(),
                    ]);
                    let hash = xxhash_rust::xxh3::xxh3_64(canonical.to_string().as_bytes());
                    let n = seen.entry(hash).or_insert(0);
                    *n += 1;
                    format!("syn-{hash:016x}-{}", *n - 1)
                }
            };
            rows.push(RehydrateMessage {
                source_id,
                parent_source_id: m.parent_id().map(String::from),
                timestamp: last.unwrap_or(OffsetDateTime::UNIX_EPOCH),
                role: m.role(),
                content,
                model,
                usage,
                stop_reason: m.stop_reason(),
                turn_id: m.turn_id(),
                cwd,
                git_branch: m.git_branch(),
            });
        }
        rows
    }

    pub fn keys(rows: &[RehydrateMessage]) -> Vec<(String, Role, Vec<Content>)> {
        rows.iter().map(|r| (r.source_id.clone(), r.role.clone(), r.content.clone())).collect()
    }

    /// What a rollout cannot carry back (see the module docs): reasoning, and thread names.
    fn carried(row: &RehydrateMessage) -> bool {
        let reasoning = matches!(row.content.as_slice(), [Content::ReasoningSummary { .. }]);
        let title = row.role == Role::Other("event_msg".to_owned())
            && row.content.is_empty()
            && row.usage.is_none();
        !reasoning && !title
    }

    pub fn session(id: &str, messages: Vec<RehydrateMessage>) -> RehydrateSession {
        RehydrateSession {
            id: id.to_owned(),
            title: None,
            cwd: PathBuf::from("/elsewhere/proj"),
            original_cwd: Some(PathBuf::from("/work")),
            git_branch: Some("main".to_owned()),
            model: None,
            started_at: messages.first().map_or(OffsetDateTime::UNIX_EPOCH, |m| m.timestamp),
            messages,
            fork_of: None,
        }
    }

    /// The session a fixture holds: its first line's (own) `session_meta`.
    pub fn own_id(path: &Path) -> String {
        let text = std::fs::read_to_string(path).unwrap();
        let first: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        first["payload"]["id"].as_str().unwrap().to_owned()
    }

    /// Real rollouts, captured, written back and captured again, key the same rows: the same
    /// ids -- native or content-addressed -- roles and content, in the same order, save for what
    /// a rollout cannot carry.
    #[rstest]
    #[case::paginated_with_compaction("paginated-compacted.jsonl")]
    #[case::custom_tools_and_records("session1.jsonl")]
    #[case::legacy_forked_subagent("legacy-forked-subagent.jsonl")]
    #[case::forked_subagent("forked-subagent.jsonl")]
    #[tokio::test]
    async fn a_rehydrated_rollout_recaptures_as_the_same_rows(#[case] name: &str) {
        let id = own_id(&fixture(name));
        let original = captured(&id, &read(&id, fixture(name)).await);
        assert!(original.len() > 5, "the fixture has rows to carry");

        let root = tempfile::tempdir().unwrap();
        let path = write(root.path(), &session(&id, original.clone())).unwrap();
        assert_eq!(session::locate(root.path(), &id), Some(path.clone()));

        assert_paginated(&path);
        let again = captured(&id, &read(&id, path).await);
        let expected: Vec<RehydrateMessage> = original.into_iter().filter(carried).collect();
        pretty_assertions::assert_eq!(keys(&again), keys(&expected));
    }

    /// The rollout at `path` is in Codex's paginated history mode, its lines numbered from 0 on
    /// without a number twice or out of order, as Codex needs to continue it.
    pub fn assert_paginated(path: &Path) -> Vec<u64> {
        let text = std::fs::read_to_string(path).unwrap();
        let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines[0]["type"], "session_meta");
        assert_eq!(lines[0]["payload"]["history_mode"], "paginated");
        let ordinals: Vec<u64> =
            lines.iter().map(|l| l["ordinal"].as_u64().expect("every line numbered")).collect();
        assert!(ordinals.windows(2).all(|w| w[0] < w[1]), "ordinals {ordinals:?} not increasing");
        ordinals
    }

    /// Codex shows a paginated rollout's history from its turn and item events: each prompt's
    /// turn starts at the context before it, holds the prompt and the answers as items, and ends
    /// after its last row (or on its own failure). Capture keeps no row of any of them, and the
    /// turns are named the same every time.
    #[rstest]
    fn turns_are_written_for_codex_to_show() {
        let text = |t: &str| vec![Content::Text(t.to_owned())];
        let mut failed = row("syn-f", 7, Role::Assistant, vec![Content::Error("boom".into())]);
        failed.stop_reason = Some(StopReason::Error);
        let messages = vec![
            row(ID, 0, Role::Other("session_meta".into()), vec![]),
            row("msg_dev", 1, Role::System, text("<permissions instructions>x")),
            row("syn-t1", 2, Role::Other("turn_context".into()), vec![]),
            row("msg_u1", 3, Role::User, text("first")),
            row("msg_a1", 4, Role::Assistant, text("one")),
            row("syn-t2", 5, Role::Other("turn_context".into()), vec![]),
            row("msg_u2", 6, Role::User, text("second")),
            failed,
        ];
        let lines = turn_lines(&messages);
        let event = |l: &Value| l["payload"]["type"].as_str().unwrap_or_default().to_owned();
        let kinds: Vec<String> = lines
            .iter()
            .map(|l| match l["type"].as_str().unwrap() {
                "event_msg" => event(l),
                "response_item" => l["payload"]["id"].as_str().unwrap().to_owned(),
                other => other.to_owned(),
            })
            .collect();
        assert_eq!(kinds, [
            "task_started",
            "msg_dev",
            "turn_context",
            "msg_u1",
            "item_completed",
            "item_completed",
            "msg_a1",
            "task_complete",
            "task_started",
            "turn_context",
            "msg_u2",
            "item_completed",
            "task_complete",
        ]);
        let turn = |l: &Value| l["payload"]["turn_id"].as_str().unwrap().to_owned();
        let first = turn(&lines[0]);
        assert!(lines[..8].iter().filter(|l| l["type"] == "event_msg").all(|l| turn(l) == first));
        let second = turn(&lines[8]);
        assert_ne!(first, second);
        assert_eq!(turn(&lines[12]), second, "the failure ends the turn");
        assert!(lines[12]["payload"]["error"].is_object());
        assert_eq!(lines[4]["payload"]["item"]["type"], "UserMessage");
        assert_eq!(lines[5]["payload"]["item"]["type"], "AgentMessage");
        assert_eq!(lines[7]["payload"]["last_agent_message"], "one");
        assert_eq!(turn_lines(&messages), lines, "the same turns every time");
    }

    /// The lines `messages` are written as, in order, unnumbered.
    fn turn_lines(messages: &[RehydrateMessage]) -> Vec<Value> {
        rows(&session(ID, messages.to_vec()), messages)
            .into_iter()
            .flat_map(|row| row.before.into_iter().chain(row.lines))
            .collect()
    }

    /// Usage rows come back as the lines they were captured from, counted the same.
    #[rstest]
    #[tokio::test]
    async fn usage_recaptures_with_its_counts_and_turn() {
        let name = "paginated-compacted.jsonl";
        let id = own_id(&fixture(name));
        let original = captured(&id, &read(&id, fixture(name)).await);
        let root = tempfile::tempdir().unwrap();
        let path = write(root.path(), &session(&id, original.clone())).unwrap();
        let again = captured(&id, &read(&id, path).await);
        let usage = |rows: &[RehydrateMessage]| -> Vec<(Option<String>, Option<Usage>)> {
            rows.iter()
                .filter(|r| r.usage.is_some())
                .map(|r| (r.turn_id.clone(), r.usage))
                .collect()
        };
        assert!(!usage(&original).is_empty());
        assert_eq!(usage(&again), usage(&original));
    }

    pub fn row(source_id: &str, at: i64, role: Role, content: Vec<Content>) -> RehydrateMessage {
        RehydrateMessage {
            source_id: source_id.to_owned(),
            parent_source_id: None,
            timestamp: OffsetDateTime::from_unix_timestamp(at).unwrap(),
            role,
            content,
            model: None,
            usage: None,
            stop_reason: None,
            turn_id: None,
            cwd: None,
            git_branch: None,
        }
    }

    const ID: &str = "01a0d14f-e276-77d3-b955-89d5b0151306";

    fn tool_use(id: &str, name: &str, input: Value) -> Content {
        Content::ToolUse(ToolUse {
            id: id.to_owned().into(),
            name: name.to_owned(),
            input,
        })
    }

    fn tool_result(call: &str, output: &str, error: bool) -> Content {
        Content::ToolResult(ToolResult {
            call: call.to_owned().into(),
            output: json!(output),
            error,
        })
    }

    /// Rows from older rollouts: items keyed on their `call_id`, a `token_count` keyed on the
    /// running total, and one keyed on its timestamp.
    #[rstest]
    #[tokio::test]
    async fn call_id_and_token_count_keys_come_back() {
        let mut total =
            row("token_count:10.2.5.1.15#usage", 4, Role::Other("event_msg".into()), vec![]);
        total.usage = Some(Usage {
            input: Some(8),
            output: Some(5),
            cache_read: Some(2),
            cache_write: None,
            reasoning: Some(1),
        });
        total.turn_id = Some("token_count:10.2.5.1.15".to_owned());
        let mut stamped = total.clone();
        stamped.source_id = "token_count:1970-01-01T00:00:05.123Z:10.2.5.1.15#usage".to_owned();
        stamped.turn_id = Some("token_count:1970-01-01T00:00:05.123Z:10.2.5.1.15".to_owned());
        let messages = vec![
            row(ID, 0, Role::Other("session_meta".into()), vec![]),
            row("call_1", 1, Role::Assistant, vec![tool_use(
                "call_1",
                "shell",
                json!("{\"command\":[\"ls\"]}"),
            )]),
            row("call_1#out", 2, Role::Tool, vec![tool_result(
                "call_1",
                "Exit code: 1\nOutput:\nnope",
                true,
            )]),
            row("call_2", 3, Role::Assistant, vec![tool_use(
                "call_2",
                "apply_patch",
                json!("*** Begin Patch\n*** End Patch"),
            )]),
            row("call_2#out", 3, Role::Tool, vec![tool_result("call_2", "Done", false)]),
            total,
            stamped,
        ];
        let root = tempfile::tempdir().unwrap();
        let path = write(root.path(), &session(ID, messages.clone())).unwrap();
        let again = captured(ID, &read(ID, path).await);
        pretty_assertions::assert_eq!(keys(&again), keys(&messages));
        assert_eq!(again[5].usage, messages[5].usage);
        assert_eq!(again[6].usage, messages[6].usage);
    }

    /// An applied patch comes back as the `FileChange` item Codex shows its diff from, in the
    /// turn it was applied in, and recaptures as the same row.
    #[rstest]
    #[tokio::test]
    async fn a_patch_comes_back_as_its_file_changes() {
        use crate::harnesstools::session::{FilePatch, Hunk};

        let update = FilePatch {
            path: "/w/a.rs".to_owned(),
            change: Change::Update,
            moved_to: None,
            hunks: vec![Hunk {
                old_start: 2,
                old_lines: 2,
                new_start: 2,
                new_lines: 2,
                lines: vec![" bar".into(), "-baz".into(), "+BAZ".into()],
            }],
        };
        let moved = FilePatch {
            path: "/w/b.rs".to_owned(),
            moved_to: Some("/w/c.rs".to_owned()),
            ..update.clone()
        };
        let patch = Patch {
            call: "call_2".to_owned().into(),
            files: vec![
                update,
                moved,
                FilePatch::added("/w/new.rs".to_owned(), "fn main() {}\n"),
                FilePatch::deleted("/w/old.rs".to_owned(), "gone"),
            ],
        };
        let text = |t: &str| vec![Content::Text(t.to_owned())];
        let messages = vec![
            row(ID, 0, Role::Other("session_meta".into()), vec![]),
            row("msg_u1", 1, Role::User, text("fix it")),
            row("ctc_2", 2, Role::Assistant, vec![tool_use(
                "call_2",
                "apply_patch",
                json!("*** Begin Patch\n*** End Patch"),
            )]),
            row("call_2#patch", 3, Role::Tool, vec![Content::Patch(patch)]),
            row("ctco_2", 3, Role::Tool, vec![tool_result("call_2", "Done", false)]),
            row("msg_a1", 4, Role::Assistant, text("fixed")),
        ];
        let lines = turn_lines(&messages);
        let change = lines.iter().find(|l| l["payload"]["item"]["type"] == "FileChange").unwrap();
        let started = lines.iter().find(|l| l["payload"]["type"] == "task_started").unwrap();
        assert_eq!(change["payload"]["turn_id"], started["payload"]["turn_id"]);
        assert_eq!(change["payload"]["item"]["changes"]["/w/b.rs"]["move_path"], "/w/c.rs");
        assert_eq!(change["payload"]["item"]["changes"]["/w/old.rs"]["content"], "gone");

        let root = tempfile::tempdir().unwrap();
        let path = write(root.path(), &session(ID, messages.clone())).unwrap();
        let again = captured(ID, &read(ID, path).await);
        pretty_assertions::assert_eq!(keys(&again), keys(&messages));
    }

    /// Checks a rollout as strictly as Codex and the Responses API check the history Codex
    /// resumes from it: a `function_call`'s `arguments` is a string of a JSON object, a
    /// `custom_tool_call`'s `input` a string, a `local_shell_call`'s `action` an object; every
    /// output answers a call before it and says something; every call is answered.
    fn assert_the_api_takes(path: &Path) {
        let text = std::fs::read_to_string(path).unwrap();
        let mut open: Vec<String> = Vec::new();
        for line in text.lines() {
            let line: Value = serde_json::from_str(line).unwrap();
            if line["type"] != "response_item" {
                continue;
            }
            let item = &line["payload"];
            let call_id = || item["call_id"].as_str().expect("an item names its call").to_owned();
            match item["type"].as_str().unwrap() {
                "function_call" => {
                    let arguments = item["arguments"].as_str().expect("arguments are a string");
                    let parsed: Value = serde_json::from_str(arguments).expect("arguments parse");
                    assert!(parsed.is_object(), "arguments are no object: {item}");
                    open.push(call_id());
                }
                "custom_tool_call" => {
                    assert!(item["input"].is_string(), "input is no string: {item}");
                    open.push(call_id());
                }
                "local_shell_call" => {
                    assert!(item["action"].is_object(), "action is no object: {item}");
                    open.push(call_id());
                }
                "function_call_output" | "custom_tool_call_output" => {
                    let id = call_id();
                    let at = open.iter().position(|c| *c == id);
                    assert!(at.is_some(), "output {id} answers no call before it");
                    open.remove(at.unwrap());
                    let output = &item["output"];
                    assert!(
                        output.as_str().is_some_and(|o| !o.is_empty()) || output.is_array(),
                        "an output says nothing: {item}"
                    );
                }
                _ => {}
            }
        }
        assert!(open.is_empty(), "calls {open:?} never answered");
    }

    /// What capture syncs keeps no call's input and no output. Written back, each such call is a
    /// note, which Codex and the API take; re-captured, every line reads back under an id already
    /// synced (so nothing is pushed), and the outputs not at all.
    #[rstest]
    #[tokio::test]
    async fn synced_calls_come_back_as_notes_the_api_takes() {
        let name = "session1.jsonl";
        let id = own_id(&fixture(name));
        let synced = testing::synced(captured(&id, &read(&id, fixture(name)).await));
        assert!(testing::uncaptured(&synced) > 0, "the fixture makes calls");
        let root = tempfile::tempdir().unwrap();
        let path = write(root.path(), &session(&id, synced.clone())).unwrap();
        assert_the_api_takes(&path);

        let again = captured(&id, &read(&id, path).await);
        testing::assert_nothing_new(&synced, again.iter().map(|m| m.source_id.as_str()));
        let again = testing::synced(again);
        // Rows left with no content are not written, but for the bookkeeping lines.
        let expected: Vec<RehydrateMessage> = flatten_uncaptured_calls(&synced, &Flatten::Notes {
            host: &|host, _| !host.source_id.starts_with("syn-"),
            own: &|m| !m.source_id.starts_with("syn-"),
        })
        .into_iter()
        .filter(carried)
        .filter(|m| !m.content.is_empty() || matches!(m.role, Role::Other(_)))
        .collect();
        pretty_assertions::assert_eq!(keys(&again), keys(&expected));
        assert!(again.iter().any(|m| matches!(
            m.content.as_slice(),
            [Content::Text(text)] if m.role == Role::Assistant && text.contains("[called `tool`]")
        )));
    }

    /// A note joins the assistant message before it in the turn, under that message's id; with
    /// none, it is a message of its own under its call's id: the `call_id` capture keyed it on.
    /// A call kept with its input stays a call, its output saying it was not captured.
    #[rstest]
    #[tokio::test]
    async fn notes_join_the_message_before_them_or_take_the_calls_id() {
        let messages = vec![
            row(ID, 0, Role::Other("session_meta".into()), vec![]),
            row("msg_u", 1, Role::User, vec![Content::Text("go".to_owned())]),
            row("call_1", 2, Role::Assistant, vec![tool_use("call_1", "shell", Value::Null)]),
            row("call_1#out", 3, Role::Tool, vec![tool_result("call_1", "", false)]),
            row("call_2", 4, Role::Assistant, vec![tool_use("call_2", "shell", Value::Null)]),
            row("call_2#out", 5, Role::Tool, vec![tool_result("call_2", "", false)]),
            row("msg_a", 6, Role::Assistant, vec![Content::Text("Looked.".to_owned())]),
            row("ctc_3", 7, Role::Assistant, vec![tool_use("call_3", "apply_patch", Value::Null)]),
            row("call_4", 8, Role::Assistant, vec![tool_use(
                "call_4",
                "shell",
                json!("{\"command\":[\"ls\"]}"),
            )]),
            row("call_4#out", 9, Role::Tool, vec![Content::ToolResult(ToolResult {
                call: "call_4".to_owned().into(),
                output: Value::Null,
                error: false,
            })]),
            row("call_5", 11, Role::Assistant, vec![tool_use(
                "call_5",
                "local_shell",
                Value::Null,
            )]),
        ];
        let root = tempfile::tempdir().unwrap();
        let path = write(root.path(), &session(ID, messages.clone())).unwrap();
        assert_the_api_takes(&path);
        let again = captured(ID, &read(ID, path).await);
        testing::assert_nothing_new(&messages, again.iter().map(|m| m.source_id.as_str()));
        let text = |t: &str| vec![Content::Text(t.to_owned())];
        pretty_assertions::assert_eq!(keys(&again[1..]), vec![
            ("msg_u".to_owned(), Role::User, text("go")),
            ("call_1".to_owned(), Role::Assistant, text("[ran a shell command] ×2")),
            ("msg_a".to_owned(), Role::Assistant, text("Looked.\n\n[applied a patch]")),
            ("call_4".to_owned(), Role::Assistant, messages[8].content.clone()),
            ("call_4#out".to_owned(), Role::Tool, vec![tool_result(
                "call_4",
                UNCAPTURED_OUTPUT,
                false
            )]),
            ("call_5".to_owned(), Role::Assistant, text("[ran a shell command]")),
        ]);
    }

    #[rstest]
    fn an_existing_rollout_is_never_replaced() {
        let root = tempfile::tempdir().unwrap();
        let first = write(root.path(), &session(ID, vec![])).unwrap();
        let before = std::fs::read(&first).unwrap();

        // Written again, even starting on another day, it is still the same session.
        let mut again =
            session(ID, vec![row("m", 1, Role::User, vec![Content::Text("hi".into())])]);
        again.started_at = OffsetDateTime::from_unix_timestamp(86_400 * 400).unwrap();
        assert!(matches!(
            write(root.path(), &again),
            Err(RehydrateError::AlreadyExists(path)) if path == first
        ));
        assert_eq!(std::fs::read(&first).unwrap(), before);
    }

    /// A rollout whose index row can't be pointed at it is left where it is: Codex may be
    /// resuming the thread from it already.
    #[rstest]
    #[tokio::test]
    async fn a_rollout_the_index_cant_be_pointed_at_is_left_in_place() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("sessions");
        let thread = session::resume_id(ID);
        let other = home.path().join("other.jsonl");
        std::fs::write(&other, "{}\n").unwrap();
        let opts = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(home.path().join("state_5.sqlite"))
            .create_if_missing(true);
        let mut conn =
            <sqlx::SqliteConnection as sqlx::Connection>::connect_with(&opts).await.unwrap();
        for sql in [
            "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL)",
            "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
        ] {
            crate::db::query::<sqlx::Sqlite>(sql)
                .bind(thread)
                .bind(other.to_string_lossy().into_owned())
                .execute(&mut conn)
                .await
                .unwrap();
        }
        <sqlx::SqliteConnection as sqlx::Connection>::close(conn).await.unwrap();

        let err = rehydrate_into(&root, home.path(), &session(ID, vec![])).await.unwrap_err();
        assert!(matches!(&err, RehydrateError::Other(why) if why.contains("left at")), "{err}");
        let left = session::locate(&root, ID).expect("the rollout is still there");
        assert!(left.is_file());
    }

    /// A rollout an earlier restore left behind, when pointing Codex's index at it failed, is
    /// pointed at again on the next restore: the index stops naming the gone rollout.
    #[rstest]
    #[tokio::test]
    async fn a_left_rollout_is_pointed_at_on_the_next_restore() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("sessions");
        let thread = session::resume_id(ID);
        let gone = home.path().join("gone.jsonl");
        let index = home.path().join("state_5.sqlite");
        let opts =
            sqlx::sqlite::SqliteConnectOptions::new().filename(&index).create_if_missing(true);
        let mut conn =
            <sqlx::SqliteConnection as sqlx::Connection>::connect_with(&opts).await.unwrap();
        for sql in [
            "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL)",
            "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
        ] {
            crate::db::query::<sqlx::Sqlite>(sql)
                .bind(thread)
                .bind(gone.to_string_lossy().into_owned())
                .execute(&mut conn)
                .await
                .unwrap();
        }
        // The rollout an earlier restore wrote, before its index update failed.
        let left = write(&root, &session(ID, vec![])).unwrap();

        let err = rehydrate_into(&root, home.path(), &session(ID, vec![])).await.unwrap_err();
        assert!(matches!(&err, RehydrateError::AlreadyExists(path) if *path == left), "{err}");
        let named = crate::db::query_scalar::<sqlx::Sqlite, String>(
            "SELECT rollout_path FROM threads WHERE id = ?1",
        )
        .bind(thread)
        .fetch_one(&mut conn)
        .await
        .unwrap();
        assert_eq!(std::path::Path::new(&named), left);
        <sqlx::SqliteConnection as sqlx::Connection>::close(conn).await.unwrap();
    }

    #[rstest]
    fn an_archived_rollout_counts_as_existing() {
        let home = tempfile::tempdir().unwrap();
        let archive = home.path().join("archived_sessions");
        std::fs::create_dir_all(&archive).unwrap();
        let archived = archive.join(format!("rollout-2026-01-01T00-00-00-{ID}.jsonl"));
        std::fs::write(&archived, "{}\n").unwrap();
        assert!(matches!(
            write(&home.path().join("sessions"), &session(ID, vec![])),
            Err(RehydrateError::AlreadyExists(path)) if path == archived
        ));
    }

    /// The rollout goes where Codex keeps it, named as Codex names it, and nothing else is left
    /// behind: the temporary file it was written as is gone, and one already in the way is
    /// stepped around rather than overwritten.
    #[rstest]
    fn the_rollout_is_written_whole_and_in_place() {
        let root = tempfile::tempdir().unwrap();
        let mut s = session(ID, vec![]);
        s.started_at = OffsetDateTime::from_unix_timestamp(1_790_218_068).unwrap();
        let dir = root.path().join("2026/09/24");
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("rollout-2026-09-24T02-47-48-{ID}.jsonl");
        let squatter = dir.join(format!(".{name}.atuin-tmp.0"));
        std::fs::write(&squatter, "not mine").unwrap();

        let path = write(root.path(), &s).unwrap();
        assert_eq!(path, dir.join(&name));
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, vec![format!(".{name}.atuin-tmp.0"), name]);
        assert_eq!(std::fs::read_to_string(&squatter).unwrap(), "not mine");

        let text = std::fs::read_to_string(&path).unwrap();
        let meta: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(meta["type"], "session_meta");
        assert_eq!(meta["payload"]["id"], ID);
        assert_eq!(meta["payload"]["cwd"], "/elsewhere/proj");
        assert_eq!(meta["payload"]["git"]["branch"], "main");
    }

    #[rstest]
    #[case::not_a_uuid("abc")]
    #[case::a_path("../x")]
    fn an_id_codex_would_not_read_back_is_refused(#[case] id: &str) {
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(write(root.path(), &session(id, vec![])), Err(RehydrateError::Other(_))));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
