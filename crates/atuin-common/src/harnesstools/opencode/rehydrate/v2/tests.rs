use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rstest::rstest;
use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{Connection, Executor};
use time::OffsetDateTime;

use super::*;
use crate::harnesstools::opencode::rehydrate::import;
use crate::harnesstools::opencode::rehydrate::tests::{SES, captured, every_kind};
use crate::harnesstools::opencode::session::projection::projected;
use crate::harnesstools::rehydrate::{ForkOf, RehydrateError, testing};
use crate::harnesstools::session::Usage;
use crate::path::AbsPathBuf;

/// The part of opencode 2.0.24's schema its import writes and capture reads.
const DDL: &[&str] = &[
    "CREATE TABLE event_sequence (aggregate_id TEXT PRIMARY KEY, seq INTEGER NOT NULL, owner_id \
     TEXT)",
    "CREATE TABLE event (id TEXT PRIMARY KEY, aggregate_id TEXT NOT NULL, seq INTEGER NOT NULL, \
     created INTEGER DEFAULT 0 NOT NULL, type TEXT NOT NULL, data TEXT NOT NULL)",
    "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, parent_id TEXT, \
     fork_session_id TEXT, slug TEXT NOT NULL, directory TEXT NOT NULL, title TEXT, version TEXT \
     NOT NULL, revert TEXT, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL)",
    "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT \
     NULL, seq INTEGER NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, \
     data TEXT NOT NULL)",
];

/// The real export opencode 2.0.24 wrote of a session it ran (`opencode session export`; the
/// directory redacted): three turns of shell, read, write and edit calls, one shell command
/// failing.
const EXPORT: &str = include_str!("../../../../../tests/fixtures/opencode/v2_export.json");

async fn database(path: &Path) -> SqliteConnection {
    let opts = SqliteConnectOptions::new().filename(path).create_if_missing(true);
    let mut conn = SqliteConnection::connect_with(&opts).await.unwrap();
    for ddl in DDL {
        conn.execute(*ddl).await.unwrap();
    }
    conn
}

/// What `opencode session import` does with an export (opencode 2.0.24,
/// `SessionTransfer.import`): the session in the directory it is imported into, and each message
/// but an assistant's that never ended under its id, numbered in the order given, its data the
/// message without `id` and `type`.
async fn session_import(conn: &mut SqliteConnection, export: &Value, directory: &str) {
    let info = &export["info"];
    let id = info["id"].as_str().unwrap();
    crate::db::query::<sqlx::Sqlite>(
        "INSERT INTO session_v2 (id, project_id, slug, directory, title, version, time_created, \
         time_updated) VALUES (?1, 'global', 'slug', ?2, ?3, '2.0.24', ?4, ?4)",
    )
    .bind(id)
    .bind(directory)
    .bind(info["title"].as_str())
    .bind(info["time"]["created"].as_i64())
    .execute(&mut *conn)
    .await
    .unwrap();
    let messages: Vec<&Value> = export["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["type"] != "assistant" || !m["time"]["completed"].is_null())
        .collect();
    for (seq, message) in (1i64..).zip(&messages) {
        let mut data = (*message).clone();
        let data = data.as_object_mut().unwrap();
        let id = data.remove("id").unwrap();
        let kind = data.remove("type").unwrap();
        crate::db::query::<sqlx::Sqlite>(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, \
             data) VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
        )
        .bind(id.as_str())
        .bind(info["id"].as_str())
        .bind(kind.as_str())
        .bind(seq)
        .bind(message["time"]["created"].as_i64())
        .bind(Value::Object(data.clone()).to_string())
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    crate::db::query::<sqlx::Sqlite>(
        "INSERT INTO event_sequence (aggregate_id, seq) VALUES (?1, ?2)",
    )
    .bind(id)
    .bind(i64::try_from(messages.len()).unwrap())
    .execute(&mut *conn)
    .await
    .unwrap();
}

/// `export` imported into a new database, and captured back.
async fn imported(export: &Value) -> Vec<RehydrateMessage> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.db");
    let mut conn = database(&path).await;
    session_import(&mut conn, export, "/here").await;
    conn.close().await.unwrap();
    captured(&path).await
}

fn session(id: &str, title: Option<&str>, messages: Vec<RehydrateMessage>) -> RehydrateSession {
    RehydrateSession {
        id: id.to_owned(),
        title: title.map(str::to_owned),
        cwd: PathBuf::from("/elsewhere/proj"),
        original_cwd: Some(PathBuf::from("/work/proj")),
        git_branch: None,
        model: None,
        started_at: messages.first().map_or(OffsetDateTime::UNIX_EPOCH, |m| m.timestamp),
        messages,
        fork_of: None,
    }
}

fn is_session(id: &str, row: &RehydrateMessage) -> bool {
    row.source_id.starts_with(&format!("{id}:"))
}

/// Each row's id, role and content, but for reasoning, which is not written back (capture keeps
/// only that there was some).
fn keys(rows: &[RehydrateMessage]) -> Vec<(String, Role, Vec<Content>)> {
    rows.iter()
        .map(|r| {
            let mut content = r.content.clone();
            content.retain(|c| !matches!(c, Content::ReasoningSummary { .. }));
            (r.source_id.clone(), r.role.clone(), content)
        })
        .collect()
}

/// A session opencode 2.0.24 wrote, captured, written back and captured again, keys the same rows
/// in the same order, saying the same: every message is its own row.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_real_session_recaptures_as_the_same_rows() {
    let real: Value = serde_json::from_str(EXPORT).unwrap();
    let id = real["info"]["id"].as_str().unwrap();
    let original = testing::synced_as(imported(&real).await, true);
    assert!(original.len() > 10);

    let written = export(&session(id, Some("octest"), original.clone()));
    let again = imported(&written).await;
    pretty_assertions::assert_eq!(keys(&again), keys(&original));
    testing::assert_nothing_new(&original, again.iter().map(|r| r.source_id.as_str()));
    let calls = |rows: &[RehydrateMessage]| -> Vec<_> {
        rows.iter()
            .filter(|r| !is_session(id, r))
            .map(|r| (r.turn_id.clone(), r.usage, r.stop_reason.clone(), r.model.clone()))
            .collect()
    };
    assert_eq!(calls(&again), calls(&original));

    // the messages are those opencode exported, but for what capture keeps nothing of: the idle
    // rows of turns that succeeded, and reasoning
    let kinds = |export: &Value| -> Vec<(String, String)> {
        export["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["type"] != "idle")
            .map(|m| (m["id"].as_str().unwrap().to_owned(), m["type"].as_str().unwrap().to_owned()))
            .collect()
    };
    assert_eq!(kinds(&written), kinds(&real));
    let edit = |export: &Value| -> Value {
        export["messages"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|m| m["content"].as_array().into_iter().flatten())
            .find(|item| item["name"] == "edit")
            .map(|item| item["state"]["metadata"]["files"][0]["patch"].clone())
            .unwrap()
    };
    // the diff, without the `Index:` header opencode's has
    let (written, real) = (edit(&written), edit(&real));
    assert!(real.as_str().unwrap().ends_with(written.as_str().unwrap()), "{written} vs {real}");
}

/// Rows opencode 1.x wrote (parts) are laid out as opencode 2's messages: the prompt, the text
/// opencode injected into it, the model call with its text, calls and usage, and the next prompt
/// with the call that failed.
#[rstest]
fn every_kind_of_1_x_row_goes_into_a_message() {
    let written = export(&session(SES, Some("Fix the build"), every_kind()));
    let messages = written["messages"].as_array().unwrap();
    let kinds: Vec<&str> = messages.iter().map(|m| m["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["user", "synthetic", "assistant", "user", "assistant"]);
    assert_eq!(messages[0]["text"], "fix the build");
    assert_eq!(messages[1]["text"], "Called the Read tool");

    let call = &messages[2];
    assert_eq!(call["model"], json!({"id": "gpt-x", "providerID": "openai"}));
    assert_eq!(call["time"]["created"], 1_790_217_606_979i64);
    assert_eq!(call["finish"], "tool-calls");
    assert_eq!(
        call["tokens"],
        json!({"input": 802, "output": 42, "reasoning": 10, "cache": {"read": 200, "write": 0}})
    );
    let content: Vec<(&str, &str)> = call["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            let what = item["text"].as_str().or_else(|| item["state"]["status"].as_str());
            (item["type"].as_str().unwrap(), what.unwrap())
        })
        .collect();
    // the retry's error is not one of the call's
    assert_eq!(content, [("text", "On it."), ("tool", "completed"), ("tool", "error")]);
    assert_eq!(call["content"][1]["state"]["content"], json!([{"type": "text", "text": "ok"}]));
    assert_eq!(
        call["content"][2]["state"]["error"],
        json!({"type": "tool.execution", "message": "no such file"})
    );

    // a failed call keeps its message's id
    let failed = &messages[4];
    assert_eq!(failed["id"], "msg_0d149b2d5001gkfmc9bUMDXLJ5");
    assert_eq!(failed["error"], json!({"type": "unknown", "message": "mock bad request"}));
    assert_eq!(written["info"]["title"], "Fix the build");
    // idle, or opencode waits for the turn it would be in to end
    assert_eq!(written["info"]["time"]["idle"], written["info"]["time"]["updated"]);
    assert_eq!(written["info"]["location"]["directory"], "/elsewhere/proj");
}

/// A session opencode 1.x wrote, restored on opencode 2, reads back under the ids it was synced
/// with: its rows are grouped into fewer messages than it had parts, each keyed on its first
/// row's id, so re-capturing it pushes nothing new.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_1_x_session_restored_on_2_recaptures_as_nothing_new() {
    holds_every_row(every_kind()).await;
}

/// `rows` written back for opencode 2 as session [`SES`], and what sync makes of the copy: every
/// row of it re-captured is one synced already, and the copy is known to hold every row, those
/// merged into another's message too; the rows each merged row went into, by its id.
async fn holds_every_row(rows: Vec<RehydrateMessage>) -> HashMap<String, String> {
    let written = export(&session(SES, Some("Fix the build"), rows.clone()));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.db");
    let mut conn = database(&path).await;
    session_import(&mut conn, &written, "/here").await;
    conn.close().await.unwrap();
    let again = captured(&path).await;
    let again: Vec<&str> = again
        .iter()
        .map(|r| r.source_id.as_str())
        .filter(|id| !id.starts_with(&format!("{SES}:")))
        .collect();
    assert_eq!(again.len(), written["messages"].as_array().unwrap().len());
    testing::assert_nothing_new(&rows, again.iter().copied());

    let held = projected(&path, SES).await.unwrap().unwrap();
    let mut known: Vec<&str> =
        held.rows.iter().chain(held.merged.keys()).map(String::as_str).collect();
    known.sort_unstable();
    let mut synced: Vec<&str> = rows
        .iter()
        .map(|r| r.source_id.as_str())
        .filter(|id| !id.starts_with(&format!("{SES}:")))
        .collect();
    synced.sort_unstable();
    assert_eq!(known, synced);
    assert_eq!(held.rows, again);
    held.merged
}

fn said_by_assistant(id: &str, content: Vec<Content>) -> RehydrateMessage {
    RehydrateMessage {
        role: Role::Assistant,
        content,
        ..prompt(id, None)
    }
}

/// A row no message is written for is held all the same: a turn that left only reasoning (by
/// the message before it, else the one after), a retry, and a result kept apart from its call
/// (by its call's message).
#[rstest]
#[case::a_reasoning_only_turn(&["prt_u1", "prt_r", "prt_u2"])]
#[case::a_retry_only_turn(&["prt_u1", "prt_e", "prt_u2"])]
#[case::a_reasoning_only_turn_first(&["prt_r", "prt_u1"])]
#[case::a_result_apart(&["prt_u1", "prt_c", "prt_res", "prt_u2"])]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_no_message_is_written_for_is_held(#[case] ids: &[&str]) {
    let call = || ToolUse {
        id: "call_1".to_owned().into(),
        name: "shell".to_owned(),
        input: json!({"command": "ls"}),
    };
    let rows = ids
        .iter()
        .map(|id| match *id {
            "prt_r" => said_by_assistant(id, vec![Content::ReasoningSummary { tokens: None }]),
            "prt_e" => said_by_assistant(id, vec![Content::Error("overloaded".to_owned())]),
            "prt_c" => said_by_assistant(id, vec![Content::ToolUse(call())]),
            "prt_res" => RehydrateMessage {
                role: Role::Tool,
                ..said_by_assistant(id, vec![Content::ToolResult(ToolResult {
                    call: "call_1".to_owned().into(),
                    output: json!("files"),
                    error: false,
                })])
            },
            _ => prompt(id, None),
        })
        .collect();
    let merged = holds_every_row(rows).await;
    if ids.contains(&"prt_res") {
        assert_eq!(merged.get("prt_res").map(String::as_str), Some("prt_c"));
    }
}

/// A message atuin wrote names the session it wrote it for: opencode's own fork, which copies it
/// whole into another session, has a row of its own, read under its own id.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_copy_of_a_restored_message_in_another_session_is_its_own() {
    let mut written = export(&session(SES, None, every_kind()));
    written["info"]["id"] = json!("ses_0d148d4b0001WQdGo06CVdqAff");
    for (n, message) in written["messages"].as_array_mut().unwrap().iter_mut().enumerate() {
        message["id"] = json!(format!("msg_copy{n}"));
    }
    let again = imported(&written).await;
    let ids: Vec<&str> =
        again.iter().map(|r| r.source_id.as_str()).filter(|id| !id.starts_with("ses_")).collect();
    assert!(!ids.is_empty());
    assert!(ids.iter().all(|id| id.starts_with("msg_copy")), "{ids:?}");
}

/// A fork's messages get ids of their own, and it starts with its marker, which links it to the
/// session it was forked from once captured. Its calls, a compaction's too, are the original's,
/// so they are not counted again.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fork_is_linked_to_its_original() {
    let mut real: Value = serde_json::from_str(EXPORT).unwrap();
    // a completed compaction as opencode 2 writes one (`v2.json`)
    real["messages"].as_array_mut().unwrap().push(json!({
        "id": "msg_11e4bc8960010Ypn4F9uXZ2kQe",
        "type": "compaction",
        "time": {"created": 1_791_509_645_470i64},
        "status": "completed",
        "reason": "auto",
        "model": {"id": "mock-model", "providerID": "mock"},
        "summary": "## Objective\n- mock objective\n",
        "recent": "",
        "cost": 0.000_923,
        "tokens": {"input": 801, "output": 41, "reasoning": 10, "cache": {"read": 200, "write": 0}},
    }));
    let original = testing::synced_as(imported(&real).await, true);
    let mut fork = session("ses_ee1b7b1dfffe46nQL0NuU68Gzz", None, original.clone());
    fork.fork_of = Some(ForkOf {
        id: real["info"]["id"].as_str().unwrap().to_owned(),
        atuin_id: None,
        path: None,
    });
    let written = export(&fork);
    let ids = |export: &Value| -> Vec<String> {
        export["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let theirs = ids(&real);
    assert!(ids(&written).iter().all(|id| id.starts_with("msg_") && !theirs.contains(id)));
    assert_eq!(ids(&written), ids(&export(&fork)), "a retried import mints the same ids");

    let again = imported(&written).await;
    let first = again.iter().find(|row| !is_session(&fork.id, row)).unwrap();
    assert_eq!(first.role, Role::System);
    let [Content::Text(marker)] = first.content.as_slice() else {
        panic!("no marker: {first:?}");
    };
    assert!(continuation::is_fork_marker(marker), "{marker}");
    assert!(marker.contains(theirs_session(&real)), "{marker}");

    let calls = |rows: &[RehydrateMessage]| -> Vec<_> {
        rows.iter().filter_map(|r| r.turn_id.clone().zip(r.usage)).collect()
    };
    assert!(calls(&original).iter().any(|(turn, _)| turn.contains(":mock/mock-model#")));
    assert_eq!(calls(&again), calls(&original));
}

fn theirs_session(export: &Value) -> &str {
    export["info"]["id"].as_str().unwrap()
}

fn prompt(id: &str, parent: Option<&str>) -> RehydrateMessage {
    RehydrateMessage {
        source_id: id.to_owned(),
        parent_source_id: parent.map(str::to_owned),
        timestamp: OffsetDateTime::from_unix_timestamp(1_790_000_000).unwrap(),
        role: Role::User,
        content: vec![Content::Text(format!("said {id}"))],
        model: None,
        provider: None,
        usage: None,
        stop_reason: None,
        turn_id: None,
        cwd: None,
        git_branch: None,
    }
}

/// Rows of one prompt (following on from the same row) are one message; prompts that follow on
/// from each other (another harness's consecutive lines) stay apart.
#[rstest]
#[case::one_prompt(&[("a", None), ("b", None)], &["said a\n\nsaid b"])]
#[case::two_prompts(&[("a", None), ("b", Some("a"))], &["said a", "said b"])]
fn a_prompt_is_the_rows_that_follow_on_from_one_row(
    #[case] rows: &[(&str, Option<&str>)],
    #[case] texts: &[&str],
) {
    let rows = rows.iter().map(|(id, parent)| prompt(id, *parent)).collect();
    let written = export(&session(SES, None, rows));
    let got: Vec<&str> = written["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["text"].as_str().unwrap())
        .collect();
    assert_eq!(got, texts);
}

/// A call's `finish` is one opencode 2's import takes (`Session.Message.Assistant.finish`), whatever
/// the call ended for; one that was stopped also says so in its error, as opencode writes it.
#[rstest]
#[case::end_turn(Some(StopReason::EndTurn), "stop")]
#[case::stop_sequence(Some(StopReason::StopSequence), "stop")]
#[case::max_tokens(Some(StopReason::MaxTokens), "length")]
#[case::tool_use(Some(StopReason::ToolUse), "tool-calls")]
#[case::refusal(Some(StopReason::Refusal), "content-filter")]
#[case::error(Some(StopReason::Error), "error")]
#[case::aborted(Some(StopReason::Aborted), "error")]
#[case::another_harness_s(Some(StopReason::Other("pause_turn".into())), "unknown")]
#[case::opencode_s_own(Some(StopReason::Other("length".into())), "length")]
#[case::none(None, "stop")]
fn a_call_finishes_as_opencode_2_takes_it(#[case] stop: Option<StopReason>, #[case] want: &str) {
    const ALLOWED: [&str; 6] =
        ["stop", "length", "tool-calls", "content-filter", "error", "unknown"];
    let mut row = said_by_assistant("prt_a", vec![Content::Text("hi".to_owned())]);
    row.usage = Some(Usage::default());
    row.stop_reason = stop.clone();
    let written = export(&session(SES, None, vec![row]));
    let call = &written["messages"][0];
    assert_eq!(call["finish"], want);
    assert!(ALLOWED.contains(&want));
    let aborted = stop == Some(StopReason::Aborted);
    assert_eq!(call["error"]["type"] == "aborted", aborted, "{call}");
}

/// The session ended as its last turn did: failed or interrupted when its idle row says so.
#[rstest]
#[case::succeeded(None, "succeeded")]
#[case::failed(Some(StopReason::Error), "failed")]
#[case::interrupted(Some(StopReason::Aborted), "interrupted")]
fn the_session_ended_as_its_last_turn_did(#[case] stop: Option<StopReason>, #[case] want: &str) {
    let mut rows = vec![prompt("msg_u", None)];
    if let Some(stop) = stop {
        let mut idle = prompt("msg_i", None);
        idle.role = Role::System;
        idle.content.clear();
        idle.stop_reason = Some(stop);
        rows.push(idle);
    }
    let written = export(&session(SES, None, rows));
    assert_eq!(written["info"]["outcome"], want);
}

/// A call whose turn names no model is written on the provider capture kept, else on the one its
/// model names, else on opencode's own.
#[rstest]
#[case::captured("big-pickle", Some("anthropic"), ("anthropic", "big-pickle"))]
#[case::captured_routed(
    "anthropic/claude-sonnet-4",
    Some("openrouter"),
    ("openrouter", "anthropic/claude-sonnet-4")
)]
#[case::named("mock/big-pickle", None, ("mock", "big-pickle"))]
#[case::neither("big-pickle", None, ("opencode", "big-pickle"))]
fn a_call_is_written_on_its_provider(
    #[case] model: &str,
    #[case] provider: Option<&str>,
    #[case] want: (&str, &str),
) {
    let row = RehydrateMessage {
        source_id: "prt_1".to_owned(),
        parent_source_id: None,
        timestamp: OffsetDateTime::from_unix_timestamp(1_790_000_000).unwrap(),
        role: Role::Assistant,
        content: vec![Content::Text("hi".to_owned())],
        model: Some(model.to_owned()),
        provider: provider.map(str::to_owned),
        usage: Some(Usage::default()),
        stop_reason: None,
        turn_id: None,
        cwd: None,
        git_branch: None,
    };
    let written = export(&session(SES, None, vec![row]));
    let written = &written["messages"][0]["model"];
    assert_eq!(
        (written["providerID"].as_str(), written["id"].as_str()),
        (Some(want.0), Some(want.1))
    );
}

/// A model call capture kept no result of is written failed, as opencode 2 ends a call it was
/// stopped in: import keeps only ended calls.
#[rstest]
fn a_call_without_a_result_is_written_aborted() {
    let mut row = RehydrateMessage {
        source_id: "prt_1".to_owned(),
        parent_source_id: None,
        timestamp: OffsetDateTime::from_unix_timestamp(1_790_000_000).unwrap(),
        role: Role::Assistant,
        content: vec![Content::ToolUse(ToolUse {
            id: "call_1".to_owned().into(),
            name: "shell".to_owned(),
            input: json!({"command": "sleep 100"}),
        })],
        model: Some("opencode/big-pickle".to_owned()),
        provider: None,
        usage: None,
        stop_reason: None,
        turn_id: None,
        cwd: None,
        git_branch: None,
    };
    row.usage = Some(Usage::default());
    let written = export(&session(SES, None, vec![row]));
    let call = &written["messages"][0];
    assert_eq!(call["model"], json!({"id": "big-pickle", "providerID": "opencode"}));
    assert!(call["time"]["completed"].is_i64());
    assert_eq!(call["content"][0]["state"]["status"], "error");
    assert_eq!(call["content"][0]["state"]["error"]["type"], "aborted");
}

/// A tool's output as opencode 2 keeps it: items of its own (`text`, `file`) as they are, another
/// harness's text blocks (whatever their type) as text, and anything else (a Claude image, a `tool_reference`) as a note
/// of it, which opencode's import takes. Only a shell call is written completed with an exit code
/// for a failure.
#[rstest]
#[case::opencode_s_own(
    "shell",
    json!([{"type": "text", "text": "out"}, {"type": "file", "uri": "file:///a.png", "mime": "image/png"}]),
    false,
    json!({"status": "completed", "content": [
        {"type": "text", "text": "out"},
        {"type": "file", "uri": "file:///a.png", "mime": "image/png"},
    ]}),
)]
#[case::a_claude_image(
    "Read",
    json!([
        {"type": "text", "text": "see", "cache_control": {"type": "ephemeral"}},
        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}},
        {"type": "tool_reference", "tool_name": "x"},
    ]),
    false,
    json!({"status": "completed", "content": [
        {"type": "text", "text": "see"},
        {"type": "text", "text": "[image]"},
        {"type": "text", "text": "[tool_reference]"},
    ]}),
)]
#[case::codex_s_text(
    "exec",
    json!([{"type": "input_text", "text": "ran"}]),
    false,
    json!({"status": "completed", "content": [{"type": "text", "text": "ran"}]}),
)]
#[case::a_failed_command(
    "shell",
    json!([{"type": "text", "text": "no\n"}, {"type": "text", "text": "Exited with code 2"}]),
    true,
    json!({"status": "completed", "content": [
        {"type": "text", "text": "no\n"},
        {"type": "text", "text": "Exited with code 2"},
    ], "metadata": {"exit": 2}}),
)]
#[case::another_tool_s_failure(
    "Read",
    json!([{"type": "text", "text": "no such file"}]),
    true,
    json!({"status": "error", "error": {"type": "tool.execution", "message": "no such file"}}),
)]
fn a_tool_s_output_is_written_as_opencode_2_keeps_it(
    #[case] name: &str,
    #[case] output: Value,
    #[case] error: bool,
    #[case] want: Value,
) {
    let call = ToolUse {
        id: "call_1".to_owned().into(),
        name: name.to_owned(),
        input: json!({}),
    };
    let result = ToolResult {
        call: "call_1".to_owned().into(),
        output,
        error,
    };
    let mut state = tool(&call, Some(&result), None, 0)["state"].clone();
    state.as_object_mut().unwrap().remove("input");
    assert_eq!(state, want);
}

/// A directory programs can be run from, next to the test binary: CI's temporary directory is
/// mounted without exec, so a fake program written there could not be run.
#[cfg(unix)]
fn exec_dir() -> tempfile::TempDir {
    let exe = std::env::current_exe().unwrap();
    tempfile::tempdir_in(exe.parent().unwrap()).unwrap()
}

/// A fake opencode 2: says its version, and records how an import was run, then fails it.
#[cfg(unix)]
fn fake_opencode(dir: &Path) -> (tempfile::TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let bin = exec_dir();
    let path = bin.path().join("opencode");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n[ \"$1\" = --version ] && echo 'opencode v2.0.24' && exit 0\necho \"$@\" \
             > {log}/args\npwd > {log}/cwd\necho \"$OPENCODE_DB\" > {log}/env\nfor file; do :; \
             done\ncp \"$file\" {log}/export.json\necho boom >&2\nexit 3\n",
            log = dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    (bin, path)
}

/// opencode 2 imports with `opencode session import`, through a server of its own on the
/// database it was given, into the session's directory, the export in opencode 2's format.
#[cfg(unix)]
#[rstest]
#[tokio::test]
async fn opencode_2_imports_its_own_export() {
    let dir = tempfile::tempdir().unwrap();
    let (_bin, program) = fake_opencode(dir.path());
    let db = dir.path().join("opencode.db");
    let mut s = session(SES, Some("t"), every_kind());
    s.cwd = AbsPathBuf::canonicalize(dir.path()).unwrap().into_path_buf();

    let err = import(&program, &db, &s).await.unwrap_err();
    assert!(matches!(&err, RehydrateError::Other(why) if why.contains("boom")), "{err}");
    let read = |name: &str| std::fs::read_to_string(dir.path().join(name)).unwrap();
    let args = read("args");
    let expected = format!("session import --standalone --directory {} ", s.cwd.display());
    assert!(args.starts_with(&expected), "{args}");
    assert_eq!(read("cwd").trim(), s.cwd.to_str().unwrap());
    assert_eq!(read("env").trim(), db.display().to_string());
    let sent: Value = serde_json::from_str(&read("export.json")).unwrap();
    assert_eq!(sent, export(&s));
    let file = args.trim().rsplit(' ').next().unwrap();
    assert!(!Path::new(file).exists(), "the export file is removed");
}

/// A session opencode 2 holds already is not imported again.
#[rstest]
#[tokio::test]
async fn a_session_opencode_2_holds_is_not_imported_again() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("opencode.db");
    let mut conn = database(&db).await;
    session_import(&mut conn, &export(&session(SES, None, every_kind())), "/here").await;
    conn.close().await.unwrap();
    let missing = dir.path().join("no-such-opencode");
    let err = import(&missing, &db, &session(SES, None, vec![])).await.unwrap_err();
    assert!(matches!(err, RehydrateError::AlreadyExists(path) if path == db));
}
