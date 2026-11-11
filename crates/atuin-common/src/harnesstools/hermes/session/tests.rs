use std::collections::HashMap;
use std::num::NonZeroUsize;

use futures::{StreamExt, TryStreamExt};
use pretty_assertions::assert_eq;
use rstest::rstest;
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection, SqliteJournalMode};

use super::*;
use crate::path::PathBufExt as _;

const SCHEMA: &str = include_str!("../../../../tests/fixtures/hermes/schema.sql");
const PROBE: &str = include_str!("../../../../tests/fixtures/hermes/probe.sql");
const COMPACTION: &str = include_str!("../../../../tests/fixtures/hermes/compaction.sql");
const LEGACY: &str = include_str!("../../../../tests/fixtures/hermes/legacy.sql");

/// The probe's session with one shell command, resumed for a second turn, then `/undo`ne.
const RESUMED: &str = "local-8baebc867cd817b810a10420638858ba79ef3e78ad6ada513ccf9d79bd31d565";
/// The probe's session with a failing `ls`, a `cd` and a command after it.
pub const TOOL_ERROR: &str =
    "local-524543fc0c9eeda01f40f2cd88fefc201a75a0fd59817a174c4f18e0ebb80136";
/// The probe's session whose turn failed on the provider's 401.
const FAILED: &str = "local-86e8f7d82d63ddc82b3bed02f8e342917838bccabbcc3d3ca79ce9d35a2c1d62";
/// The probe's session that delegated a task, and the subagent's.
const DELEGATOR: &str = "local-56e64527431b680fad9f17881150e9234ddbca30777b3d57949bd767d30315e1";
const SUBAGENT: &str = "20261109_094652_9ca0a5";

fn pool() -> BlockingPool {
    BlockingPool::new(NonZeroUsize::MIN)
}

/// A writer to the store at `db`, as Hermes opens one: Python's `sqlite3` leaves foreign keys
/// off, and the fixtures name system prompts they leave out.
async fn connect(db: &Path) -> SqliteConnection {
    let opts = SqliteConnectOptions::new()
        .filename(db)
        .create_if_missing(true)
        .foreign_keys(false)
        .journal_mode(SqliteJournalMode::Wal);
    SqliteConnection::connect_with(&opts).await.unwrap()
}

/// Run `sql` against `conn`, as Hermes would write it.
async fn execute(conn: &mut SqliteConnection, sql: &str) {
    // Not through `crate::db::query`, which takes only static SQL: these stand for Hermes's
    // own writes.
    #[allow(clippy::disallowed_methods)]
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned())).execute(conn).await.unwrap();
}

/// A WAL store at `dir/state.db` holding the probe's sessions.
pub async fn probe_store(dir: &Path) -> PathBuf {
    store_in(dir, &[SCHEMA, PROBE]).await
}

/// Run `sql` against the store at `db`, as Hermes would write it, and close the writer.
pub async fn execute_in(db: &Path, sql: &str) {
    let mut conn = connect(db).await;
    execute(&mut conn, sql).await;
    conn.close().await.unwrap();
}

/// A WAL store at `dir/state.db` holding `sqls`, with its writer closed.
async fn store_in(dir: &Path, sqls: &[&str]) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let db = dir.join(DB_FILE);
    let mut conn = connect(&db).await;
    for sql in sqls {
        execute(&mut conn, sql).await;
    }
    conn.close().await.unwrap();
    db
}

/// Every message a read of session `id` of `db` yields.
async fn read(db: &Path, id: &str) -> Vec<HermesMessage> {
    HermesSession::open(SessionId::from(id.to_owned()), db).read().try_collect().await.unwrap()
}

fn ids(messages: &[HermesMessage]) -> Vec<String> {
    messages.iter().map(|m| m.id().unwrap().to_string()).collect()
}

/// Each `message_uid` in the store with the first row id holding it, so tests can name the rows
/// the reader keys on their uids by their ids.
async fn row_ids(db: &Path) -> HashMap<String, String> {
    let mut conn = connect(db).await;
    let rows: Vec<(String, i64)> = crate::db::query_as(
        "SELECT message_uid, MIN(id) FROM messages WHERE message_uid IS NOT NULL GROUP BY \
         message_uid",
    )
    .fetch_all(&mut conn)
    .await
    .unwrap();
    conn.close().await.unwrap();
    rows.into_iter().map(|(uid, id)| (uid, id.to_string())).collect()
}

/// `links` (see [`parents`]) with each key a uid named by its row id (see [`row_ids`]).
fn by_row_id(
    rows: &HashMap<String, String>,
    links: Vec<(String, Option<String>)>,
) -> Vec<(String, Option<String>)> {
    let id = |key: String| rows.get(&key).cloned().unwrap_or(key);
    links.into_iter().map(|(key, parent)| (id(key), parent.map(id))).collect()
}

fn parents(messages: &[HermesMessage]) -> Vec<(String, Option<String>)> {
    messages
        .iter()
        .filter(|m| m.title().is_none())
        .map(|m| (m.id().unwrap().to_string(), m.parent_id().map(|p| p.to_string())))
        .collect()
}

fn text(content: &[Content]) -> Vec<&str> {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[rstest]
#[tokio::test]
async fn a_shell_command_maps_to_a_call_and_its_result() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[SCHEMA, PROBE]).await;
    let messages = read(&db, RESUMED).await;
    let first: Vec<_> = messages.iter().take(4).collect();

    assert_eq!(first[0].role(), Role::User);
    assert!(text(&first[0].content())[0].starts_with("Use the terminal tool to run exactly"));
    assert_eq!(first[0].model(), None, "only the model's rows name it");

    assert_eq!(first[1].role(), Role::Assistant);
    let reply = first[1].content();
    let [Content::Reasoning(_), Content::ToolUse(call)] = reply.as_slice() else {
        panic!("{reply:?}");
    };
    assert_eq!(call.id.as_ref(), "call-5cdd0f45-1ad3-4c81-8d35-1439bdf166bf");
    assert_eq!(call.name, "terminal");
    assert_eq!(
        call.input,
        serde_json::json!({"command": "cat README.txt && git branch --show-current ."})
    );
    assert_eq!(first[1].stop_reason(), Some(StopReason::ToolUse));
    assert_eq!(first[1].model().as_deref(), Some("xiaomi/mimo-v2.5-pro"));
    assert_eq!(first[1].provider().as_deref(), Some("openrouter"));

    assert_eq!(first[2].role(), Role::Tool);
    let output = first[2].content();
    let [Content::ToolResult(result)] = output.as_slice() else {
        panic!("{output:?}");
    };
    assert_eq!(result.call, call.id);
    assert!(!result.error);
    assert!(result.output_text().unwrap().contains("hello from probe"));

    assert_eq!(text(&first[3].content()), ["Output:\n\nhello from probe\nprobe-branch"]);
    assert_eq!(first[3].stop_reason(), Some(StopReason::EndTurn));

    for message in &messages {
        assert_eq!(message.cwd().as_deref(), Some(Path::new("/tmp/hermes-play/work")));
    }
}

/// The turn `/undo` dropped stays, as a branch hanging from the turn before it.
#[rstest]
#[tokio::test]
async fn an_undone_turn_is_a_dead_branch() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[SCHEMA, PROBE]).await;
    let messages = read(&db, RESUMED).await;
    let link = |id: &str, parent: Option<&str>| (id.to_owned(), parent.map(str::to_owned));
    assert_eq!(by_row_id(&row_ids(&db).await, parents(&messages)), [
        link("3", None),
        link("4", Some("3")),
        link("5", Some("4")),
        link("6", Some("5")),
        link("15", Some("6")),
        link("16", Some("15")),
        link("17", Some("16")),
        link("18", Some("17")),
        link("19", Some("18")),
        link("20", Some("19")),
    ]);
}

/// Rows are keyed on their `message_uid`, which an import keeps as given: a session atuin
/// writes into a store reads back under the keys it was written from, each row hanging from the
/// one before by its key. A row with no uid is keyed on its id.
#[rstest]
#[tokio::test]
async fn rows_read_back_under_their_message_uids() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[SCHEMA]).await;
    let mut writer = connect(&db).await;
    execute(
        &mut writer,
        "INSERT INTO sessions (id, source, started_at) VALUES ('restored', 'cli', 1.0);
         INSERT INTO messages (id, session_id, role, content, timestamp, message_uid) VALUES
             (61, 'restored', 'user', 'hello', 1.0, 'toolu_01/abc'),
             (62, 'restored', 'assistant', 'hi', 2.0, '7f3c-a2'),
             (63, 'restored', 'user', 'and again', 3.0, NULL);",
    )
    .await;
    writer.close().await.unwrap();
    let messages = read(&db, "restored").await;
    let link = |id: &str, parent: Option<&str>| (id.to_owned(), parent.map(str::to_owned));
    assert_eq!(parents(&messages), [
        link("toolu_01/abc", None),
        link("7f3c-a2", Some("toolu_01/abc")),
        link("63", Some("7f3c-a2")),
    ]);
}

/// A row read live hangs from the newest row still live when it lands, whatever was read after
/// that: a turn after `/undo` goes on from before the undone turn.
#[rstest]
#[tokio::test]
async fn a_turn_after_an_undo_hangs_from_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[SCHEMA, PROBE]).await;
    let mut writer = connect(&db).await;
    execute(&mut writer, "UPDATE messages SET active = 1 WHERE id BETWEEN 15 AND 20").await;
    let (wake, woken) = watch::channel(0);
    let session = HermesSession {
        changes: Some(woken),
        ..HermesSession::open(SessionId::from(RESUMED.to_owned()), &db)
    };
    let mut messages = Box::pin(session.messages());
    let mut next = async || {
        tokio::time::timeout(Duration::from_secs(5), messages.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    };
    for _ in 0..10 {
        next().await;
    }
    execute(
        &mut writer,
        &format!(
            "UPDATE messages SET active = 0 WHERE id BETWEEN 15 AND 20;
             INSERT INTO messages (id, session_id, role, content, timestamp, message_uid)
                 VALUES (41, '{RESUMED}', 'user', 'try again', 1794246600.0, 'u-41');"
        ),
    )
    .await;
    wake.send_modify(|n| *n += 1);
    let again = next().await;
    let rows = row_ids(&db).await;
    assert_eq!(again.id().unwrap().as_ref(), "u-41");
    assert_eq!(rows[again.parent_id().unwrap().as_ref()], "6");
    writer.close().await.unwrap();
}

/// A store rewound under a read (a backup restored after capture read past its end) hands out
/// row ids the read has passed again. The read sees its last row changed and starts over, rather
/// than skip the new rows that reuse old ids.
#[rstest]
#[tokio::test]
async fn a_store_rewound_under_a_read_starts_it_over() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[SCHEMA, PROBE]).await;
    let first = read(&db, RESUMED).await;
    let mut writer = connect(&db).await;
    let (wake, woken) = watch::channel(0);
    let session = HermesSession {
        changes: Some(woken),
        ..HermesSession::open(SessionId::from(RESUMED.to_owned()), &db)
    };
    let mut messages = Box::pin(session.messages_from(None));
    let mut next = async || {
        tokio::time::timeout(Duration::from_secs(5), messages.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    };
    for _ in 0..first.len() {
        assert_eq!(next().await.0.generation, 0);
    }
    execute(
        &mut writer,
        &format!(
            "DELETE FROM messages WHERE session_id = '{RESUMED}' AND id >= 15;
             INSERT INTO messages (id, session_id, role, content, timestamp, message_uid)
                 VALUES (15, '{RESUMED}', 'user', 'after the restore', 1794246700.0, 'u-r15');"
        ),
    )
    .await;
    wake.send_modify(|n| *n += 1);

    let (at, again) = next().await;
    assert_eq!(at.generation, 1, "the read started over");
    assert_eq!(again.id(), first[0].id());
    let mut rest = vec![again];
    while rest.last().and_then(Message::id).is_none_or(|id| id.as_ref() != "u-r15") {
        rest.push(next().await.1);
    }
    let restored = rest.last().unwrap();
    assert_eq!(restored.content(), [Content::Text("after the restore".to_owned())]);
    writer.close().await.unwrap();
}

#[rstest]
#[tokio::test]
async fn a_failing_command_is_an_error_result() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[SCHEMA, PROBE]).await;
    let messages = read(&db, TOOL_ERROR).await;
    let results: Vec<(String, bool)> = messages
        .iter()
        .flat_map(|m| m.content())
        .filter_map(|c| match c {
            Content::ToolResult(r) => Some((r.call.to_string(), r.error)),
            _ => None,
        })
        .collect();
    assert_eq!(results, [
        ("call-50b5552b-7f41-4258-8684-95d89f27b5cb".to_owned(), true),
        ("call-8f1024c2-8a84-4c80-8c6f-1507d60bd6df".to_owned(), false),
        ("call-01ec7f17-ec96-459b-87b2-2693b2c951c6".to_owned(), false),
    ]);
    // A reply that says something beside its calls keeps both.
    assert_eq!(text(&messages[1].content()), ["I'll run them in order as separate calls."]);
}

#[rstest]
#[case::exit(r#"{"output": "", "exit_code": 2, "error": null}"#, true)]
#[case::ok(r#"{"output": "x", "exit_code": 0, "error": null}"#, false)]
#[case::error(r#"{"error": "command is required"}"#, true)]
#[case::blocked(r#"{"output": "", "exit_code": null, "error": null, "status": "blocked"}"#, true)]
#[case::background(
    r#"{"output": "", "exit_code": null, "error": null, "status": "yielded_to_background"}"#,
    false
)]
#[case::context_appended(
    "{\"content\": \"x\"}\n\n[Subdirectory context discovered: ~/AGENTS.md]\nrules",
    false
)]
#[case::plain_text("no such file", false)]
fn a_result_fails_by_what_its_json_says(#[case] output: &str, #[case] want: bool) {
    assert_eq!(failed(output), want);
}

#[rstest]
#[tokio::test]
async fn a_turn_the_provider_refused_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[SCHEMA, PROBE]).await;
    let messages = read(&db, FAILED).await;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].content(), [Content::Error("HTTP 401: Invalid API Key".to_owned())]);
    assert_eq!(messages[1].stop_reason(), Some(StopReason::Error));
    assert_eq!(messages[1].provider().as_deref(), Some("xiaomi"));
}

/// Summaries, hidden scaffolding, compaction copies, lurk rows and empty rows are left out; a
/// row cloned into the live transcript still counts as live, so the turn after hangs from it.
#[rstest]
#[tokio::test]
async fn a_compaction_keeps_the_conversation_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[SCHEMA, COMPACTION]).await;
    let messages = read(&db, "local-compacted").await;
    let rows = row_ids(&db).await;
    let link = |id: &str, parent: Option<&str>| (id.to_owned(), parent.map(str::to_owned));
    assert_eq!(by_row_id(&rows, parents(&messages)), [
        link("101", None),
        link("102", Some("101")),
        link("103", Some("102")),
        link("104", Some("103")),
        link("108", Some("104")),
        link("112", Some("108")),
        link("113", Some("112")),
    ]);
    let steer = messages
        .iter()
        .find(|m| rows.get(m.id().unwrap().as_ref()).is_some_and(|id| id == "112"))
        .unwrap();
    assert_eq!(steer.role(), Role::User);
    let title = messages.iter().find_map(|m| m.title()).unwrap();
    assert_eq!(title, TitleChange::new(TitleSource::Generated, "Compaction probe"));
}

/// A session a compaction rotated into names the one it carries on, on every row.
#[rstest]
#[tokio::test]
async fn a_rotated_compaction_continues_its_parent() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[SCHEMA, COMPACTION]).await;
    let parent = read(&db, "20260511_171303_982a49").await;
    assert!(parent.iter().all(|m| m.parent_session().is_none()));
    let child = read(&db, "20260511_172545_31d799").await;
    assert_eq!(ids(&child)[..2], ["u-203", "u-204"]);
    for message in &child {
        assert_eq!(message.parent_session().unwrap().as_ref(), "20260511_171303_982a49");
        assert_eq!(message.parent_kind(), Some(ParentKind::Continuation));
    }
    assert_eq!(child[0].parent_id(), None);
}

#[rstest]
#[tokio::test]
async fn only_coding_sessions_and_their_subagents_are_threads() {
    let dir = tempfile::tempdir().unwrap();
    store_in(dir.path(), &[SCHEMA, PROBE, COMPACTION]).await;
    let sessions = HermesSessions::builder().home(dir.path()).pool(pool()).build();
    let mut found: Vec<String> =
        sessions.existing().unwrap().map_ok(|s| s.id().to_string()).try_collect().await.unwrap();
    found.sort();
    let mut want = vec![
        "20260511_171303_982a49",
        "20260511_172545_31d799",
        DELEGATOR,
        FAILED,
        RESUMED,
        SUBAGENT,
        TOOL_ERROR,
        "local-compacted",
        "local-desktop",
    ];
    want.sort_unstable();
    assert_eq!(found, want);
}

/// Every row of a subagent, its title row too, names the session that delegated to it.
#[rstest]
#[tokio::test]
async fn a_subagents_rows_name_the_session_it_works_for() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[SCHEMA, PROBE]).await;
    let messages = read(&db, SUBAGENT).await;
    assert!(!messages.is_empty());
    for message in &messages {
        assert_eq!(message.parent_session().as_ref().map(AsRef::as_ref), Some(DELEGATOR));
        assert_eq!(message.parent_kind(), Some(ParentKind::Subagent));
    }
    let delegator = read(&db, DELEGATOR).await;
    assert!(delegator.iter().all(|m| m.parent_session().is_none()));
}

/// A subagent `delegate_task` starts while capture runs is reported as it appears, a child of the
/// session it works for.
#[rstest]
#[tokio::test]
async fn the_listener_reports_a_subagent_as_it_appears() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[SCHEMA, COMPACTION]).await;
    let listener = HermesListener {
        root: dir.path().to_path_buf(),
        profile_scan: Duration::from_millis(100),
    };
    let (tx, mut watched) = tokio::sync::mpsc::unbounded_channel();
    let listening = tokio::spawn(async move {
        let mut sessions = Box::pin(listener.watch());
        while let Some(session) = sessions.next().await {
            let _ = tx.send(session.unwrap());
        }
    });
    let mut reported = async || {
        tokio::time::timeout(Duration::from_secs(10), watched.recv())
            .await
            .expect("a session is reported")
            .unwrap()
    };
    for _ in 0..4 {
        reported().await;
    }

    let mut writer = connect(&db).await;
    execute(
        &mut writer,
        r#"INSERT INTO sessions (id, source, model_config, parent_session_id, started_at)
               VALUES ('sa-live', 'subagent', '{"_delegate_from": "local-desktop"}', 'local-desktop', 9.0);
           INSERT INTO messages (id, session_id, role, content, timestamp)
               VALUES (900, 'sa-live', 'user', 'count the lines', 9.0);"#,
    )
    .await;
    let child = reported().await;
    assert_eq!(child.id().as_ref(), "sa-live");
    let rows: Vec<HermesMessage> = child.read().try_collect().await.unwrap();
    assert_eq!(rows[0].parent_session().as_ref().map(AsRef::as_ref), Some("local-desktop"));
    assert_eq!(rows[0].parent_kind(), Some(ParentKind::Subagent));
    writer.close().await.unwrap();
    listening.abort();
}

/// A subagent is a thread when the session it works for is one, however deep, and whichever
/// field names that session (`_delegate_from`, or before v0.23's re-tagging only
/// `parent_session_id`); not a gateway chat's subagent, nor one Hermes marked orphaned.
#[rstest]
#[tokio::test]
async fn a_subagent_is_a_thread_by_the_session_it_works_for() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[SCHEMA, PROBE]).await;
    let mut writer = connect(&db).await;
    let delegate = |from: &str| format!(r#"'{{"_delegate_from": "{from}"}}'"#);
    execute(
        &mut writer,
        &format!(
            "INSERT INTO sessions (id, source, model_config, parent_session_id, started_at) VALUES
                 ('tg-chat', 'telegram', NULL, NULL, 1.0),
                 ('sa-of-chat', 'subagent', {}, 'tg-chat', 2.0),
                 ('sa-nested', 'subagent', {}, '{SUBAGENT}', 3.0),
                 ('sa-untagged', 'subagent', NULL, '{DELEGATOR}', 4.0),
                 ('sa-orphaned', 'subagent', {}, NULL, 5.0);",
            delegate("tg-chat"),
            delegate(SUBAGENT),
            delegate("__orphaned__"),
        ),
    )
    .await;
    writer.close().await.unwrap();
    let sessions = HermesSessions::builder().home(dir.path()).pool(pool()).build();
    let found: Vec<String> =
        sessions.existing().unwrap().map_ok(|s| s.id().to_string()).try_collect().await.unwrap();
    for id in ["sa-nested", "sa-untagged"] {
        assert!(found.iter().any(|f| f == id), "{id} is a thread: {found:?}");
    }
    for id in ["tg-chat", "sa-of-chat", "sa-orphaned"] {
        assert!(!found.iter().any(|f| f == id), "{id} is no thread: {found:?}");
    }
    // One session asked for walks up its own chain, and comes to the same answer.
    let mut store = Store::open(&db).await.unwrap();
    for (id, thread) in [
        (SUBAGENT, true),
        ("sa-nested", true),
        ("sa-untagged", true),
        ("tg-chat", false),
        ("sa-of-chat", false),
        ("sa-orphaned", false),
    ] {
        assert_eq!(store.session(id).await.unwrap().is_some(), thread, "{id}");
    }
    store.close().await;
}

/// Finding the threads stays linear in the store's sessions: a store a busy cron job fills in a
/// month (every run is a session) lists in well under a second, where walking every chain from
/// every thread took seconds at a tenth the size.
#[rstest]
#[tokio::test]
async fn a_large_store_lists_its_threads_quickly() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[SCHEMA, PROBE]).await;
    let mut writer = connect(&db).await;
    execute(
        &mut writer,
        r"WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 20000)
           INSERT INTO sessions (id, source, created_source, model_config, parent_session_id, started_at)
           SELECT 's' || i,
                  CASE i % 4 WHEN 0 THEN 'cron' WHEN 1 THEN 'oneshot' ELSE 'subagent' END,
                  CASE i % 4 WHEN 0 THEN 'cron' WHEN 1 THEN 'oneshot' ELSE 'subagent' END,
                  CASE WHEN i % 4 > 1 THEN json_object('_delegate_from', 's' || (i - i % 4 + 1)) END,
                  CASE WHEN i % 4 > 1 THEN 's' || (i - i % 4 + 1) END,
                  i
           FROM n;",
    )
    .await;
    writer.close().await.unwrap();

    let started = std::time::Instant::now();
    let mut store = Store::open(&db).await.unwrap();
    let threads = store.sessions().await.unwrap();
    let one = store.session("s19998").await.unwrap();
    store.close().await;
    // 5,000 one-shot runs and their 10,000 subagents, besides the probe's.
    assert!(threads.len() > 15_000, "{}", threads.len());
    assert!(one.is_some());
    assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
}

#[rstest]
#[case::untitled(None, None, None)]
#[case::user(Some("Fix it"), Some("user"), Some(TitleSource::Named))]
#[case::legacy(Some("Fix it"), None, Some(TitleSource::Named))]
#[case::derived(Some("Fix it"), Some("derived"), Some(TitleSource::Generated))]
#[case::llm(Some("Fix it"), Some("llm"), Some(TitleSource::Generated))]
#[case::blank(Some("  "), Some("user"), None)]
fn a_title_ranks_by_who_wrote_it(
    #[case] title: Option<&str>,
    #[case] source: Option<&str>,
    #[case] want: Option<TitleSource>,
) {
    let row = SessionRow {
        id: "s".to_owned(),
        title: title.map(str::to_owned),
        title_source: source.map(str::to_owned),
        cwd: None,
        model: None,
        model_config: None,
        parent: None,
        parent_end_reason: None,
        delegated_by: None,
        started_at: 0.0,
        ended_at: None,
        last_activity_at: None,
        tip: None,
        tip_at: None,
    };
    assert_eq!(title_of(&row).map(|t| t.source), want);
}

/// A `profiles` directory that can't be read makes the scan say it is partial, rather than pass
/// for a home with no profiles.
#[cfg(unix)]
#[rstest]
#[tokio::test]
async fn an_unreadable_profiles_directory_is_an_error() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    store_in(dir.path(), &[SCHEMA, PROBE]).await;
    let profiles = dir.path().join(PROFILES_DIR);
    std::fs::create_dir(&profiles).unwrap();
    std::fs::set_permissions(&profiles, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&profiles).is_ok() {
        eprintln!("skipped: this user reads a directory with no permissions (root?)");
        return;
    }
    let sessions = HermesSessions::builder().home(dir.path()).pool(pool()).build();
    let scanned: Vec<_> = sessions.existing().unwrap().collect().await;
    std::fs::set_permissions(&profiles, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(scanned.iter().any(Result::is_ok), "the home's own store still reads");
    assert!(scanned.iter().any(Result::is_err), "the profiles scan is reported");
}

/// A store from before v0.22 (no `created_source`, no `message_uid`) reads as far as its columns
/// allow.
#[rstest]
#[tokio::test]
async fn a_store_from_before_message_uids_still_reads() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[LEGACY]).await;
    let sessions = HermesSessions::builder().home(dir.path()).pool(pool()).build();
    let mut found: Vec<String> =
        sessions.existing().unwrap().map_ok(|s| s.id().to_string()).try_collect().await.unwrap();
    found.sort();
    assert_eq!(found, ["20260514_220652_40bb26", "20260514_230000_aaaaaa"]);
    let messages = read(&db, "20260514_220652_40bb26").await;
    assert_eq!(ids(&messages), ["1", "2", "3", "4", &ids(&messages)[4]]);
    assert_eq!(messages[4].title(), Some(TitleChange::new(TitleSource::Named, "Old CLI session")));
    assert!(
        matches!(messages[1].content().as_slice(), [Content::ToolUse(call)] if call.name == "terminal")
    );
}

/// A read resumed past a checkpoint yields what a read from the start yields after it, links
/// included; one whose row is gone reads from the start, as a read started over.
#[rstest]
#[tokio::test]
async fn a_read_resumes_where_its_checkpoint_holds() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[SCHEMA, PROBE]).await;
    let session = || HermesSession::open(SessionId::from(RESUMED.to_owned()), &db);
    let whole: Vec<(Checkpoint, HermesMessage)> =
        session().messages_from(None).try_collect().await.unwrap();
    let from = whole[3].0;

    let resumed: Vec<(Checkpoint, HermesMessage)> =
        session().messages_from(Some(from)).try_collect().await.unwrap();
    let rows = |items: &[(Checkpoint, HermesMessage)]| {
        parents(&items.iter().map(|(_, m)| m.clone()).collect::<Vec<_>>())
    };
    assert_eq!(rows(&resumed), rows(&whole)[4..]);
    assert!(resumed.iter().all(|(at, _)| at.generation == 0));

    let gone = Checkpoint::new(from.at, b"another row");
    let over: Vec<(Checkpoint, HermesMessage)> =
        session().messages_from(Some(gone)).try_collect().await.unwrap();
    assert_eq!(rows(&over), rows(&whole));
    assert!(over.iter().all(|(at, _)| at.generation == 1));
}

/// Once the last Hermes connection closes, SQLite removes the `-wal` and `-shm` files; the store
/// still opens.
#[rstest]
#[tokio::test]
async fn a_store_without_its_sidecars_opens() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[SCHEMA, PROBE]).await;
    assert!(!dir.path().join(WAL_FILE).exists(), "the writer's close removes the log");
    assert!(!dir.path().join("state.db-shm").exists());
    assert_eq!(read(&db, FAILED).await.len(), 2);
}

#[rstest]
#[tokio::test]
async fn locate_finds_the_store_holding_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let home = store_in(dir.path(), &[SCHEMA, PROBE]).await;
    let work = store_in(&dir.path().join("profiles").joined("work"), &[SCHEMA, COMPACTION]).await;
    assert_eq!(locate(dir.path(), RESUMED).await, Some(home));
    assert_eq!(locate(dir.path(), "local-compacted").await, Some(work));
    assert_eq!(locate(dir.path(), "no-such-session").await, None);
}

#[rstest]
#[case::text("plain", vec![Content::Text("plain".to_owned())])]
#[case::empty("", vec![])]
#[case::parts(
    "\u{0}json:[{\"type\": \"text\", \"text\": \"look\"}, {\"type\": \"image_url\", \"image_url\": {\"url\": \"x\"}}]",
    vec![
        Content::Text("look".to_owned()),
        Content::Other(serde_json::json!({"type": "image_url", "image_url": {"url": "x"}})),
    ],
)]
#[case::wrapped_literal("\u{0}json:\"\\u0000json:not parts\"", vec![Content::Text("\u{0}json:not parts".to_owned())])]
fn multimodal_content_is_decoded(#[case] raw: &str, #[case] want: Vec<Content>) {
    assert_eq!(decode(raw), want);
}

/// The listener reports what the stores hold, and each change as Hermes commits it: a new
/// session, a row appended, a title given, a profile made after it started.
#[rstest]
#[tokio::test]
async fn the_listener_follows_every_store() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[SCHEMA, COMPACTION]).await;
    let listener = HermesListener {
        root: dir.path().to_path_buf(),
        profile_scan: Duration::from_millis(100),
    };
    // On a task of its own, as capture polls it beside the sessions: a pass of the listener is
    // what wakes a session.
    let (tx, mut watched) = tokio::sync::mpsc::unbounded_channel();
    let listening = tokio::spawn(async move {
        let mut sessions = Box::pin(listener.watch());
        while let Some(session) = sessions.next().await {
            let _ = tx.send(session.unwrap());
        }
    });
    let mut reported = async || {
        tokio::time::timeout(Duration::from_secs(10), watched.recv())
            .await
            .expect("a session is reported")
            .unwrap()
    };
    let mut first = Vec::new();
    for _ in 0..4 {
        first.push(reported().await);
    }
    let mut names: Vec<String> = first.iter().map(|s| s.id().to_string()).collect();
    names.sort();
    assert_eq!(names, [
        "20260511_171303_982a49",
        "20260511_172545_31d799",
        "local-compacted",
        "local-desktop"
    ]);

    let desktop = first.into_iter().find(|s| s.id().as_ref() == "local-desktop").unwrap();
    let mut rows = Box::pin(desktop.messages());
    let mut next = async || {
        tokio::time::timeout(Duration::from_secs(10), rows.next()).await.unwrap().unwrap().unwrap()
    };
    assert_eq!(next().await.id().unwrap().as_ref(), "304");

    let mut writer = connect(&db).await;
    execute(
        &mut writer,
        "INSERT INTO messages (id, session_id, role, content, timestamp)
             VALUES (305, 'local-desktop', 'assistant', 'hello', 1794230004.0);",
    )
    .await;
    let appended = next().await;
    assert_eq!(appended.id().unwrap().as_ref(), "305");
    assert_eq!(appended.parent_id().unwrap().as_ref(), "304");

    execute(
        &mut writer,
        "UPDATE sessions SET title = 'Desk', title_source = 'derived' WHERE id = 'local-desktop'",
    )
    .await;
    assert_eq!(next().await.title(), Some(TitleChange::new(TitleSource::Generated, "Desk")));

    execute(
        &mut writer,
        "INSERT INTO sessions (id, source, created_source, started_at, cwd)
             VALUES ('local-new', 'local', 'tui', 1794260000.0, '/tmp/hermes-play');",
    )
    .await;
    assert_eq!(reported().await.id().as_ref(), "local-new");
    writer.close().await.unwrap();

    store_in(&dir.path().join("profiles").joined("work"), &[SCHEMA, PROBE]).await;
    let profile = reported().await;
    assert_eq!(profile.db(), dir.path().join("profiles").joined("work").joined(DB_FILE));
    listening.abort();
}

/// A session no change touches since the listener started is left to an import.
#[rstest]
#[tokio::test]
async fn an_untouched_session_waits_for_a_change() {
    let dir = tempfile::tempdir().unwrap();
    let db = store_in(dir.path(), &[SCHEMA, COMPACTION]).await;
    let (wake, woken) = watch::channel(0);
    let rows = Store::open(&db).await.unwrap();
    let mut store = rows;
    let row = store.session("local-desktop").await.unwrap().unwrap();
    store.close().await;
    let session = HermesSession::new(Arc::from(db.as_path()), &row, Some(woken));
    let later = SystemTime::now();
    let changed = tokio::spawn(session.changed_after(later));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!changed.is_finished());
    wake.send_modify(|n| *n += 1);
    assert!(changed.await.unwrap().is_some());
}
