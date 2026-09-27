//! A Codex rollout written back out from captured messages (see [`rehydrate`]).
//!
//! # What survives
//!
//! Every row capture keys on a native id is written back under that id, so re-capturing the
//! rollout finds the rows already synced:
//!
//! - messages (`response_item` `message`), under their `id`; a system row is written as a
//!   `developer` message, or as the `user` message Codex injected it as when its text is one of
//!   Codex's contextual fragments (AGENTS.md, `<environment_context>`, ...);
//! - tool calls and their outputs (`function_call`, `custom_tool_call`, `local_shell_call`,
//!   `web_search_call`, `image_generation_call`, `tool_search_call` and their `*_output`s), under
//!   their `id`, or with none when capture keyed them on their `call_id` (`<call_id>#out` for an
//!   output);
//! - usage, as the line it was captured from: a `token_usage_record` under its response (or the
//!   thread total it was keyed on), a `token_count` under the running total its key spells out;
//! - the session itself, as the rollout's `session_meta` (under the session id).
//!
//! Rows capture keyed on their content (`syn-<hash>`: a line without an id) are written back
//! with the very fields that content hash covers, so they hash the same again: `turn_context`
//! (its model and original cwd), `compacted` summaries, and a turn's failure (`turn_aborted`, or
//! a `task_complete` carrying an error). What they cannot get back is a line that had no
//! timestamp of its own (only rollouts from before Codex 0.32 have those): it now has one, so it
//! hashes differently and is captured again as a new row.
//!
//! Codex's `user_message` / `agent_message` events are written beside each message so that Codex
//! shows the history when the session is resumed; capture keeps no row for those.
//!
//! # What is dropped
//!
//! - **Reasoning.** Capture keeps only that a model reasoned ([`Content::ReasoningSummary`]),
//!   never its summary or its `encrypted_content`, which only the model can read. A `reasoning`
//!   item without that ciphertext is worse than none: resuming sends it back to the Responses
//!   API by `id`, which a store-less request cannot resolve. The model's earlier reasoning is not
//!   something it needs to continue, so these items are left out, and re-capture has no row to
//!   compare them with.
//! - **Tool calls captured without their input** (capture keeps only a call's name now). A
//!   `function_call`'s `arguments` must be a string of JSON, a `custom_tool_call`'s `input` a
//!   string, a `local_shell_call`'s `action` an object: Codex cannot read the line back, let alone
//!   send it. Each becomes a note (`[ran a shell command]`, as a continuation writes it) and its
//!   output is dropped ([`Flatten::Notes`]). A note joins the text of the assistant `message`
//!   before it in the turn (the same note several times in a row counted, `×3`), written under
//!   that message's id; with none, it is an assistant `message` of its own under the call's id
//!   (its `id`, or its `call_id` when capture keyed it on that). Re-captured, either reads back
//!   under a source id capture already holds, so nothing is pushed, and the outputs are not there
//!   to capture again. A line keyed on its content (`syn-`) would hash differently once changed:
//!   a note never joins one, and a call keyed so (none in practice: every call has a `call_id`)
//!   is left out.
//! - **Tool output**: capture keeps none now; a call kept with its input (older records) gets
//!   [`UNCAPTURED_OUTPUT`] as its output (an empty `tools` list for a tool search).
//! - Rows whose line carries nothing Codex needs back: thread names (Codex keeps those in its
//!   `session_index.jsonl` now) and other events.
//! - Content kinds a line of the kind cannot carry (text inside a tool call, and so on).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

use super::session::{self, archive_of, is_contextual_user_text, session_id_of};
use crate::harnesstools::rehydrate::{
    Flatten, RehydrateError, RehydrateMessage, RehydrateSession, UNCAPTURED_OUTPUT,
    flatten_uncaptured_calls,
};
use crate::harnesstools::resume::is_plain_name;
use crate::harnesstools::session::{Content, Role, StopReason, ToolResult, ToolUse, Usage};

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
/// live or archived; the file is written whole under a temporary name and linked into place, so
/// no reader ever sees half of it and an existing file is never replaced.
pub async fn rehydrate(session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
    let root = session::default_root();
    let session = session.clone();
    tokio::task::spawn_blocking(move || write(&root, &session))
        .await
        .map_err(|err| RehydrateError::Other(err.to_string()))?
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
    let mut text = String::new();
    for line in rollout(session) {
        text.push_str(&line.to_string());
        text.push('\n');
    }
    let path = dir.join(&name);
    create_new(&path, text.as_bytes())?;
    Ok(path)
}

/// Write `path` whole or not at all, and never over an existing file: `bytes` go to a temporary
/// file beside it (named so that no watcher takes it for a rollout), which is then hard-linked
/// into place. Where hard links are unsupported, the temporary file is renamed instead, after
/// checking that nothing is there.
fn create_new(path: &Path, bytes: &[u8]) -> Result<(), RehydrateError> {
    let dir = path.parent().ok_or(RehydrateError::NoDataDir)?;
    let name = path.file_name().ok_or(RehydrateError::NoDataDir)?.to_string_lossy();
    let (tmp, mut file) = (0u32..)
        .find_map(|n| {
            let tmp = dir.join(format!(".{name}.atuin-tmp.{n}"));
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
                Ok(file) => Some(Ok((crate::fs::RemoveOnDropPath(tmp), file))),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(err) => Some(Err(err)),
            }
        })
        .expect("an unbounded range always finds a free name")?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    match std::fs::hard_link(&*tmp, path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(RehydrateError::AlreadyExists(path.to_path_buf()))
        }
        Err(err) if err.kind() == std::io::ErrorKind::Unsupported => {
            if path.exists() {
                return Err(RehydrateError::AlreadyExists(path.to_path_buf()));
            }
            std::fs::rename(&*tmp, path)?;
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

/// The time as Codex stamps a line: RFC 3339 in UTC, as precise as it was captured.
fn stamp(at: OffsetDateTime) -> String {
    at.to_offset(UtcOffset::UTC).format(&Rfc3339).unwrap_or_default()
}

/// A rollout line (codex-rs `RolloutLine`).
fn line(at: &str, kind: &str, payload: Value) -> Value {
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

/// The rollout's lines: its `session_meta`, then each message's.
///
/// The `session_meta` is the session as it is resumed here: its cwd is this machine's. It names
/// no `model_provider`, which capture does not keep: Codex takes a rollout without one for its
/// configured provider, where one naming another would be hidden from `codex resume`'s picker.
/// Its `cli_version` is a development build's, which capture (like Codex) reads as a Codex
/// older than per-call usage records, so that `token_count` lines count as usage again when that
/// is what they were captured as; `token_usage_record` lines are read as records either way.
fn rollout(session: &RehydrateSession) -> Vec<Value> {
    let thread = session::resume_id(&session.id);
    let mut meta = json!({
        "id": thread,
        "timestamp": stamp(session.started_at),
        "cwd": session.cwd,
        "originator": "codex_cli_rs",
        "cli_version": "0.0.0",
        "source": "cli",
    });
    if let Some(branch) = &session.git_branch {
        meta["git"] = json!({"branch": branch});
    }
    let mut lines = vec![line(&stamp(session.started_at), "session_meta", meta)];
    let mut calls = HashMap::new();
    // A line keyed on its content would be captured again as a new row once its content changes.
    let keyed = |m: &RehydrateMessage| !m.source_id.starts_with("syn-");
    let messages = flatten_uncaptured_calls(&session.messages, &Flatten::Notes {
        host: &|host, _| keyed(host),
        own: &keyed,
    });
    for message in &messages {
        lines.extend(message_lines(session, message, &mut calls));
    }
    lines
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
        content if !content.is_empty() && content.iter().all(is_message_block) => {
            text_message(message, &at)
        }
        // Reasoning (see the module docs), and shapes no Codex line has.
        _ => Vec::new(),
    }
}

fn is_message_block(content: &Content) -> bool {
    matches!(content, Content::Text(_) | Content::Other(_))
}

/// A `message` item, and for what the user said or the model answered, the event Codex shows it
/// from on resume.
fn text_message(message: &RehydrateMessage, at: &str) -> Vec<Value> {
    let texts: Vec<&str> = message
        .content
        .iter()
        .filter_map(|content| match content {
            Content::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let (role, block) = match &message.role {
        Role::User => ("user", "input_text"),
        Role::Assistant => ("assistant", "output_text"),
        // Context Codex injected as a user message reads back as the system's again.
        _ if texts.iter().any(|text| is_contextual_user_text(text)) => ("user", "input_text"),
        _ => ("developer", "input_text"),
    };
    let blocks: Vec<Value> = message
        .content
        .iter()
        .filter_map(|content| match content {
            Content::Text(text) => Some(json!({"type": block, "text": text})),
            // A block capture kept whole (an image, say): back as it was.
            Content::Other(value) => Some(value.clone()),
            _ => None,
        })
        .collect();
    let item = with_id(
        json!({"type": "message", "role": role, "content": blocks}),
        item_id(message, None),
    );
    let mut lines = vec![line(at, "response_item", item)];
    let said = texts.join("\n");
    match message.role {
        Role::User => lines.push(line(
            at,
            "event_msg",
            json!({"type": "user_message", "message": said, "images": []}),
        )),
        Role::Assistant => {
            lines.push(line(at, "event_msg", json!({"type": "agent_message", "message": said})));
        }
        _ => {}
    }
    lines
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
mod tests {
    use std::collections::HashMap;

    use futures::TryStreamExt;
    use rstest::rstest;

    use super::*;
    use crate::harnesstools::codex::session::{CodexMessage, CodexSession};
    use crate::harnesstools::rehydrate::testing;
    use crate::harnesstools::session::{Message, Session, SessionId};
    use crate::sync::BlockingPool;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex").join(name)
    }

    async fn read(id: &str, path: PathBuf) -> Vec<CodexMessage> {
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
    fn captured(session: &str, lines: &[CodexMessage]) -> Vec<RehydrateMessage> {
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

    fn keys(rows: &[RehydrateMessage]) -> Vec<(String, Role, Vec<Content>)> {
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

    fn session(id: &str, messages: Vec<RehydrateMessage>) -> RehydrateSession {
        RehydrateSession {
            id: id.to_owned(),
            title: None,
            cwd: PathBuf::from("/elsewhere/proj"),
            original_cwd: Some(PathBuf::from("/work")),
            git_branch: Some("main".to_owned()),
            model: None,
            started_at: messages.first().map_or(OffsetDateTime::UNIX_EPOCH, |m| m.timestamp),
            messages,
        }
    }

    /// The session a fixture holds: its first line's (own) `session_meta`.
    fn own_id(path: &Path) -> String {
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

        let again = captured(&id, &read(&id, path).await);
        let expected: Vec<RehydrateMessage> = original.into_iter().filter(carried).collect();
        pretty_assertions::assert_eq!(keys(&again), keys(&expected));
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

    fn row(source_id: &str, at: i64, role: Role, content: Vec<Content>) -> RehydrateMessage {
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
