//! Writing a Grok Build session back out from captured messages, as
//! `<sessions>/<encoded cwd>/<session>/{summary.json,chat_history.jsonl,updates.jsonl,.atuin.json}`.
//!
//! # Keying, ordering, and rewrite resilience
//!
//! Grok Build rewrites `chat_history.jsonl` on `--resume`: it prepends its own `{"type":"system"}`
//! prompt when none is present (an empty `system` line would replace Grok's system prompt with an
//! empty one, so none is written here) and strips unknown JSON fields from every line. Therefore:
//!
//! - `chat_history.jsonl` contains only standard Grok Build fields (`type`, `content`,
//!   `prompt_index`, `synthetic_reason`, `tool_calls`, `tool_call_id`, `model_id`).
//! - Conversational lines are keyed deterministically by their preceding `gk-` key, role, and
//!   content (`line_key` in [`super::session`]). When a written row's synced `source_id` differs
//!   from its content key (such as a cross-harness continuation minted with UUIDs, or a run of
//!   uncaptured tool calls flattened into a note), `.atuin.json` beside `chat_history.jsonl` maps
//!   that content key back to `source_id` and records merged rows.
//! - `.atuin.json`, `summary.json`, and `updates.jsonl` are written **before** `chat_history.jsonl`
//!   is published (and sidecar write errors fail the operation), so a running capture daemon never
//!   observes a new transcript line before its ID mapping is on disk.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use time::OffsetDateTime;
use time::macros::format_description;

use super::session::{
    AtuinSidecar, SIDECAR_FILE, SUMMARY_FILE, TRANSCRIPT_FILE, UPDATES_FILE, default_root,
    encode_cwd, inferred_tool_error, line_key, locate, meta_id,
};
use crate::harnesstools::rehydrate::{
    RehydrateError, RehydrateMessage, RehydrateSession, UNCAPTURED_OUTPUT, merge_uncaptured_runs,
};
use crate::harnesstools::resume;
use crate::harnesstools::session::{Content, Role, ToolCallId, ToolResult, ToolUse};
use crate::path::PathBufExt as _;

/// Rendered files for a full restore or incremental append/switch.
#[derive(Debug, Default)]
pub(crate) struct Rendered {
    pub chat_lines: Vec<Value>,
    pub update_lines: Vec<Value>,
    pub written_source_ids: Vec<String>,
    pub sidecar: AtuinSidecar,
}

/// Write `session` under Grok Build's sessions directory (`$GROK_HOME/sessions`, else
/// `~/.grok/sessions`), keyed by `session.cwd`.
pub async fn rehydrate(session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
    let (root, grok_home, session) = (default_root(), super::home(), session.clone());
    tokio::task::spawn_blocking(move || rehydrate_into(&root, &grok_home, &session))
        .await
        .map_err(|err| RehydrateError::Other(err.to_string()))?
}

/// [`rehydrate`] under `root` (`<grok_home>/sessions`).
pub(crate) fn rehydrate_into(
    root: &Path,
    grok_home: &Path,
    session: &RehydrateSession,
) -> Result<PathBuf, RehydrateError> {
    if !resume::is_plain_name(&session.id) {
        return Err(RehydrateError::Other(format!("not a session id: {:?}", session.id)));
    }
    if let Some(existing) = locate(root, &session.id) {
        return Err(RehydrateError::AlreadyExists(existing));
    }
    let sdir = root.join(encode_cwd(&session.cwd)).joined(&session.id);
    std::fs::create_dir_all(&sdir)?;

    let mut prev_gk = None;
    let meta = meta_id(&session.id);
    let rendered = render_messages(&session.id, &session.messages, &mut prev_gk, 0, Some(&meta));
    let body = join_jsonl(&rendered.chat_lines);
    let updates_body = join_jsonl(&rendered.update_lines);
    let summary = summary_json(session, grok_home, rendered.chat_lines.len());

    let transcript_path = sdir.join(TRANSCRIPT_FILE);
    let summary_path = sdir.join(SUMMARY_FILE);
    let updates_path = sdir.join(UPDATES_FILE);
    let sidecar_path = sdir.joined(SIDECAR_FILE);

    // Publish `.atuin.json`, `updates.jsonl`, and `summary.json` BEFORE `chat_history.jsonl` so a
    // running capture watcher never reads transcript lines before their ID mappings and metadata
    // are on disk.
    if !rendered.sidecar.is_empty() {
        let sidecar_bytes = serde_json::to_vec_pretty(&rendered.sidecar)
            .map_err(|err| RehydrateError::Other(err.to_string()))?;
        crate::fs::write_new(&sidecar_path, &sidecar_bytes).map_err(|err| {
            if err.kind() == std::io::ErrorKind::AlreadyExists {
                RehydrateError::AlreadyExists(sidecar_path.clone())
            } else {
                err.into()
            }
        })?;
    }

    if !updates_body.is_empty()
        && let Err(err) = crate::fs::write_new(&updates_path, updates_body.as_bytes())
    {
        let _ = std::fs::remove_file(&sidecar_path);
        return Err(if err.kind() == std::io::ErrorKind::AlreadyExists {
            RehydrateError::AlreadyExists(updates_path)
        } else {
            err.into()
        });
    }

    let summary_bytes = serde_json::to_vec_pretty(&summary)
        .map_err(|err| RehydrateError::Other(err.to_string()))?;
    if let Err(err) = crate::fs::write_new(&summary_path, &summary_bytes) {
        let _ = std::fs::remove_file(&sidecar_path);
        let _ = std::fs::remove_file(&updates_path);
        return Err(if err.kind() == std::io::ErrorKind::AlreadyExists {
            RehydrateError::AlreadyExists(summary_path)
        } else {
            err.into()
        });
    }

    if let Err(err) = crate::fs::write_new(&transcript_path, body.as_bytes()) {
        let _ = std::fs::remove_file(&sidecar_path);
        let _ = std::fs::remove_file(&updates_path);
        let _ = std::fs::remove_file(&summary_path);
        return Err(if err.kind() == std::io::ErrorKind::AlreadyExists {
            RehydrateError::AlreadyExists(transcript_path)
        } else {
            err.into()
        });
    }

    Ok(transcript_path)
}

pub(crate) fn join_jsonl(lines: &[Value]) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(&line.to_string());
        out.push('\n');
    }
    out
}

pub(crate) fn timestamp(at: OffsetDateTime) -> String {
    let format =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]Z");
    at.to_offset(time::UtcOffset::UTC).format(&format).unwrap_or_default()
}

/// Build the `summary.json` object Grok Build reads to list and reopen a session.
#[must_use]
pub(crate) fn summary_json(
    session: &RehydrateSession,
    grok_home: &Path,
    num_chat_messages: usize,
) -> Value {
    let updated_at =
        session.messages.iter().map(|m| m.timestamp).max().unwrap_or(session.started_at);
    let title = session.title.clone().unwrap_or_default();
    let hex_suffix: String = session
        .id
        .chars()
        .filter(char::is_ascii_hexdigit)
        .chain(std::iter::repeat('0'))
        .take(32)
        .collect();
    let mut summary = json!({
        "info": {
            "id": session.id,
            "cwd": session.cwd,
        },
        "agent_id": format!("ag1.{hex_suffix}"),
        "attempt_id": format!("at1.{hex_suffix}"),
        "session_summary": title,
        "created_at": timestamp(session.started_at),
        "updated_at": timestamp(updated_at),
        "last_active_at": timestamp(updated_at),
        "num_messages": num_chat_messages,
        "num_chat_messages": num_chat_messages,
        "current_model_id": session.model.clone().unwrap_or_default(),
        "next_trace_turn": 1,
        "chat_format_version": 1,
        "grok_home": grok_home,
        "sandbox_profile": "off",
    });
    if let Some(title) = session.title.as_deref().filter(|t| !t.trim().is_empty()) {
        summary["generated_title"] = json!(title);
    }
    if let Some(fork_of) = &session.fork_of {
        summary["parent_session_id"] = json!(fork_of.id);
        summary["forked_at"] = json!(timestamp(session.started_at));
    }
    summary
}

fn is_session_bookkeeping_row(session_id: &str, row: &RehydrateMessage) -> bool {
    row.source_id == session_id
        || row.source_id == meta_id(session_id)
        || row.source_id.starts_with("atuin-")
}

/// Render `messages` into pure Grok `chat_history.jsonl` lines, `updates.jsonl` lines, and
/// `.atuin.json` sidecar mappings, continuing `prev_gk` and `next_prompt_index` when appending
/// onto an existing transcript. A row with nothing to write is merged into the line before it:
/// the last written, else `after`, the line the rows follow (the session's metadata row, for a
/// transcript written whole), so the transcript holds every row of the session.
pub(crate) fn render_messages(
    session_id: &str,
    messages: &[RehydrateMessage],
    prev_gk: &mut Option<String>,
    mut next_prompt_index: u64,
    after: Option<&str>,
) -> Rendered {
    let (flattened, mut merged_into) = merge_uncaptured_runs(messages);
    let mut rendered = Rendered::default();
    let mut last_written: Option<String> = after.map(str::to_owned);

    for row in &flattened {
        if row.role == Role::System && row.content.is_empty() {
            if !is_session_bookkeeping_row(session_id, row)
                && let Some(ancestor) = &last_written
            {
                merged_into.entry(row.source_id.clone()).or_insert_with(|| ancestor.clone());
            }
            continue;
        }

        let pieces = render_row_pieces(session_id, row, &mut next_prompt_index);
        if pieces.is_empty() {
            if let Some(ancestor) = &last_written {
                merged_into.entry(row.source_id.clone()).or_insert_with(|| ancestor.clone());
            }
            continue;
        }

        for (idx, piece) in pieces.into_iter().enumerate() {
            let target_source_id = if idx == 0 {
                row.source_id.clone()
            } else {
                format!("{}#{idx}", row.source_id)
            };
            if let Some(gk) = line_key(prev_gk.as_deref(), &piece.read_role, &piece.read_content) {
                *prev_gk = Some(gk.clone());
                if gk != target_source_id {
                    rendered.sidecar.ids.insert(gk, target_source_id.clone());
                }
            }
            rendered.chat_lines.push(piece.chat_line);
            rendered.update_lines.extend(piece.update_lines);
            rendered.written_source_ids.push(target_source_id.clone());
            last_written = Some(target_source_id);
        }
    }

    for (dropped, into) in merged_into {
        if dropped != into {
            rendered.sidecar.merged.insert(dropped, into);
        }
    }

    rendered
}

struct Piece {
    chat_line: Value,
    update_lines: Vec<Value>,
    read_role: Role,
    read_content: Vec<Content>,
}

fn update_envelope(session_id: &str, at: OffsetDateTime, mut update: Value) -> Value {
    let agent_ms = i64::try_from(at.unix_timestamp_nanos() / 1_000_000)
        .unwrap_or_else(|_| at.unix_timestamp().saturating_mul(1_000));
    match update.get_mut("_meta") {
        Some(Value::Object(meta)) => {
            meta.entry("agentTimestampMs").or_insert_with(|| json!(agent_ms));
        }
        _ => {
            update["_meta"] = json!({"agentTimestampMs": agent_ms});
        }
    }
    json!({
        "timestamp": at.unix_timestamp(),
        "method": "session/update",
        "params": {
            "sessionId": session_id,
            "update": update,
        },
    })
}

fn render_row_pieces(
    session_id: &str,
    row: &RehydrateMessage,
    next_prompt_index: &mut u64,
) -> Vec<Piece> {
    match &row.role {
        Role::User => {
            let texts: Vec<String> = row
                .content
                .iter()
                .filter_map(|c| match c {
                    Content::Text(t) if !t.is_empty() => Some(t.clone()),
                    Content::Summary(s) if !s.is_empty() => Some(s.clone()),
                    _ => None,
                })
                .collect();
            if texts.is_empty() {
                return Vec::new();
            }
            let idx = *next_prompt_index;
            *next_prompt_index += 1;
            let blocks: Vec<Value> = texts
                .iter()
                .map(|t| json!({"type": "text", "text": format!("<user_query>\n{t}\n</user_query>")}))
                .collect();
            let joined = texts.join("\n\n");
            let read_content = texts.into_iter().map(Content::Text).collect();
            vec![Piece {
                chat_line: json!({
                    "type": "user",
                    "content": blocks,
                    "prompt_index": idx,
                }),
                update_lines: vec![update_envelope(
                    session_id,
                    row.timestamp,
                    json!({
                        "sessionUpdate": "user_message_chunk",
                        "content": {"type": "text", "text": joined},
                        "_meta": {"promptIndex": idx},
                    }),
                )],
                read_role: Role::User,
                read_content,
            }]
        }
        Role::System | Role::Other(_) => {
            let texts: Vec<String> = row
                .content
                .iter()
                .filter_map(|c| match c {
                    Content::Text(t) if !t.is_empty() => Some(t.clone()),
                    Content::Summary(s) if !s.is_empty() => Some(s.clone()),
                    _ => None,
                })
                .collect();
            if texts.is_empty() {
                return Vec::new();
            }
            let blocks: Vec<Value> =
                texts.iter().map(|t| json!({"type": "text", "text": t})).collect();
            let read_content = texts.into_iter().map(Content::Text).collect();
            vec![Piece {
                chat_line: json!({
                    "type": "user",
                    "content": blocks,
                    "synthetic_reason": "system_reminder",
                }),
                update_lines: Vec::new(),
                read_role: Role::System,
                read_content,
            }]
        }
        Role::Assistant => {
            let mut text_parts: Vec<String> = Vec::new();
            let mut calls: Vec<&ToolUse> = Vec::new();
            for item in &row.content {
                match item {
                    Content::Text(t) if !t.is_empty() => text_parts.push(t.clone()),
                    Content::Summary(s) if !s.is_empty() => text_parts.push(s.clone()),
                    Content::Error(e) if !e.is_empty() => text_parts.push(format!("[error: {e}]")),
                    Content::ToolUse(call) if !call.input.is_null() => calls.push(call),
                    _ => {}
                }
            }
            if text_parts.is_empty() && calls.is_empty() {
                return Vec::new();
            }
            let content_str = text_parts.join("\n\n");
            let mut chat_line = json!({
                "type": "assistant",
                "content": content_str,
            });
            let mut read_content = Vec::new();
            let mut update_lines = Vec::new();
            if !content_str.is_empty() {
                read_content.push(Content::Text(content_str.clone()));
                update_lines.push(update_envelope(
                    session_id,
                    row.timestamp,
                    json!({
                        "sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": content_str},
                    }),
                ));
            }
            if !calls.is_empty() {
                let mut encoded_calls = Vec::with_capacity(calls.len());
                for call in calls {
                    let args_str = match &call.input {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    let parsed_input = serde_json::from_str::<Value>(&args_str)
                        .unwrap_or_else(|_| Value::String(args_str.clone()));
                    encoded_calls.push(json!({
                        "id": call.id.as_ref(),
                        "name": call.name,
                        "arguments": args_str,
                    }));
                    update_lines.push(update_envelope(
                        session_id,
                        row.timestamp,
                        json!({
                            "sessionUpdate": "tool_call",
                            "toolCallId": call.id.as_ref(),
                            "title": call.name,
                            "rawInput": parsed_input,
                        }),
                    ));
                    read_content.push(Content::ToolUse(ToolUse {
                        id: ToolCallId::from(call.id.as_ref().to_owned()),
                        name: call.name.clone(),
                        input: parsed_input,
                    }));
                }
                chat_line["tool_calls"] = json!(encoded_calls);
            }
            if let Some(model) = row.model.as_deref().filter(|m| !m.is_empty()) {
                chat_line["model_id"] = json!(model);
            }
            vec![Piece {
                chat_line,
                update_lines,
                read_role: Role::Assistant,
                read_content,
            }]
        }
        Role::Tool => {
            let results: Vec<&ToolResult> = row
                .content
                .iter()
                .filter_map(|c| match c {
                    Content::ToolResult(r) => Some(r),
                    _ => None,
                })
                .collect();
            let mut pieces = Vec::with_capacity(results.len());
            for result in results {
                let output_val = match &result.output {
                    Value::Null => json!(UNCAPTURED_OUTPUT),
                    other => other.clone(),
                };
                let text_preview = result
                    .output_text()
                    .map_or_else(|| UNCAPTURED_OUTPUT.to_owned(), std::borrow::Cow::into_owned);
                let inferred_err = inferred_tool_error(&output_val);
                let status = if result.error || inferred_err {
                    "failed"
                } else {
                    "completed"
                };
                pieces.push(Piece {
                    chat_line: json!({
                        "type": "tool_result",
                        "tool_call_id": result.call.as_ref(),
                        "content": output_val,
                    }),
                    update_lines: vec![update_envelope(
                        session_id,
                        row.timestamp,
                        json!({
                            "sessionUpdate": "tool_call_update",
                            "toolCallId": result.call.as_ref(),
                            "status": status,
                            "content": [{"type": "content", "content": {"type": "text", "text": text_preview}}],
                        }),
                    )],
                    read_role: Role::Tool,
                    read_content: vec![Content::ToolResult(ToolResult {
                        call: result.call.clone(),
                        output: output_val,
                        error: inferred_err,
                    })],
                });
            }
            pieces
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use futures::TryStreamExt;
    use rstest::rstest;

    use super::*;
    use crate::harnesstools::grok::session::GrokSession;
    use crate::harnesstools::rehydrate::ForkOf;
    use crate::harnesstools::session::{Message, ParentKind, Session, SessionId};
    use crate::sync::BlockingPool;

    fn sample_msg(id: &str, role: Role, content: Vec<Content>, at: i64) -> RehydrateMessage {
        RehydrateMessage {
            source_id: id.to_owned(),
            parent_source_id: None,
            timestamp: OffsetDateTime::from_unix_timestamp(at).unwrap(),
            role,
            content,
            model: None,
            provider: None,
            usage: None,
            stop_reason: None,
            turn_id: None,
            cwd: None,
            git_branch: None,
        }
    }

    #[rstest]
    #[tokio::test]
    async fn rehydrate_emits_no_system_row_writes_updates_and_survives_grok_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sessions");
        let sid = "01a18221-a7b8-75e0-b6ef-b2a5cc95d2c9";
        let session = RehydrateSession {
            id: sid.to_owned(),
            title: Some("Restore test".to_owned()),
            cwd: PathBuf::from("/work/proj"),
            original_cwd: Some(PathBuf::from("/remote/proj")),
            git_branch: Some("main".to_owned()),
            model: None,
            started_at: OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
            messages: vec![
                sample_msg(
                    "u1",
                    Role::User,
                    vec![Content::Text("hello grok".to_owned())],
                    1_700_000_010,
                ),
                sample_msg(
                    "a1",
                    Role::Assistant,
                    vec![
                        Content::Text("checking".to_owned()),
                        Content::ToolUse(ToolUse {
                            id: ToolCallId::from("c1".to_owned()),
                            name: "run_terminal_command".to_owned(),
                            input: json!({"command": "ls"}),
                        }),
                    ],
                    1_700_000_020,
                ),
                sample_msg(
                    "t1",
                    Role::Tool,
                    vec![Content::ToolResult(ToolResult {
                        call: ToolCallId::from("c1".to_owned()),
                        output: json!("exit: 0\na.txt\n"),
                        error: false,
                    })],
                    1_700_000_025,
                ),
                sample_msg(
                    "a2",
                    Role::Assistant,
                    vec![Content::Text("all set".to_owned())],
                    1_700_000_030,
                ),
            ],
            fork_of: Some(ForkOf {
                id: "01a18220-0000-7000-8000-000000000000".to_owned(),
                atuin_id: None,
                path: None,
            }),
        };

        let path = rehydrate_into(&root, dir.path(), &session).unwrap();
        assert_eq!(locate(&root, &session.id), Some(path.clone()));

        let raw_lines: Vec<Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(raw_lines.iter().all(|l| l["type"] != "system"));
        assert!(raw_lines.iter().all(|l| l.get("atuin_id").is_none()));
        assert!(raw_lines.iter().all(|l| l.get("timestamp").is_none()));

        let summary: Value =
            serde_json::from_slice(&std::fs::read(path.with_file_name(SUMMARY_FILE)).unwrap())
                .unwrap();
        assert_eq!(summary["current_model_id"], json!(""));
        assert!(path.with_file_name(UPDATES_FILE).is_file());
        assert!(path.with_file_name(SIDECAR_FILE).is_file());

        let pool = BlockingPool::new(NonZeroUsize::MIN);
        let before_rewrite: Vec<_> =
            GrokSession::open(SessionId::from(sid.to_owned()), path.clone(), pool.clone())
                .read()
                .try_collect()
                .await
                .unwrap();
        let before_ids: Vec<_> = before_rewrite.iter().map(|m| m.id().unwrap()).collect();
        assert_eq!(
            before_rewrite[0].parent_session(),
            Some(SessionId::from("01a18220-0000-7000-8000-000000000000".to_owned()))
        );
        assert_eq!(before_rewrite[0].parent_kind(), Some(ParentKind::Fork));
        assert_eq!(before_rewrite[2].timestamp().unwrap().unix_timestamp(), 1_700_000_010);
        assert_eq!(before_rewrite[5].timestamp().unwrap().unix_timestamp(), 1_700_000_030);

        let mut rewritten =
            vec![json!({"type": "system", "content": "x".repeat(5059)}).to_string()];
        rewritten.extend(raw_lines.iter().map(Value::to_string));
        rewritten.push(
            json!({
                "type": "user",
                "content": [{"type": "text", "text": "<user_query>\nfollow up\n</user_query>"}],
                "prompt_index": 1,
            })
            .to_string(),
        );
        rewritten.push(json!({"type": "assistant", "content": "second reply"}).to_string());
        std::fs::write(&path, rewritten.join("\n") + "\n").unwrap();

        let after_rewrite: Vec<_> = GrokSession::open(SessionId::from(sid.to_owned()), path, pool)
            .read()
            .try_collect()
            .await
            .unwrap();
        let after_ids: Vec<_> = after_rewrite.iter().map(|m| m.id().unwrap()).collect();

        assert_eq!(&after_ids[..before_ids.len()], before_ids.as_slice());
        assert_eq!(after_ids.len(), before_ids.len() + 2);
    }

    #[rstest]
    fn uncaptured_calls_flatten_into_notes_and_record_merged_in_sidecar() {
        let sid = "01a18221-a7b8-75e0-b6ef-b2a5cc95d2c9";
        let messages = vec![
            sample_msg("u1", Role::User, vec![Content::Text("check".to_owned())], 10),
            sample_msg(
                "a1",
                Role::Assistant,
                vec![Content::ToolUse(ToolUse {
                    id: ToolCallId::from("c1".to_owned()),
                    name: "run_terminal_command".to_owned(),
                    input: Value::Null,
                })],
                11,
            ),
            sample_msg(
                "t1",
                Role::Tool,
                vec![Content::ToolResult(ToolResult {
                    call: ToolCallId::from("c1".to_owned()),
                    output: Value::Null,
                    error: false,
                })],
                12,
            ),
            sample_msg("a2", Role::Assistant, vec![Content::Text("done".to_owned())], 13),
        ];
        let mut prev_gk = None;
        let rendered = render_messages(sid, &messages, &mut prev_gk, 0, None);
        assert_eq!(rendered.chat_lines.len(), 2);
        assert!(
            rendered.chat_lines[1]["content"].as_str().unwrap().contains("[ran a shell command]")
        );
        assert_eq!(rendered.sidecar.merged.get("a1").map(String::as_str), Some("a2"));
        assert_eq!(rendered.sidecar.merged.get("t1").map(String::as_str), Some("a2"));
    }
}
