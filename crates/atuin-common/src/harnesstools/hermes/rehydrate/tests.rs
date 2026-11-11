use std::path::PathBuf;
use std::sync::Arc;

use futures::TryStreamExt;
use parking_lot::Mutex;
use pretty_assertions::assert_eq;
use rstest::rstest;
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection, SqliteJournalMode};

use super::super::cli::fake::{FakeHermes, Reply};
use super::super::session::HermesSession;
use super::super::session::tests::{TOOL_ERROR, probe_store};
use super::*;
use crate::harnesstools::continuation::tests::{HERMES, recorded};
use crate::harnesstools::rehydrate::{ForkOf, testing};
use crate::harnesstools::session::{Message, Session, SessionId};

/// The rows of `session` as imported, and its session row's fields, read back from `db`.
async fn connect(db: &Path) -> SqliteConnection {
    let opts = SqliteConnectOptions::new()
        .filename(db)
        .create_if_missing(true)
        .foreign_keys(false)
        .journal_mode(SqliteJournalMode::Wal);
    SqliteConnection::connect_with(&opts).await.unwrap()
}

fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

/// A message's `content` as Hermes stores it: a list of parts as its multimodal content, and a
/// string as given. A string that starts as that content does is the exception: Hermes wraps it,
/// so that it can't be taken for parts. The rig probe showed it.
fn content_of(value: &Value) -> Option<String> {
    const PREFIX: &str = "\u{0}json:";
    match value {
        Value::Array(_) => Some(format!("{PREFIX}{value}")),
        Value::String(text) if text.starts_with(PREFIX) => Some(format!("{PREFIX}{value}")),
        other => text_of(other),
    }
}

/// Write `row` into the store `db` as the gateway's import does: the session's columns as given,
/// each message under a fresh `messages.id`, its `message_uid` kept, JSON columns as text, and a
/// list of parts as multimodal content.
pub async fn import_into(db: &Path, row: &Value) {
    let mut conn = connect(db).await;
    let mut tx = conn.begin().await.unwrap();
    crate::db::query::<sqlx::Sqlite>(
        "INSERT INTO sessions (id, source, title, cwd, model, model_config, parent_session_id, \
         started_at, ended_at, end_reason, git_branch) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
         ?9, ?10, ?11)",
    )
    .bind(text_of(&row["id"]))
    .bind(text_of(&row["source"]))
    .bind(text_of(&row["title"]))
    .bind(text_of(&row["cwd"]))
    .bind(text_of(&row["model"]))
    .bind(text_of(&row["model_config"]))
    .bind(text_of(&row["parent_session_id"]))
    .bind(row["started_at"].as_f64())
    .bind(row["ended_at"].as_f64())
    .bind(text_of(&row["end_reason"]))
    .bind(text_of(&row["git_branch"]))
    .execute(&mut *tx)
    .await
    .unwrap();
    for m in row["messages"].as_array().unwrap() {
        crate::db::query::<sqlx::Sqlite>(
            "INSERT INTO messages (session_id, role, content, tool_call_id, tool_calls, \
             tool_name, timestamp, finish_reason, reasoning, display_kind, display_metadata, \
             message_uid) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )
        .bind(text_of(&row["id"]))
        .bind(text_of(&m["role"]))
        .bind(content_of(&m["content"]))
        .bind(text_of(&m["tool_call_id"]))
        .bind(text_of(&m["tool_calls"]))
        .bind(text_of(&m["tool_name"]))
        .bind(m["timestamp"].as_f64())
        .bind(text_of(&m["finish_reason"]))
        .bind(text_of(&m["reasoning"]))
        .bind(text_of(&m["display_kind"]))
        .bind(text_of(&m["display_metadata"]))
        .bind(text_of(&m["message_uid"]))
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    conn.close().await.unwrap();
}

/// A Hermes whose `atuin import` keeps each row it is handed in `rows`, then answers `reply`.
fn importing(rows: &Arc<Mutex<Vec<Value>>>, reply: CliOutput) -> Reply {
    let rows = Arc::clone(rows);
    Box::new(move |args: &[&str]| {
        assert_eq!(&args[..2], ["atuin", "import"]);
        let row = serde_json::from_str(&std::fs::read_to_string(args[2]).unwrap()).unwrap();
        rows.lock().push(row);
        Ok(reply.clone())
    })
}

fn said(stdout: &str, code: i32) -> CliOutput {
    CliOutput {
        code: Some(code),
        stdout: format!("{stdout}\n"),
        stderr: String::new(),
    }
}

fn imported() -> CliOutput {
    said(r#"{"ok": true, "receipt": {"imported": 1, "skipped": 0}}"#, 0)
}

/// The probe's session with a failing `ls`, as capture holds it, to restore under `cwd`.
async fn captured(cwd: &Path) -> RehydrateSession {
    let mut session = recorded(HERMES).await;
    session.cwd = cwd.to_owned();
    // As capture holds it: the shared fixture leaves the provider the reader gives replies out.
    for m in session.messages.iter_mut().filter(|m| m.role == Role::Assistant) {
        m.provider = Some("openrouter".to_owned());
    }
    session
}

/// `session` written into a Hermes home of its own by [`rehydrate_with`], imported by the fake
/// gateway, and the row it was handed.
async fn restored(home: &Path, session: &RehydrateSession) -> (PathBuf, Value) {
    let (written, row) = restored_as(home, session).await;
    (written.path, row)
}

/// [`restored`], with what the writer said it left out.
async fn restored_as(home: &Path, session: &RehydrateSession) -> (Rehydrated, Value) {
    if !home.join(DB_FILE).is_file() {
        empty_store(home).await;
    }
    let rows = Arc::new(Mutex::new(Vec::new()));
    let hermes = FakeHermes::replying([importing(&rows, imported())]);
    let written = rehydrate_with(home, session, &hermes).await.unwrap();
    let row = rows.lock().pop().unwrap();
    import_into(&written.path, &row).await;
    (written, row)
}

/// `session` written into the Hermes home `home` as [`restored`] writes it, and read back by the
/// reader capture uses.
pub async fn written(
    home: &Path,
    session: &RehydrateSession,
) -> Vec<super::super::session::HermesMessage> {
    let (db, _) = restored(home, session).await;
    read_back(&db, &session.id).await
}

async fn read_back(db: &Path, id: &str) -> Vec<super::super::session::HermesMessage> {
    HermesSession::open(SessionId::from(id.to_owned()), db).read().try_collect().await.unwrap()
}

/// An empty store at `home/state.db`, as Hermes makes one.
async fn empty_store(home: &Path) -> PathBuf {
    let dir = tempfile::tempdir().unwrap();
    let probe = probe_store(dir.path()).await;
    std::fs::create_dir_all(home).unwrap();
    let db = home.join(DB_FILE);
    std::fs::copy(&probe, &db).unwrap();
    let mut conn = connect(&db).await;
    crate::db::query::<sqlx::Sqlite>("DELETE FROM messages").execute(&mut conn).await.unwrap();
    crate::db::query::<sqlx::Sqlite>("DELETE FROM sessions").execute(&mut conn).await.unwrap();
    conn.close().await.unwrap();
    db
}

fn msg(id: &str, role: Role, content: Vec<Content>) -> RehydrateMessage {
    RehydrateMessage {
        source_id: id.to_owned(),
        parent_source_id: None,
        timestamp: OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1_794_000_000),
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

fn call(id: &str, name: &str, input: Value) -> Content {
    Content::ToolUse(ToolUse {
        id: ToolCallId::from(id.to_owned()),
        name: name.to_owned(),
        input,
    })
}

fn result(call: &str, output: Value) -> Content {
    Content::ToolResult(ToolResult {
        call: ToolCallId::from(call.to_owned()),
        output,
        error: false,
    })
}

fn session(cwd: &Path, messages: Vec<RehydrateMessage>) -> RehydrateSession {
    RehydrateSession {
        id: "20261110_120000_a1b2c3".to_owned(),
        title: Some("fix the flaky test".to_owned()),
        cwd: cwd.to_owned(),
        original_cwd: None,
        git_branch: None,
        model: None,
        started_at: OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1_793_999_999),
        messages,
        fork_of: None,
    }
}

fn uids(row: &Value) -> Vec<&str> {
    row["messages"].as_array().unwrap().iter().map(|m| m["message_uid"].as_str().unwrap()).collect()
}

/// A Hermes session captured on another machine is restored under its own id, as a session that a
/// Hermes here resumes: `source` `cli`, no route or owner, the cwd here, and ended. It keeps its
/// model and provider. The reader reads every row back under the key it was synced under, so
/// capture pushes nothing new.
#[rstest]
#[tokio::test]
async fn a_restore_reads_back_under_the_synced_keys() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // The profile runs on the provider the session did, so its route is kept.
    std::fs::write(home.path().join("config.yaml"), "model:\n  provider: openrouter\n").unwrap();
    let session = captured(cwd.path()).await;

    let (written, row) = restored_as(home.path(), &session).await;
    let db = written.path;

    assert_eq!(db, home.path().join(DB_FILE));
    assert_eq!(written.note, None, "nothing left out");
    assert_eq!(row["id"], TOOL_ERROR);
    assert_eq!(row["source"], "cli");
    assert_eq!(row["cwd"], json!(cwd.path()));
    assert_eq!(row["end_reason"], "cli_close");
    assert!(row["ended_at"].as_f64() >= row["started_at"].as_f64());
    for key in ["user_id", "session_key", "chat_id", "origin_json", "system_prompt"] {
        assert!(row.get(key).is_none(), "{key} written");
    }
    assert_eq!(row["model"], json!(session.model));
    assert_eq!(row["model_config"]["gateway_runtime"]["provider"], "openrouter");

    let again = read_back(&db, TOOL_ERROR).await;
    let read: Vec<String> =
        again.iter().filter(|m| m.title().is_none()).map(|m| m.id().unwrap().to_string()).collect();
    testing::assert_nothing_new(&session.messages, read.iter().map(String::as_str));
    let synced: Vec<&str> = session
        .messages
        .iter()
        .filter(|m| !m.content.is_empty())
        .map(|m| m.source_id.as_str())
        .collect();
    assert_eq!(read, synced);
    // The shell call and its failed result, as Hermes wrote them.
    let calls: Vec<_> = again
        .iter()
        .flat_map(Message::content)
        .filter_map(|c| match c {
            Content::ToolResult(r) => Some(r.error),
            _ => None,
        })
        .collect();
    assert!(calls.contains(&true), "the failed result isn't one: {calls:?}");
}

/// A restore names the model and provider it ran on only when the profile runs on the same
/// provider, whatever its case. With no config, no `model.provider`, or another provider, it names
/// neither, and the profile's default runs it.
#[rstest]
#[case::same(Some("model:\n  default: xiaomi/mimo-v2.5-pro\n  provider: openrouter\n"), true)]
#[case::same_in_capitals(Some("model:\n  provider: OpenRouter\n"), true)]
#[case::another(Some("model:\n  default: claude-opus-5-5\n  provider: anthropic\n"), false)]
#[case::no_config(None, false)]
#[case::no_provider(Some("model:\n  default: xiaomi/mimo-v2.5-pro\n"), false)]
#[case::model_as_a_name(Some("model: xiaomi/mimo-v2.5-pro\n"), false)]
#[case::not_yaml(Some("model: [\n"), false)]
#[tokio::test]
async fn a_restore_names_its_route_only_for_the_profiles_provider(
    #[case] config: Option<&str>,
    #[case] kept: bool,
) {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    if let Some(config) = config {
        std::fs::write(home.path().join("config.yaml"), config).unwrap();
    }
    let session = captured(cwd.path()).await;
    assert!(session.model.is_some());

    let (_, row) = restored(home.path(), &session).await;

    if kept {
        assert_eq!(row["model"], json!(session.model));
        assert_eq!(row["model_config"]["gateway_runtime"]["provider"], "openrouter");
    } else {
        assert!(row.get("model").is_none(), "{}", row["model"]);
        assert!(row.get("model_config").is_none(), "{}", row["model_config"]);
    }
}

/// A continuation names no route, whatever the profile runs on: its rows name no model.
#[rstest]
#[tokio::test]
async fn a_continuation_names_no_route() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("config.yaml"), "model:\n  provider: openrouter\n").unwrap();
    let original = recorded(crate::harnesstools::continuation::tests::CLAUDE).await;
    let mut continued = crate::harnesstools::continuation::continue_in(
        crate::harnesstools::continuation::tests::CLAUDE,
        &original,
        None,
        HERMES,
    )
    .unwrap()
    .session;
    continued.cwd = cwd.path().to_owned();

    let (_, row) = restored(home.path(), &continued).await;

    assert!(row.get("model").is_none(), "{}", row["model"]);
    assert!(row.get("model_config").is_none(), "{}", row["model_config"]);
}

/// A fork's link survives a route left out.
#[rstest]
fn a_fork_without_a_route_keeps_its_link() {
    let cwd = tempfile::tempdir().unwrap();
    let mut fork =
        session(cwd.path(), vec![msg("u1", Role::User, vec![Content::Text("go".into())])]);
    fork.fork_of = Some(ForkOf {
        id: "orig".to_owned(),
        atuin_id: None,
        path: None,
    });
    let row = session_row(&fork, None, None, &[]);
    assert_eq!(row["model_config"], json!({"_branched_from": "orig"}));
    assert!(row.get("model").is_none());
}

/// A prompt, a reply with reasoning and a call, its result, and a reply: each a Hermes row of
/// its own shape.
#[rstest]
fn each_role_is_its_own_row() {
    let cwd = tempfile::tempdir().unwrap();
    let mut reply = msg("a1", Role::Assistant, vec![
        Content::Reasoning("look first".into()),
        Content::Text("Looking.".into()),
        call("call_1", "terminal", json!({"command": "ls"})),
    ]);
    reply.stop_reason = Some(StopReason::ToolUse);
    let mut done = msg("a2", Role::Assistant, vec![Content::Text("Done.".into())]);
    done.stop_reason = Some(StopReason::EndTurn);
    let rows = messages(&session(cwd.path(), vec![
        msg("u1", Role::User, vec![Content::Text("list it".into())]),
        reply,
        msg("t1", Role::Tool, vec![result("call_1", json!("a\nb"))]),
        done,
    ]))
    .unwrap();

    assert_eq!(rows[0]["role"], "user");
    assert_eq!(rows[0]["content"], "list it");
    assert_eq!(rows[1]["content"], "Looking.");
    assert_eq!(rows[1]["reasoning"], "look first");
    assert_eq!(rows[1]["finish_reason"], "tool_calls");
    assert_eq!(
        rows[1]["tool_calls"],
        json!([{"id": "call_1", "type": "function", "function": {"name": "terminal", "arguments": r#"{"command":"ls"}"#}}])
    );
    assert_eq!(rows[2]["role"], "tool");
    assert_eq!(rows[2]["tool_call_id"], "call_1");
    assert_eq!(rows[2]["tool_name"], "terminal");
    assert_eq!(rows[2]["content"], "a\nb");
    assert_eq!(rows[3]["finish_reason"], "stop");
    let uids: Vec<&str> = rows.iter().map(|r| r["message_uid"].as_str().unwrap()).collect();
    assert_eq!(uids, ["u1", "a1", "t1", "a2"]);
    assert!(rows.iter().all(|r| r["timestamp"].as_f64() == Some(1_794_000_000.0)));
}

/// A prompt of several blocks is Hermes's multimodal parts, which the reader reads back as the
/// blocks they were.
#[rstest]
fn a_prompt_of_several_blocks_is_parts() {
    let content = user_content(&[
        Content::Text("first".into()),
        Content::Text("second".into()),
        Content::Other(json!({"type": "image_url", "image_url": {"url": "data:"}})),
    ])
    .unwrap();
    let Value::Array(parts) = content else {
        panic!("not parts: {content}");
    };
    assert_eq!(parts[0], json!({"type": "text", "text": "first"}));
    assert_eq!(parts[1], json!({"type": "text", "text": "second"}));
    assert_eq!(parts[2]["type"], "image_url");
    assert_eq!(user_content(&[Content::Text(" ".into())]), None);
}

/// Every call gets a result: one captured without its output says so, and one with no result
/// at all gets one atuin makes, named after the call's row, before the next prompt.
#[rstest]
fn every_call_is_answered() {
    let cwd = tempfile::tempdir().unwrap();
    let rows = messages(&session(cwd.path(), vec![
        msg("u1", Role::User, vec![Content::Text("go".into())]),
        msg("a1", Role::Assistant, vec![
            call("c1", "terminal", json!({"command": "ls"})),
            call("c2", "read_file", json!({"path": "a"})),
            call("c3", "read_file", json!({"path": "b"})),
        ]),
        msg("t1", Role::Tool, vec![result("c1", Value::Null)]),
        msg("u2", Role::User, vec![Content::Text("again".into())]),
    ]))
    .unwrap();
    let got: Vec<(&str, &str, &str)> = rows
        .iter()
        .filter(|r| r["role"] == "tool")
        .map(|r| {
            let field = |k: &str| r[k].as_str().unwrap();
            (field("message_uid"), field("tool_call_id"), field("content"))
        })
        .collect();
    assert_eq!(got, [
        ("t1", "c1", UNCAPTURED_OUTPUT),
        ("a1#1", "c2", UNCAPTURED_OUTPUT),
        ("a1#2", "c3", UNCAPTURED_OUTPUT),
    ]);
    assert_eq!(rows.last().unwrap()["message_uid"], "u2");
}

/// A row of several results is a `tool` row each, the first under the row's id.
#[rstest]
fn a_row_of_several_results_is_several_rows() {
    let cwd = tempfile::tempdir().unwrap();
    let rows = messages(&session(cwd.path(), vec![
        msg("a1", Role::Assistant, vec![
            call("c1", "terminal", json!({"command": "ls"})),
            call("c2", "terminal", json!({"command": "pwd"})),
        ]),
        msg("t1", Role::Tool, vec![result("c1", json!("x")), result("c2", json!("/w"))]),
    ]))
    .unwrap();
    let got: Vec<&str> = rows.iter().map(|r| r["message_uid"].as_str().unwrap()).collect();
    assert_eq!(got, ["a1", "t1", "t1#1"]);
}

/// Calls captured without their input become notes, the replies they stood between merged into
/// the run's last; rows with nothing to write are left out. Each is recorded on the row it went
/// into.
#[rstest]
fn merged_and_left_out_rows_are_recorded() {
    let cwd = tempfile::tempdir().unwrap();
    let rows = messages(&session(cwd.path(), vec![
        msg("title", Role::Other("atuin-title".into()), vec![]),
        msg("u1", Role::User, vec![Content::Text("go".into())]),
        msg("a1", Role::Assistant, vec![
            Content::Text("Running.".into()),
            call("c1", "terminal", Value::Null),
        ]),
        msg("t1", Role::Tool, vec![result("c1", Value::Null)]),
        msg("a2", Role::Assistant, vec![Content::Text("Done.".into())]),
        msg("r1", Role::Assistant, vec![Content::ReasoningSummary { tokens: None }]),
    ]))
    .unwrap();
    // The run of replies ends at the reasoning-only row, which holds what they said.
    assert_eq!(rows.iter().map(|r| r["message_uid"].as_str().unwrap()).collect::<Vec<_>>(), [
        "u1", "r1"
    ]);
    let said = rows[1]["content"].as_str().unwrap();
    assert!(said.contains("Running.") && said.contains("[ran a shell command]"), "{said}");
    assert!(rows[1].get("tool_calls").is_none());
    assert_eq!(rows[0]["display_metadata"][MERGED_KEY], json!({"title": ""}));
    assert_eq!(
        rows[1]["display_metadata"][MERGED_KEY],
        json!({"a1": "r1", "a2": "r1", "t1": "r1"})
    );
}

/// A model call that failed is written as Hermes records one.
#[rstest]
fn a_failed_call_is_a_failed_turn() {
    let cwd = tempfile::tempdir().unwrap();
    let rows =
        messages(&session(cwd.path(), vec![msg("a1", Role::Assistant, vec![Content::Error(
            "401 Unauthorized".into(),
        )])]))
        .unwrap();
    assert_eq!(rows[0]["display_kind"], "failed_turn");
    assert_eq!(rows[0]["content"], "401 Unauthorized", "the reader keeps no empty row");
    assert_eq!(rows[0]["display_metadata"]["error"], "401 Unauthorized");
}

/// A fork names its original as `/branch` does, and keeps the original's row keys.
#[rstest]
#[tokio::test]
async fn a_fork_names_its_original() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut original = captured(cwd.path()).await;
    original.title = Some("probe".to_owned());
    restored(home.path(), &original).await;
    let fork = RehydrateSession {
        id: "20261110_130000_f0f0f0".to_owned(),
        fork_of: Some(ForkOf {
            id: TOOL_ERROR.to_owned(),
            atuin_id: None,
            path: None,
        }),
        ..original.clone()
    };

    let (db, row) = restored(home.path(), &fork).await;

    assert_eq!(row["parent_session_id"], TOOL_ERROR);
    assert_eq!(row["model_config"]["_branched_from"], TOOL_ERROR);
    assert_eq!(row["title"], "probe #2", "the original's title is taken");
    let read: Vec<String> = read_back(&db, &fork.id)
        .await
        .iter()
        .filter(|m| m.title().is_none())
        .map(|m| m.id().unwrap().to_string())
        .collect();
    testing::assert_nothing_new(&original.messages, read.iter().map(String::as_str));
}

/// A restore of a fork keeps its own id and names the session it was forked from, as the fork
/// did. The reader then reads it as that fork, though its first prompt opens with the marker of
/// the continuation it was forked from.
#[rstest]
#[tokio::test]
async fn a_restored_fork_names_its_original() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let marker = crate::harnesstools::continuation::marker_text(
        crate::harnesstools::continuation::tests::CLAUDE,
        "4bd6b5bd-7d34-4a39-a2d6-2e6f6a6c0b1d",
        None,
    );
    let mut restored_fork = session(cwd.path(), vec![
        msg("u1", Role::User, vec![Content::Text(marker), Content::Text("go".into())]),
        msg("a1", Role::Assistant, vec![Content::Text("Done.".into())]),
    ]);
    restored_fork.id = "20261110_213611_5a7e87".to_owned();
    restored_fork.fork_of = Some(ForkOf {
        id: "20261110_213513_eaa5a6".to_owned(),
        atuin_id: None,
        path: None,
    });

    let (db, row) = restored(home.path(), &restored_fork).await;

    assert_eq!(row["id"], "20261110_213611_5a7e87");
    assert_eq!(row["parent_session_id"], "20261110_213513_eaa5a6");
    assert_eq!(row["model_config"]["_branched_from"], "20261110_213513_eaa5a6");
    let read = read_back(&db, &restored_fork.id).await;
    let first = read.iter().find(|m| m.title().is_none()).unwrap();
    assert_eq!(first.parent_session().as_ref().map(AsRef::as_ref), Some("20261110_213513_eaa5a6"));
    assert_eq!(first.parent_kind(), Some(crate::harnesstools::session::ParentKind::Fork));
}

/// A title is kept when free, else goes one past the highest of its lineage, the bare title
/// counting as the first.
#[rstest]
#[case::free(&[], "Fix it", "Fix it")]
#[case::taken(&["Fix it"], "Fix it", "Fix it #2")]
#[case::numbered(&["Fix it", "Fix it #2", "Fix it #5"], "Fix it", "Fix it #6")]
#[case::of_a_numbered_one(&["Fix it", "Fix it #3"], "Fix it #3", "Fix it #4")]
#[case::numbered_free(&["Fix it"], "Fix it #3", "Fix it #3")]
#[case::not_a_number(&["Fix #it"], "Fix #it", "Fix #it #2")]
fn titles_go_on_their_lineage(#[case] held: &[&str], #[case] title: &str, #[case] want: &str) {
    let held = Held {
        id: false,
        titles: held.iter().map(|&t| t.to_owned()).collect(),
    };
    assert_eq!(held.free(title), want);
}

#[rstest]
#[tokio::test]
async fn the_store_names_a_titles_lineage() {
    let home = tempfile::tempdir().unwrap();
    let db = empty_store(home.path()).await;
    for (id, title) in [("a", "Fix it"), ("b", "Fix it #2"), ("c", "Fix itself"), ("d", "Other")] {
        import_into(
            &db,
            &json!({"id": id, "source": "cli", "title": title, "started_at": 1.0, "messages": []}),
        )
        .await;
    }
    let held = Held::read(&db, "a", Some("Fix it")).await.unwrap();
    assert!(held.id);
    let mut titles: Vec<_> = held.titles.into_iter().collect();
    titles.sort();
    assert_eq!(titles, ["Fix it", "Fix it #2"]);
}

/// A session the store holds already is never imported again.
#[rstest]
#[tokio::test]
async fn an_id_the_store_holds_is_not_written() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = captured(cwd.path()).await;
    restored(home.path(), &session).await;
    let hermes = FakeHermes::default();

    let err = rehydrate_with(home.path(), &session, &hermes).await.unwrap_err();

    assert!(matches!(err, RehydrateError::AlreadyExists(db) if db == home.path().join(DB_FILE)));
    assert!(hermes.calls().is_empty(), "ran hermes: {:?}", hermes.ran());
}

/// A title clash that the store reports is tried once more under the next title of its lineage.
/// Another session took the title since it was read.
#[rstest]
#[tokio::test]
async fn a_title_taken_meanwhile_is_tried_again_under_the_next() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let db = empty_store(home.path()).await;
    let rows = Arc::new(Mutex::new(Vec::new()));
    let clash: Reply = {
        let db = db.clone();
        Box::new(move |_: &[&str]| {
            // Another session takes the title between the read and the import.
            let db = db.clone();
            std::thread::spawn(move || {
                tokio::runtime::Runtime::new().unwrap().block_on(import_into(
                    &db,
                    &json!({"id": "other", "source": "cli", "title": "fix the flaky test", "started_at": 1.0, "messages": []}),
                ));
            })
            .join()
            .unwrap();
            Ok(said(r#"{"ok": false, "error": "storage_unavailable"}"#, 1))
        })
    };
    let hermes = FakeHermes::replying([clash, importing(&rows, imported())]);
    let session =
        session(cwd.path(), vec![msg("u1", Role::User, vec![Content::Text("go".into())])]);

    rehydrate_with(home.path(), &session, &hermes).await.unwrap();

    assert_eq!(rows.lock()[0]["title"], "fix the flaky test #2");
    assert_eq!(hermes.calls().len(), 2);
}

/// What `hermes atuin import` says when nothing was written, as an error.
#[rstest]
#[case::no_plugin(CliOutput { code: Some(2), stdout: String::new(), stderr: "hermes: 'atuin' is not a `hermes` command.\n".into() }, "atuin hook install hermes")]
#[case::refused(said(r#"{"ok": false, "error": "revision_conflict"}"#, 1), "already exists")]
#[case::skipped(
    said(r#"{"ok": false, "error": "skipped", "receipt": {"imported": 0}}"#, 1),
    "already exists"
)]
#[case::deleted(
    said(r#"{"ok": false, "error": "admission_conflict: a deleted session is not restored"}"#, 1),
    "fork it instead"
)]
#[case::storage(said(r#"{"ok": false, "error": "storage_unavailable"}"#, 1), "storage_unavailable")]
#[case::old_hermes(
    said(r#"{"ok": false, "error": "unsupported_hermes: no session store"}"#, 1),
    "too new or too old"
)]
#[case::garbled(said("Traceback (most recent call last):", 1), "Traceback")]
#[tokio::test]
async fn a_refusal_says_why(#[case] output: CliOutput, #[case] want: &str) {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let replies: Vec<Reply> = (0..2)
        .map(|_| {
            let output = output.clone();
            Box::new(move |_: &[&str]| Ok(output.clone())) as Reply
        })
        .collect();
    let hermes = FakeHermes::replying(replies);
    let session =
        session(cwd.path(), vec![msg("u1", Role::User, vec![Content::Text("go".into())])]);

    let err = rehydrate_with(home.path(), &session, &hermes).await.unwrap_err();

    assert!(err.to_string().contains(want), "{err}");
}

#[rstest]
#[tokio::test]
async fn no_hermes_says_so() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session =
        session(cwd.path(), vec![msg("u1", Role::User, vec![Content::Text("go".into())])]);
    let err = rehydrate_with(home.path(), &session, &FakeHermes::missing()).await.unwrap_err();
    assert!(err.to_string().contains("not on your PATH"), "{err}");
}

/// A session row of a prompt, then a `terminal` call per entry of `sizes` answered by a result
/// of that many bytes of output, the `n`th exiting `n`.
fn big(sizes: &[usize]) -> Value {
    let cwd = tempfile::tempdir().unwrap();
    let mut rows = vec![msg("u1", Role::User, vec![Content::Text("go".into())])];
    for (n, size) in sizes.iter().enumerate() {
        rows.push(msg(&format!("a{n}"), Role::Assistant, vec![call(
            &format!("c{n}"),
            "terminal",
            json!({"command": format!("cat big{n}")}),
        )]));
        let output = format!(
            "{{\"output\": {}, \"exit_code\": {n}, \"error\": null}}",
            json!("x".repeat(*size))
        );
        rows.push(msg(&format!("t{n}"), Role::Tool, vec![result(&format!("c{n}"), json!(output))]));
    }
    let session = session(cwd.path(), rows);
    let messages = messages(&session).unwrap();
    session_row(&session, Some("big"), None, &messages)
}

/// The size Hermes measures, against which the limit is checked: compact UTF-8 JSON, non-ASCII
/// as itself.
#[rstest]
fn the_row_is_measured_as_hermes_measures_it() {
    assert_eq!(measured(&json!({"a": [1, "é"], "b": null})), r#"{"a":[1,"é"],"b":null}"#.len());
}

/// A row that fits goes as it is; one a byte over has its oldest result trimmed, and no more. The
/// edge is [`SIZE_MARGIN`] under Hermes's 5 MiB, so a row that just fits here fits there too.
#[rstest]
fn a_row_just_over_the_limit_loses_its_oldest_output() {
    const MIB: usize = 1024 * 1024;
    assert_eq!(FITS, 5 * MIB - 64 * 1024);
    let slack = FITS - measured(&big(&[MIB, MIB, MIB]));

    let mut row = big(&[MIB + slack, MIB, MIB]);
    assert_eq!(measured(&row), FITS);
    let same = row.clone();
    assert_eq!(fit(&mut row).unwrap(), 0);
    assert_eq!(row, same);

    let mut row = big(&[MIB + slack + 1, MIB, MIB]);
    assert_eq!(measured(&row), FITS + 1);
    let before = row.clone();

    assert_eq!(fit(&mut row).unwrap(), 1);

    assert!(measured(&row) <= FITS);
    let trimmed: Value =
        serde_json::from_str(row["messages"][2]["content"].as_str().unwrap()).unwrap();
    assert_eq!(trimmed, json!({"output": TRIMMED_OUTPUT, "exit_code": 0, "error": null}));
    for at in [4, 6] {
        assert_eq!(row["messages"][at], before["messages"][at], "a later result was trimmed");
    }
}

/// Trimming takes results oldest first, until the row fits, and never touches prompts, replies,
/// calls, or a `terminal` result's exit code and error.
#[rstest]
fn results_are_trimmed_oldest_first_keeping_the_rest() {
    let mut row = big(&[1024 * 1024; 8]);
    let before = row.clone();

    let trimmed = fit(&mut row).unwrap();

    assert_eq!(trimmed, 4, "8 MiB of output, under 5 MiB allowed");
    assert!(measured(&row) <= FITS);
    let messages = row["messages"].as_array().unwrap();
    for (at, (now, was)) in messages.iter().zip(before["messages"].as_array().unwrap()).enumerate()
    {
        assert_eq!(now["message_uid"], was["message_uid"]);
        if now["role"] != "tool" {
            assert_eq!(now, was, "row {at} changed");
            continue;
        }
        let n: usize = now["message_uid"].as_str().unwrap()[1..].parse().unwrap();
        let output: Value = serde_json::from_str(now["content"].as_str().unwrap()).unwrap();
        assert_eq!(output["exit_code"], n, "the exit code changed");
        assert_eq!(output["error"], Value::Null);
        assert_eq!(output["output"] == TRIMMED_OUTPUT, n < 4, "result {n}");
    }
}

/// A row inside the margin under Hermes's 5 MiB, with no tool output to trim, is refused: it
/// may be over as Hermes measures it.
#[rstest]
fn a_row_inside_the_margin_is_not_sent() {
    let mut row = big(&[]);
    let pad = FITS + 1 - measured(&row);
    let prompt = row["messages"][0]["content"].as_str().unwrap().to_owned();
    row["messages"][0]["content"] = json!(format!("{prompt}{}", "x".repeat(pad)));
    assert_eq!(measured(&row), FITS + 1);
    const { assert!(FITS + 1 < MAX_BYTES) };
    let err = fit(&mut row).unwrap_err();
    assert!(err.to_string().contains("5 MiB"), "{err}");
}

/// A result of another tool is trimmed to the marker alone.
#[rstest]
fn another_tools_result_is_the_marker() {
    assert_eq!(
        trim_result(r#"{"content": "1|x", "exit_code": 0}"#, Some("read_file")),
        TRIMMED_OUTPUT
    );
    assert_eq!(trim_result("not json", Some("terminal")), TRIMMED_OUTPUT);
}

/// Past the limit with every result trimmed, or past 10,000 rows, the session is refused,
/// naming the limit, rather than cut short.
#[rstest]
fn a_session_past_the_limits_is_refused() {
    let mut row = big(&[10]);
    row["messages"][0]["content"] = json!("x".repeat(MAX_BYTES));
    let err = fit(&mut row).unwrap_err();
    assert!(err.to_string().contains("5 MiB"), "{err}");

    let cwd = tempfile::tempdir().unwrap();
    let rows = (0..=MAX_MESSAGES)
        .map(|n| msg(&format!("u{n}"), Role::User, vec![Content::Text("go".into())]))
        .collect();
    let err = messages(&session(cwd.path(), rows)).unwrap_err();
    assert!(err.to_string().contains("10000"), "{err}");
}

/// A restore of a session over the limit is imported trimmed.
#[rstest]
#[tokio::test]
async fn a_big_session_is_imported_trimmed() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut rows = vec![msg("u1", Role::User, vec![Content::Text("go".into())])];
    for n in 0..6 {
        rows.push(msg(&format!("a{n}"), Role::Assistant, vec![call(
            &format!("c{n}"),
            "read_file",
            json!({"path": "f"}),
        )]));
        rows.push(msg(&format!("t{n}"), Role::Tool, vec![result(
            &format!("c{n}"),
            json!("z".repeat(1024 * 1024)),
        )]));
    }
    let (written, row) = restored_as(home.path(), &session(cwd.path(), rows)).await;
    assert!(measured(&row) <= FITS);
    let note = written.note.unwrap();
    assert!(note.starts_with("trimmed 2 tool outputs to fit Hermes's 5 MiB"), "{note}");
    assert_eq!(uids(&row).len(), 13);
    assert_eq!(row["messages"][2]["content"], TRIMMED_OUTPUT);
    assert_ne!(row["messages"][12]["content"], TRIMMED_OUTPUT);
}
