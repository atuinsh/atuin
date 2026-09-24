use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use parking_lot::Mutex;
use proptest::prelude::*;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::super::{OpencodeSession, OpencodeSessions};
use super::*;
use crate::db::query;
use crate::db::sqlite::Sqlite;
use crate::harnesstools::session::model::{Content, Role, StopReason, Usage};
use crate::harnesstools::session::{
    CaptureError, Checkpoint, Listener, Message, Session, SessionEvent, SessionId, Sessions,
};

/// opencode 2.0.15's tables as far as this module reads them, from its
/// `packages/core/src/database/schema.gen.ts` at `v2.0.15`: `project` because `session_v2`
/// references it, and the `event` log it keeps and no longer writes.
const SCHEMA: [&str; 8] = [
    "CREATE TABLE `project` (`id` text PRIMARY KEY, `worktree` text NOT NULL, `vcs` text, `name` \
     text, `icon_url` text, `icon_url_override` text, `icon_color` text, `time_created` integer \
     NOT NULL, `time_updated` integer NOT NULL, `time_initialized` integer, `time_active` integer \
     DEFAULT 0 NOT NULL, `sandboxes` text NOT NULL, `commands` text)",
    "CREATE TABLE `event_sequence` (`aggregate_id` text PRIMARY KEY, `seq` integer NOT NULL, \
     `owner_id` text)",
    "CREATE TABLE `event` (`id` text PRIMARY KEY, `aggregate_id` text NOT NULL, `seq` integer NOT \
     NULL, `created` integer DEFAULT 0 NOT NULL, `type` text NOT NULL, `data` text NOT NULL, \
     CONSTRAINT `fk_event_aggregate_id_event_sequence_aggregate_id_fk` FOREIGN KEY \
     (`aggregate_id`) REFERENCES `event_sequence`(`aggregate_id`) ON DELETE CASCADE)",
    "CREATE TABLE `session_message` (`id` text PRIMARY KEY, `session_id` text NOT NULL, `type` \
     text NOT NULL, `seq` integer NOT NULL, `time_created` integer NOT NULL, `time_updated` \
     integer NOT NULL, `data` text NOT NULL, CONSTRAINT \
     `fk_session_message_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES \
     `session_v2`(`id`) ON DELETE CASCADE)",
    "CREATE TABLE `session_v2` (`id` text PRIMARY KEY, `project_id` text NOT NULL, `workspace_id` \
     text, `parent_id` text, `fork_session_id` text, `fork_boundary` text, `slug` text NOT NULL, \
     `directory` text NOT NULL, `path` text, `title` text, `version` text NOT NULL, `share_url` \
     text, `summary_additions` integer, `summary_deletions` integer, `summary_files` integer, \
     `summary_diffs` text, `metadata` text, `cost` real DEFAULT 0 NOT NULL, `tokens_input` \
     integer DEFAULT 0 NOT NULL, `tokens_output` integer DEFAULT 0 NOT NULL, `tokens_reasoning` \
     integer DEFAULT 0 NOT NULL, `tokens_cache_read` integer DEFAULT 0 NOT NULL, \
     `tokens_cache_write` integer DEFAULT 0 NOT NULL, `revert` text, `permission` text, `agent` \
     text, `model` text, `time_created` integer NOT NULL, `time_updated` integer NOT NULL, \
     `time_idle` integer, `time_viewed` integer, `idle_outcome` text, `time_compacting` integer, \
     `time_archived` integer, `time_suspended` integer, `resume_attempts` integer DEFAULT 0 NOT \
     NULL, CONSTRAINT `fk_session_v2_project_id_project_id_fk` FOREIGN KEY (`project_id`) \
     REFERENCES `project`(`id`) ON DELETE CASCADE)",
    "CREATE UNIQUE INDEX `event_aggregate_seq_idx` ON `event` (`aggregate_id`,`seq`)",
    "CREATE UNIQUE INDEX `session_message_session_seq_idx` ON `session_message` \
     (`session_id`,`seq`)",
    "CREATE INDEX `session_message_session_type_seq_idx` ON `session_message` \
     (`session_id`,`type`,`seq`)",
];

/// opencode 1.18.32's `session`, `message` and `part` tables, which opencode 2.0 keeps after it
/// copies them (taken from a database 1.18.32 created).
const LEGACY: [&str; 3] = [
    "CREATE TABLE `session` (`id` text PRIMARY KEY, `project_id` text NOT NULL, `workspace_id` \
     text, `parent_id` text, `slug` text NOT NULL, `directory` text NOT NULL, `path` text, \
     `title` text NOT NULL, `version` text NOT NULL, `share_url` text, `summary_additions` \
     integer, `summary_deletions` integer, `summary_files` integer, `summary_diffs` text, \
     `metadata` text, `cost` real DEFAULT 0 NOT NULL, `tokens_input` integer DEFAULT 0 NOT NULL, \
     `tokens_output` integer DEFAULT 0 NOT NULL, `tokens_reasoning` integer DEFAULT 0 NOT NULL, \
     `tokens_cache_read` integer DEFAULT 0 NOT NULL, `tokens_cache_write` integer DEFAULT 0 NOT \
     NULL, `revert` text, `permission` text, `agent` text, `model` text, `time_created` integer \
     NOT NULL, `time_updated` integer NOT NULL, `time_compacting` integer, `time_archived` \
     integer, CONSTRAINT `fk_session_project_id_project_id_fk` FOREIGN KEY (`project_id`) \
     REFERENCES `project`(`id`) ON DELETE CASCADE)",
    "CREATE TABLE `message` (`id` text PRIMARY KEY, `session_id` text NOT NULL, `time_created` \
     integer NOT NULL, `time_updated` integer NOT NULL, `data` text NOT NULL, CONSTRAINT \
     `fk_message_session_id_session_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session`(`id`) \
     ON DELETE CASCADE)",
    "CREATE TABLE `part` (`id` text PRIMARY KEY, `message_id` text NOT NULL, `session_id` text \
     NOT NULL, `time_created` integer NOT NULL, `time_updated` integer NOT NULL, `data` text NOT \
     NULL, CONSTRAINT `fk_part_message_id_message_id_fk` FOREIGN KEY (`message_id`) REFERENCES \
     `message`(`id`) ON DELETE CASCADE)",
];

/// Sessions opencode 1.18.32 and then 2.0.15 wrote, driven against a local mock provider with an
/// isolated home and no network, paths redacted: `legacy` is the database 1.18.32 left (two
/// sessions: a reasoning prompt and a tool call, and a plain prompt), `final` the same database
/// after 2.0.15 migrated it and then continued the first session (a tool call, a `!` shell
/// command), ran a new one (reasoning, a tool call, a subagent, a provider error, a `length`
/// stop, a failed and a completed compaction) and forked it (and reverted the fork's last turn),
/// and 1.18.32 ran again, continuing the migrated session and starting one more.
const FIXTURE: &str = include_str!("../../../../../tests/fixtures/opencode/v2.json");

/// The fixture's sessions.
const MIGRATED: &str = "ses_f2b669e9bffejWGIuiJJ77cvry";
const NATIVE: &str = "ses_f2b662ccdffe16PlkJxD3iQBym";
const SUBAGENT: &str = "ses_f2b662462ffe5m3IWk5lDCkJD9";
const FORK: &str = "ses_f2b65fb12ffel8KFRA9rRZabhb";

/// Tables in the order their foreign keys need.
const TABLES: [&str; 8] = [
    "project",
    "event_sequence",
    "event",
    "session",
    "message",
    "part",
    "session_v2",
    "session_message",
];

/// An opencode database in a directory of its own, removed with it.
struct Db {
    _dir: TempDir,
    path: PathBuf,
    sqlite: Sqlite,
}

impl Db {
    async fn open(legacy: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let sqlite = Sqlite::builder(path.as_os_str()).open().await.unwrap();
        let db = Self {
            _dir: dir,
            path,
            sqlite,
        };
        let ddl = SCHEMA.iter().chain(if legacy {
            LEGACY.iter()
        } else {
            [].iter()
        });
        for statement in ddl {
            db.execute(statement).await;
        }
        db.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated, sandboxes) VALUES \
             ('global', '/', 0, 0, '[]')",
        )
        .await;
        db
    }

    async fn execute(&self, sql: &'static str) {
        query::<sqlx::Sqlite>(sql).execute(self.sqlite.pool()).await.unwrap();
    }

    /// A 2.0 session, created like `SessionEvent.Created` creates one: its row, and the first
    /// event of its sequence.
    async fn session(&self, id: &str, title: Option<&str>, parent: Option<&str>) {
        self.forked(id, title, parent, None).await;
    }

    async fn forked(
        &self,
        id: &str,
        title: Option<&str>,
        parent: Option<&str>,
        fork: Option<&str>,
    ) {
        let mut tx = self.sqlite.pool().begin().await.unwrap();
        query::<sqlx::Sqlite>(
            "INSERT INTO session_v2 (id, project_id, parent_id, fork_session_id, slug, directory, \
             title, version, time_created, time_updated) VALUES (?1, 'global', ?2, ?3, 'slug', \
             '/work/proj', ?4, '2.0.15', 1000, 1000)",
        )
        .bind(id)
        .bind(parent)
        .bind(fork)
        .bind(title)
        .execute(&mut *tx)
        .await
        .unwrap();
        Self::event(&mut tx, id).await;
        tx.commit().await.unwrap();
    }

    /// The next `seq` of `session`'s event sequence, as `Bus.publish` takes it.
    async fn event(tx: &mut sqlx::SqliteConnection, session: &str) -> i64 {
        query_scalar::<sqlx::Sqlite, i64>(
            "INSERT INTO event_sequence (aggregate_id, seq) VALUES (?1, 0) ON CONFLICT \
             (aggregate_id) DO UPDATE SET seq = seq + 1 RETURNING seq",
        )
        .bind(session)
        .fetch_one(tx)
        .await
        .unwrap()
    }

    /// A durable event of `session` that writes no row.
    async fn bump(&self, session: &str) {
        let mut tx = self.sqlite.pool().begin().await.unwrap();
        Self::event(&mut tx, session).await;
        tx.commit().await.unwrap();
    }

    /// A row a durable event of `session` inserts, at that event's `seq`.
    async fn append(&self, session: &str, id: &str, kind: &str, data: Value) -> i64 {
        let mut tx = self.sqlite.pool().begin().await.unwrap();
        let seq = Self::event(&mut tx, session).await;
        let created = epoch_millis(&data["time"]["created"]).unwrap_or_default();
        query::<sqlx::Sqlite>(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, \
             data) VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
        )
        .bind(id)
        .bind(session)
        .bind(kind)
        .bind(seq)
        .bind(created)
        .bind(data.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        seq
    }

    /// A row a durable event of `session` rewrites in place.
    async fn rewrite(&self, session: &str, id: &str, data: Value) {
        let mut tx = self.sqlite.pool().begin().await.unwrap();
        let seq = Self::event(&mut tx, session).await;
        query::<sqlx::Sqlite>(
            "UPDATE session_message SET data = ?1, time_updated = ?2 WHERE id = ?3",
        )
        .bind(data.to_string())
        .bind(seq)
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    /// `RevertEvent.Committed`: every row of `session` from `seq` on goes.
    async fn revert(&self, session: &str, seq: i64) {
        let mut tx = self.sqlite.pool().begin().await.unwrap();
        Self::event(&mut tx, session).await;
        query::<sqlx::Sqlite>("DELETE FROM session_message WHERE session_id = ?1 AND seq >= ?2")
            .bind(session)
            .bind(seq)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    /// Every row of the fixture's `tables`, in [`TABLES`] order, left as it is when it is there
    /// already.
    async fn load(&self, tables: &Value) {
        for table in TABLES {
            for row in tables[table].as_array().into_iter().flatten() {
                let row = row.as_object().unwrap();
                let columns: Vec<&str> = row.keys().map(String::as_str).collect();
                let slots: Vec<String> = (1..=columns.len()).map(|n| format!("?{n}")).collect();
                let sql = format!(
                    "INSERT OR IGNORE INTO {table} ({}) VALUES ({})",
                    columns.join(", "),
                    slots.join(", ")
                );
                let mut statement = query::<sqlx::Sqlite>(sqlx::AssertSqlSafe(sql));
                for value in row.values() {
                    statement = match value {
                        Value::Null => statement.bind(None::<String>),
                        Value::Number(n) if n.is_i64() => statement.bind(n.as_i64()),
                        Value::Number(n) => statement.bind(n.as_f64()),
                        Value::String(s) => statement.bind(s.clone()),
                        other => statement.bind(other.to_string()),
                    };
                }
                statement.execute(self.sqlite.pool()).await.unwrap();
            }
        }
        for row in tables["event_sequence"].as_array().into_iter().flatten() {
            query::<sqlx::Sqlite>("UPDATE event_sequence SET seq = ?1 WHERE aggregate_id = ?2")
                .bind(row["seq"].as_i64())
                .bind(row["aggregate_id"].as_str())
                .execute(self.sqlite.pool())
                .await
                .unwrap();
        }
    }
}

#[fixture]
async fn db() -> Db {
    Db::open(false).await
}

/// A database opencode 2.0 migrated from 1.x, which keeps 1.x's tables.
#[fixture]
async fn mixed() -> Db {
    Db::open(true).await
}

fn fixture_json() -> Value {
    serde_json::from_str(FIXTURE).unwrap()
}

fn user(text: &str) -> Value {
    json!({"time": {"created": 1_000}, "text": text, "files": []})
}

fn tokens_of(input: u64, output: u64, reasoning: u64) -> Value {
    json!({"input": input, "output": output, "reasoning": reasoning, "cache": {"read": 200, "write": 0}})
}

/// An assistant row as `Step.Started` inserts it, `done` once `Step.Ended` stamped it.
fn assistant(text: &str, done: bool) -> Value {
    let mut data = json!({
        "time": {"created": 2_000},
        "agent": "build",
        "model": {"id": "mock-model", "providerID": "mock"},
        "content": [{"type": "text", "text": text}],
    });
    if done {
        data["time"]["completed"] = json!(2_500);
        data["finish"] = json!("stop");
        data["tokens"] = tokens_of(800, 40, 10);
    }
    data
}

fn idle(outcome: &str) -> Value {
    json!({"time": {"created": 3_000}, "outcome": outcome})
}

type Events = BoxStream<'static, Result<SessionEvent<OpencodeMessage>, CaptureError>>;

/// Checkpoints a consumer stores, by session.
type Stored = Arc<Mutex<HashMap<SessionId, Checkpoint>>>;

fn events(path: &Path, replay: ReplayBehavior, stored: &Stored) -> Events {
    let stored = Arc::clone(stored);
    OpencodeSessions::builder()
        .db(path)
        .replay(replay)
        .build()
        .listener()
        .unwrap()
        .events(move |id| {
            let from = stored.lock().get(id).copied();
            async move { from }
        })
        .boxed()
}

/// The next `n` events, stored as a consumer stores them, failing instead of hanging.
async fn next_n(stream: &mut Events, stored: &Stored, n: usize) -> Vec<(String, OpencodeMessage)> {
    let taken =
        tokio::time::timeout(Duration::from_secs(10), stream.by_ref().take(n).collect::<Vec<_>>())
            .await
            .unwrap_or_else(|_| panic!("the capture did not deliver {n} messages within 10s"));
    taken
        .into_iter()
        .map(|event| {
            let event = event.unwrap();
            stored.lock().insert(event.session.clone(), event.checkpoint);
            (event.session.to_string(), event.message)
        })
        .collect()
}

/// Nothing more arrives for a while: a few of the poll's ticks.
async fn quiet(stream: &mut Events) {
    if let Ok(Some(event)) = tokio::time::timeout(POLL * 3, stream.next()).await {
        panic!("nothing more was due, got {:?}", event.map(|event| event.message.id()));
    }
}

/// `<role> <id>: <content>` of a message, the way the assertions read it.
fn line(message: &OpencodeMessage) -> String {
    let content = message
        .content()
        .into_iter()
        .map(|content| match content {
            Content::Text(text) => text,
            Content::Summary(text) => format!("summary {text}"),
            Content::Error(text) => format!("error {text}"),
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let id = message.id().map(String::from).unwrap_or_default();
    format!("{:?} {id}: {content}", message.role())
}

fn lines(messages: &[(String, OpencodeMessage)]) -> Vec<String> {
    messages.iter().map(|(_, message)| line(message)).collect()
}

/// Every session `existing()` lists, read whole.
async fn read_all(path: &Path) -> Vec<(String, OpencodeMessage)> {
    let sessions: Vec<OpencodeSession> = OpencodeSessions::builder()
        .db(path)
        .build()
        .existing()
        .unwrap()
        .map(Result::unwrap)
        .collect()
        .await;
    let mut out = Vec::new();
    for session in sessions {
        let id = session.id().to_string();
        let messages: Vec<_> = session.read().collect().await;
        out.extend(messages.into_iter().map(|m| (id.clone(), m.unwrap())));
    }
    out
}

fn full(kind: &str, data: &Value) -> Full {
    Full {
        seq: 5,
        id: "msg_1".to_owned(),
        kind: kind.to_owned(),
        created: Some(1),
        data: Some(data.to_string()),
        migrated: false,
    }
}

fn state() -> State {
    State {
        info: Some(SessionInfo {
            id: "ses_1".to_owned(),
            title: None,
            parent: None,
            fork: None,
            directory: Some("/work/proj".to_owned()),
            created: Some(1),
            updated: Some(1),
        }),
        ..State::default()
    }
}

fn normalised(kind: &str, data: &Value) -> Option<OpencodeMessage> {
    let state = state();
    full(kind, data).message(state.info.as_ref().unwrap(), &state).map(Result::unwrap)
}

#[rstest]
#[case::a_prompt("user", json!({"text": "hi"}), Role::User, vec![Content::Text("hi".into())])]
#[case::a_subagent_s_prompt(
    "user",
    json!({"text": "You are a subagent spawned by another session.\nsay hi"}),
    Role::System,
    vec![Content::Text("You are a subagent spawned by another session.\nsay hi".into())],
)]
#[case::synthetic("synthetic", json!({"text": "t"}), Role::System, vec![Content::Text("t".into())])]
#[case::system("system", json!({"text": "t"}), Role::System, vec![Content::Text("t".into())])]
#[case::skill("skill", json!({"text": "t", "name": "s"}), Role::System, vec![Content::Text("t".into())])]
#[case::a_shell_command(
    "shell",
    json!({"command": "ls", "status": "exited", "output": {"output": "files"}}),
    Role::User,
    vec![Content::Text("! ls".into())],
)]
#[case::a_compaction(
    "compaction",
    json!({"status": "completed", "summary": "## Objective", "tokens": tokens_of(1, 2, 0)}),
    Role::Assistant,
    vec![Content::Summary("## Objective".into())],
)]
#[case::a_failed_compaction(
    "compaction",
    json!({"status": "failed", "error": {"type": "compaction.failed", "message": "no"}}),
    Role::Assistant,
    vec![Content::Error("no".into())],
)]
#[case::a_row_of_a_type_this_module_does_not_know(
    "future",
    json!({"x": 1}),
    Role::Other("future".into()),
    vec![Content::Other(json!({"x": 1, "id": "msg_1"}))],
)]
fn a_row_says_what_its_type_says(
    #[case] kind: &str,
    #[case] data: Value,
    #[case] role: Role,
    #[case] content: Vec<Content>,
) {
    let message = normalised(kind, &data).expect("a message");
    assert_eq!(message.role(), role);
    assert_eq!(message.content(), content);
    assert_eq!(message.id().map(String::from).as_deref(), Some("msg_1"));
}

#[rstest]
#[case::an_idle_session("idle", json!({"outcome": "succeeded"}))]
#[case::an_agent_switch("agent-switched", json!({"agent": "plan"}))]
#[case::a_model_switch("model-switched", json!({"model": {"id": "m"}}))]
#[case::a_move("location-switched", json!({"location": {"directory": "/x"}}))]
fn a_row_with_nothing_to_say_is_no_message(#[case] kind: &str, #[case] data: Value) {
    assert!(normalised(kind, &data).is_none());
}

#[rstest]
#[case::failed("failed", StopReason::Error)]
#[case::interrupted("interrupted", StopReason::Aborted)]
fn a_turn_that_did_not_succeed_says_why(#[case] outcome: &str, #[case] stop: StopReason) {
    let message = normalised("idle", &json!({"outcome": outcome})).unwrap();
    assert_eq!(message.stop_reason(), Some(stop));
    assert_eq!(message.role(), Role::System);
}

#[rstest]
#[case::stop(json!({"finish": "stop"}), Some(StopReason::EndTurn), None)]
#[case::length(json!({"finish": "length"}), Some(StopReason::MaxTokens), None)]
#[case::tool_calls(json!({"finish": "tool-calls"}), Some(StopReason::ToolUse), None)]
#[case::unknown(json!({"finish": "unknown"}), Some(StopReason::Other("unknown".into())), None)]
#[case::a_provider_error(
    json!({"finish": "error", "error": {"type": "provider.invalid-request", "message": "bad"}}),
    Some(StopReason::Error),
    Some("bad"),
)]
#[case::an_interrupted_step(
    json!({"finish": "error", "error": {"type": "aborted", "message": "Step interrupted"}}),
    Some(StopReason::Aborted),
    Some("Step interrupted"),
)]
#[case::a_refusal(
    json!({"finish": "content-filter", "error": {"type": "provider.content-filter", "message": "blocked"}}),
    Some(StopReason::Refusal),
    Some("blocked"),
)]
#[case::a_step_a_newer_one_superseded(json!({}), None, None)]
fn a_call_says_how_it_ended(
    #[case] end: Value,
    #[case] stop: Option<StopReason>,
    #[case] error: Option<&str>,
) {
    let mut data = assistant("hi", false);
    data["time"]["completed"] = json!(2_500);
    for (key, value) in end.as_object().unwrap() {
        data[key] = value.clone();
    }
    let message = normalised("assistant", &data).unwrap();
    assert_eq!(message.stop_reason(), stop);
    let errors: Vec<Content> =
        message.content().into_iter().filter(|c| matches!(c, Content::Error(_))).collect();
    assert_eq!(errors, error.map(|e| Content::Error(e.to_owned())).into_iter().collect::<Vec<_>>());
}

#[rstest]
fn a_call_s_content_is_its_text_reasoning_and_tools() {
    let data = json!({
        "time": {"created": 2_000, "completed": 2_500},
        "model": {"id": "mock-model", "providerID": "mock"},
        "content": [
            {"type": "reasoning", "text": "hmm"},
            {"type": "text", "text": "Running it."},
            {"type": "tool", "id": "call_1", "name": "shell", "state": {
                "status": "completed", "input": {"command": "ls"},
                "content": [{"type": "text", "text": "files"}]}},
            {"type": "tool", "id": "call_2", "name": "read", "state": {
                "status": "error", "input": {}, "error": {"type": "tool.execution", "message": "no"}}},
        ],
        "finish": "tool-calls",
    });
    let message = normalised("assistant", &data).unwrap();
    assert_eq!(message.role(), Role::Assistant);
    assert_eq!(message.model().as_deref(), Some("mock-model"));
    assert_eq!(message.cwd(), Some(PathBuf::from("/work/proj")));
    assert_eq!(message.timestamp().unwrap().unix_timestamp_nanos(), 2_000_000_000);
    let content = message.content();
    assert!(matches!(
        content.as_slice(),
        [
            Content::ReasoningSummary { tokens: None },
            Content::Text(text),
            Content::ToolUse(call),
            Content::ToolResult(result),
            Content::ToolUse(failed),
            Content::ToolResult(error),
        ] if text == "Running it."
            && call.name == "shell" && call.input == json!({"command": "ls"})
            && result.call.as_ref() == "call_1" && !result.error
            && result.output == json!([{"type": "text", "text": "files"}])
            && failed.name == "read" && error.error && error.output == json!("no")
    ));
}

/// opencode 2.0's `input` leaves out the cached tokens and its `output` the reasoning ones
/// (`SessionUsage.tokens`: `nonCachedInputTokens`, `visibleOutputTokens`); reasoning is billed as
/// output (`calculateCost`), so it is added back.
#[rstest]
#[case::assistant("assistant")]
#[case::compaction("compaction")]
fn a_call_s_usage_counts_its_reasoning_as_output(#[case] kind: &str) {
    let mut data = assistant("hi", true);
    data["status"] = json!("completed");
    data["tokens"] =
        json!({"input": 802, "output": 42, "reasoning": 10, "cache": {"read": 200, "write": 3}});
    let message = normalised(kind, &data).unwrap();
    assert_eq!(
        message.usage(),
        Some(Usage {
            input: Some(802),
            output: Some(52),
            cache_read: Some(200),
            cache_write: Some(3),
            reasoning: Some(10),
        })
    );
    assert_eq!(message.turn_id().as_deref(), Some("2000:mock/mock-model#802/42/10/200/3"));
}

/// A fork copies a row under a new id with its `data` as it was: the copy is the same call.
#[rstest]
fn a_fork_s_copy_of_a_call_is_the_same_call() {
    let state = state();
    let info = state.info.as_ref().unwrap();
    let data = assistant("hi", true);
    let original = full("assistant", &data).message(info, &state).unwrap().unwrap();
    let mut copy = full("assistant", &data);
    copy.id = "msg_evt_5".to_owned();
    let copy = copy.message(info, &state).unwrap().unwrap();
    assert_ne!(original.id(), copy.id());
    assert_eq!(original.turn_id(), copy.turn_id());
    assert_eq!(original.usage(), copy.usage());
}

#[rstest]
#[case::before_any_move(1, "/a")]
#[case::after_the_first(3, "/b")]
#[case::after_the_second(9, "/c")]
fn a_row_was_written_where_the_session_was_then(#[case] seq: i64, #[case] cwd: &str) {
    let mut state = state();
    state.moves = vec![
        Moved {
            seq: 2,
            to: Some("/b".to_owned()),
            from: Some("/a".to_owned()),
        },
        Moved {
            seq: 5,
            to: Some("/c".to_owned()),
            from: Some("/b".to_owned()),
        },
    ];
    assert_eq!(state.cwd_at(seq).as_deref(), Some(cwd));
}

#[rstest]
fn a_session_s_info_names_its_title_and_where_it_came_from() {
    let mut info = state().info.unwrap();
    let untitled = info.message();
    assert_eq!(untitled.id().map(String::from).as_deref(), Some("ses_1:session"));
    assert_eq!(untitled.title(), None);
    assert_eq!(untitled.parent_session(), None);
    info.title = Some("Fix it".to_owned());
    info.fork = Some("ses_0".to_owned());
    let titled = info.message();
    assert_eq!(titled.id().map(String::from).as_deref(), Some("ses_1:title:Fix it"));
    assert_eq!(titled.title().and_then(|title| title.text).as_deref(), Some("Fix it"));
    assert_eq!(titled.parent_session().map(String::from).as_deref(), Some("ses_0"));
    assert_eq!(titled.cwd(), Some(PathBuf::from("/work/proj")));
}

#[rstest]
fn a_1x_checkpoint_keeps_its_old_shape() {
    assert_eq!(checkpoint(Some((7, "evt_7")), None), Checkpoint::new(7, b"evt_7"));
    assert_eq!(unpack(Checkpoint::new(7, b"evt_7")).1, None);
    assert!(unpack(Checkpoint::new(7, b"evt_7")).0.unwrap().names("evt_7"));
}

proptest! {
    /// Both places come back out of the checkpoint they were packed into.
    #[test]
    fn a_checkpoint_names_both_places(
        legacy in proptest::option::of((0i64..1 << 31, "[a-z0-9_]{1,12}")),
        next in proptest::option::of((0i64..1 << 30, "[a-z0-9_]{1,12}")),
    ) {
        let mark = next.as_ref().map(|(seq, id)| Mark::of(*seq, id));
        let at = checkpoint(legacy.as_ref().map(|(seq, id)| (*seq, id.as_str())), mark);
        let (got_legacy, got_next) = unpack(at);
        prop_assert_eq!(got_legacy.map(|mark| mark.seq), legacy.as_ref().map(|(seq, _)| *seq));
        prop_assert_eq!(got_next.map(|mark| mark.seq), next.as_ref().map(|(seq, _)| *seq));
        if let (Some(got), Some((_, id))) = (got_legacy, &legacy) {
            prop_assert!(got.names(id));
        }
        if let (Some(got), Some((_, id))) = (got_next, &next) {
            prop_assert!(got.names(id));
        }
    }

    /// A checkpoint grows as either place does, so that a resumed read's first message is past
    /// the checkpoint it resumed from (`Start::of` in the daemon).
    #[test]
    fn a_checkpoint_grows_with_either_place(
        legacy in 0i64..1 << 30, next in 0i64..1 << 30, step in 1i64..1000, which in any::<bool>(),
    ) {
        let before = checkpoint(Some((legacy, "a")), Some(Mark::of(next, "b")));
        let (legacy, next) = if which { (legacy + step, next) } else { (legacy, next + step) };
        let after = checkpoint(Some((legacy, "a")), Some(Mark::of(next, "b")));
        prop_assert!(after.at > before.at);
    }
}

/// A call's row is rewritten in place as it streams; only the row it settles as is delivered,
/// once, with what it ended with.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_is_delivered_once_it_settles(#[future] db: Db) {
    let db = db.await;
    let stored = Stored::default();
    db.session("ses_1", Some("A title"), None).await;
    db.append("ses_1", "msg_u", "user", user("hello")).await;
    db.append("ses_1", "msg_a", "assistant", assistant("", false)).await;

    let mut stream = events(&db.path, ReplayBehavior::All, &stored);
    assert_eq!(lines(&next_n(&mut stream, &stored, 2).await), [
        "System ses_1:title:A title: ",
        "User msg_u: hello",
    ]);
    db.rewrite("ses_1", "msg_a", assistant("partial", false)).await;
    db.rewrite("ses_1", "msg_a", assistant("partial answer", false)).await;
    quiet(&mut stream).await;

    db.rewrite("ses_1", "msg_a", assistant("the whole answer", true)).await;
    let settled = next_n(&mut stream, &stored, 1).await;
    assert_eq!(lines(&settled), ["Assistant msg_a: the whole answer"]);
    assert_eq!(settled[0].1.usage().and_then(|usage| usage.output), Some(50));
    // a durable event that writes no row, and the idle row that ends the turn
    db.bump("ses_1").await;
    db.append("ses_1", "msg_i", "idle", idle("succeeded")).await;
    quiet(&mut stream).await;
}

/// A prompt steered in while a call runs settles before it: it is delivered at once, and the
/// checkpoint stays where the call is until the call settles, so that a restart in between reads
/// the call.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rows_that_settle_out_of_order_are_all_delivered_across_a_restart(#[future] db: Db) {
    let db = db.await;
    let stored = Stored::default();
    db.session("ses_1", None, None).await;
    let first = db.append("ses_1", "msg_u1", "user", user("one")).await;
    db.append("ses_1", "msg_a", "assistant", assistant("", false)).await;
    db.append("ses_1", "msg_u2", "user", user("steered")).await;

    let mut stream = events(&db.path, ReplayBehavior::All, &stored);
    assert_eq!(lines(&next_n(&mut stream, &stored, 3).await), [
        "System ses_1:session: ",
        "User msg_u1: one",
        "User msg_u2: steered",
    ]);
    let held = stored.lock()[&SessionId::from("ses_1".to_owned())];
    assert_eq!(unpack(held).1.map(|mark| mark.seq), Some(first));
    drop(stream);

    // the capture restarts from what it stored: nothing before the call again, the prompt
    // after it again (a duplicate the consumer drops), and the call once it settles
    let mut stream = events(&db.path, ReplayBehavior::All, &stored);
    assert_eq!(lines(&next_n(&mut stream, &stored, 2).await), [
        "System ses_1:session: ",
        "User msg_u2: steered",
    ]);
    db.rewrite("ses_1", "msg_a", assistant("answer", true)).await;
    assert_eq!(lines(&next_n(&mut stream, &stored, 1).await), ["Assistant msg_a: answer"]);
    // the call's checkpoint is past the prompt delivered before it
    let at = stored.lock()[&SessionId::from("ses_1".to_owned())];
    assert!(unpack(at).1.unwrap().names("msg_u2"));
    drop(stream);

    let mut stream = events(&db.path, ReplayBehavior::All, &stored);
    assert_eq!(lines(&next_n(&mut stream, &stored, 1).await), ["System ses_1:session: "]);
    quiet(&mut stream).await;
}

/// A revert deletes the rows from its boundary on, the one the checkpoint names among them; the
/// sequence goes on, so the session goes on past it rather than reading everything again.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revert_of_the_checkpoint_s_row_goes_on_past_it(#[future] db: Db) {
    let db = db.await;
    let stored = Stored::default();
    db.session("ses_1", None, None).await;
    db.append("ses_1", "msg_u1", "user", user("one")).await;
    let boundary = db.append("ses_1", "msg_u2", "user", user("two")).await;
    db.append("ses_1", "msg_a2", "assistant", assistant("answer two", true)).await;

    let mut stream = events(&db.path, ReplayBehavior::All, &stored);
    assert_eq!(next_n(&mut stream, &stored, 4).await.len(), 4);
    drop(stream);

    db.revert("ses_1", boundary).await;
    db.append("ses_1", "msg_u3", "user", user("three")).await;
    let mut stream = events(&db.path, ReplayBehavior::All, &stored);
    assert_eq!(lines(&next_n(&mut stream, &stored, 2).await), [
        "System ses_1:session: ",
        "User msg_u3: three",
    ]);
    // and live, a revert of the row just delivered
    db.revert("ses_1", boundary).await;
    db.append("ses_1", "msg_u4", "user", user("four")).await;
    assert_eq!(lines(&next_n(&mut stream, &stored, 1).await), ["User msg_u4: four"]);
    quiet(&mut stream).await;
}

/// A sequence that went back below the checkpoint is a restored backup: the session is read
/// again from the start (the consumer drops what it has).
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restored_backup_is_read_again_from_the_start(#[future] db: Db) {
    let db = db.await;
    let stored = Stored::default();
    db.session("ses_1", None, None).await;
    db.append("ses_1", "msg_u1", "user", user("one")).await;
    db.append("ses_1", "msg_u2", "user", user("two")).await;
    let mut stream = events(&db.path, ReplayBehavior::All, &stored);
    assert_eq!(next_n(&mut stream, &stored, 3).await.len(), 3);
    drop(stream);

    db.execute("DELETE FROM session_message WHERE id = 'msg_u2'").await;
    db.execute("UPDATE event_sequence SET seq = 1").await;
    let mut stream = events(&db.path, ReplayBehavior::All, &stored);
    assert_eq!(lines(&next_n(&mut stream, &stored, 2).await), [
        "System ses_1:session: ",
        "User msg_u1: one",
    ]);
}

/// A fork's copies are messages of the fork, whose calls are the parent's.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fork_s_copies_count_no_call_twice(#[future] db: Db) {
    let db = db.await;
    db.session("ses_p", Some("Parent"), None).await;
    db.append("ses_p", "msg_u", "user", user("one")).await;
    let seq = db.append("ses_p", "msg_a", "assistant", assistant("answer", true)).await;
    db.forked("ses_f", Some("Parent (fork #1)"), None, Some("ses_p")).await;
    db.execute(
        "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, \
         data) SELECT 'msg_evt_' || seq, 'ses_f', type, seq, time_created, time_updated, data \
         FROM session_message WHERE session_id = 'ses_p'",
    )
    .await;
    db.execute("UPDATE event_sequence SET seq = 3 WHERE aggregate_id = 'ses_f'").await;
    db.append("ses_f", "msg_u2", "user", user("after the fork")).await;

    let read = read_all(&db.path).await;
    let call = |session: &str| {
        read.iter().find(|(of, m)| of == session && m.usage().is_some()).map(|(_, m)| m).unwrap()
    };
    let (original, copy) = (call("ses_p"), call("ses_f"));
    assert_eq!(original.turn_id(), copy.turn_id());
    assert_eq!(original.usage(), copy.usage());
    assert_eq!(copy.id().map(String::from), Some(format!("msg_evt_{seq}")));
    let fork: Vec<_> = read.iter().filter(|(session, _)| session == "ses_f").collect();
    assert_eq!(fork[0].1.parent_session().map(String::from).as_deref(), Some("ses_p"));
    assert_eq!(lines(&fork.into_iter().cloned().collect::<Vec<_>>()), [
        "System ses_f:title:Parent (fork #1): ",
        "User msg_evt_1: one",
        "Assistant msg_evt_2: answer",
        "User msg_u2: after the fork",
    ]);
}

/// A tail started from now delivers what 2.0 writes after it started, and nothing from before.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replay_from_now_skips_preexisting_rows(#[future] db: Db) {
    let db = db.await;
    let stored = Stored::default();
    db.session("ses_1", None, None).await;
    db.append("ses_1", "msg_u1", "user", user("old")).await;
    let mut stream = events(&db.path, ReplayBehavior::FromNow, &stored);
    // the tail seeds on its first poll, which the stream runs once it is polled
    quiet(&mut stream).await;
    db.append("ses_1", "msg_u2", "user", user("new")).await;
    db.session("ses_2", None, None).await;
    db.append("ses_2", "msg_x", "user", user("brand new")).await;
    let mut got = lines(&next_n(&mut stream, &stored, 4).await);
    got.sort();
    assert_eq!(got, [
        "System ses_1:session: ",
        "System ses_2:session: ",
        "User msg_u2: new",
        "User msg_x: brand new",
    ]);
}

/// A session opencode 2.0 renames is delivered its new title.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rename_is_delivered(#[future] db: Db) {
    let db = db.await;
    let stored = Stored::default();
    db.session("ses_1", None, None).await;
    let mut stream = events(&db.path, ReplayBehavior::All, &stored);
    assert_eq!(lines(&next_n(&mut stream, &stored, 1).await), ["System ses_1:session: "]);
    // opencode bumps `time_updated` on every prompt, which says nothing new of the session
    db.execute("UPDATE session_v2 SET time_updated = 2000").await;
    db.bump("ses_1").await;
    quiet(&mut stream).await;
    db.execute("UPDATE session_v2 SET title = 'Named'").await;
    db.bump("ses_1").await;
    let renamed = next_n(&mut stream, &stored, 1).await;
    assert_eq!(renamed[0].1.title().and_then(|title| title.text).as_deref(), Some("Named"));
}

/// A database only opencode 2.0 ever wrote: no 1.x tables at all.
#[rstest]
#[tokio::test]
async fn a_2_0_only_database_is_read_whole(#[future] db: Db) {
    let db = db.await;
    db.session("ses_1", Some("T"), None).await;
    db.append("ses_1", "msg_u", "user", user("hi")).await;
    db.append("ses_1", "msg_a", "assistant", assistant("", false)).await;
    // an import delivers the settled rows, and leaves the call still running to capture
    assert_eq!(lines(&read_all(&db.path).await), ["System ses_1:title:T: ", "User msg_u: hi"]);
}

/// The whole live path against a database being written: two sessions, each prompting and
/// streaming calls that are rewritten many times over, one steering a prompt in mid-call. Every
/// row is delivered exactly once, as it settled.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_database_being_written_is_captured_row_for_row(#[future] db: Db) {
    let db = Arc::new(db.await);
    let stored = Stored::default();
    let mut stream = events(&db.path, ReplayBehavior::All, &stored);
    let writer = {
        let db = Arc::clone(&db);
        tokio::spawn(async move {
            for session in ["ses_a", "ses_b"] {
                db.session(session, None, None).await;
            }
            for turn in 0..3 {
                for session in ["ses_a", "ses_b"] {
                    db.append(
                        session,
                        &format!("{session}_u{turn}"),
                        "user",
                        user(&format!("q{turn}")),
                    )
                    .await;
                    let call = format!("{session}_a{turn}");
                    db.append(session, &call, "assistant", assistant("", false)).await;
                    let mut text = String::new();
                    for chunk in 0..5 {
                        text.push_str(&format!("w{chunk} "));
                        db.rewrite(session, &call, assistant(&text, false)).await;
                        if turn == 1 && chunk == 2 && session == "ses_a" {
                            db.append(session, "ses_a_steer", "user", user("steer")).await;
                        }
                        tokio::time::sleep(Duration::from_millis(15)).await;
                    }
                    db.rewrite(session, &call, assistant(&format!("{text}done"), true)).await;
                    db.append(session, &format!("{session}_i{turn}"), "idle", idle("succeeded"))
                        .await;
                }
            }
        })
    };
    // two infos, and per session three prompts and three calls, and the steered prompt
    let got = next_n(&mut stream, &stored, 2 + 2 * 6 + 1).await;
    writer.await.unwrap();
    quiet(&mut stream).await;
    let ids: Vec<String> = got.iter().filter_map(|(_, m)| m.id()).map(String::from).collect();
    assert_eq!(
        ids.iter().collect::<HashSet<_>>().len(),
        ids.len(),
        "a row was delivered twice: {ids:?}"
    );
    for (_, message) in &got {
        if message.role() == Role::Assistant {
            assert_eq!(
                message.content(),
                vec![Content::Text("w0 w1 w2 w3 w4 done".to_owned())],
                "a call was delivered before it settled"
            );
        }
    }
    assert!(ids.contains(&"ses_a_steer".to_owned()));
}

/// The redacted fixture, as a 2.0 database it ends up as: every session read whole.
#[rstest]
#[tokio::test]
async fn the_fixture_s_sessions_read_as_opencode_wrote_them(#[future] mixed: Db) {
    let db = mixed.await;
    db.load(&fixture_json()["final"]).await;
    let read = read_all(&db.path).await;
    let of = |session: &str| -> Vec<&OpencodeMessage> {
        read.iter().filter(|(id, _)| id == session).map(|(_, m)| m).collect()
    };

    let ids: Vec<(String, String)> = read
        .iter()
        .filter_map(|(session, m)| Some((session.clone(), String::from(m.id()?))))
        .collect();
    // a title both layouts name is the one message both deliver, as each session info does
    let twice: Vec<&(String, String)> = ids
        .iter()
        .filter(|id| ids.iter().filter(|other| other == id).count() > 1)
        .filter(|(_, id)| !id.contains(":title:"))
        .collect();
    assert!(twice.is_empty(), "read twice: {twice:?}");

    // a native session: reasoning, a tool call, a subagent, a provider error, a length stop, a
    // failed compaction and a completed one
    let native = of(NATIVE);
    let described: Vec<String> = native.iter().map(|m| line(m)).collect();
    assert_eq!(described[0], "System ses_f2b662ccdffe16PlkJxD3iQBym:title:Mock title number 1: ");
    assert!(
        described
            .contains(&"User msg_0d499d376001JE6C7LQelc1wB5: \"hello THINK native\"".to_owned())
    );
    assert!(
        described.contains(
            &"Assistant msg_0d499d3f2001UoFywOqgxK1QvO: ReasoningSummary { tokens: None } | \
              reply2 to the user."
                .to_owned()
        )
    );
    let failed = native
        .iter()
        .find(|m| m.id().is_some_and(|id| id.as_ref() == "msg_0d499df74001BC57VsQoup8v9V"))
        .unwrap();
    assert_eq!(failed.stop_reason(), Some(StopReason::Error));
    assert_eq!(failed.content(), vec![Content::Error("mock bad request".into())]);
    assert!(native.iter().any(|m| m.stop_reason() == Some(StopReason::MaxTokens)));
    assert!(native.iter().any(|m| {
        m.content()
            .iter()
            .any(|c| matches!(c, Content::Summary(s) if s.starts_with("## Objective")))
    }));
    let subagent_call = native
        .iter()
        .find(|m| {
            m.content()
                .iter()
                .any(|c| matches!(c, Content::ToolUse(call) if call.name == "subagent"))
        })
        .unwrap();
    assert_eq!(subagent_call.stop_reason(), Some(StopReason::ToolUse));
    let usage = subagent_call.usage().unwrap();
    assert_eq!((usage.input, usage.output, usage.reasoning), (Some(805), Some(55), Some(10)));
    assert!(
        native
            .iter()
            .any(|m| m.role() == Role::System && m.stop_reason() == Some(StopReason::Error))
    );

    // the subagent's session: its parent, and the prompt opencode wrote for it
    let subagent = of(SUBAGENT);
    assert_eq!(subagent[0].parent_session().map(String::from).as_deref(), Some(NATIVE));
    assert_eq!(subagent[1].role(), Role::System);

    // the fork names what it copied, and its copied calls are the native session's
    let fork = of(FORK);
    assert_eq!(fork[0].parent_session().map(String::from).as_deref(), Some(NATIVE));
    let turns = |messages: &[&OpencodeMessage]| -> HashSet<String> {
        messages.iter().filter(|m| m.usage().is_some()).filter_map(|m| m.turn_id()).collect()
    };
    let (fork_turns, native_turns) = (turns(&fork), turns(&native));
    assert!(fork_turns.len() >= 6);
    assert!(fork_turns.is_subset(&native_turns), "the fork's copies count calls again");
    // the fork's reverted turn is gone, the system row written for it stays
    assert!(
        fork.iter()
            .all(|m| m.id().is_none_or(|id| id.as_ref() != "msg_0d49a059d001Ir0F4R0w554SPX"))
    );

    // the migrated session: its 1.x messages read from 1.x's tables, what 2.0 wrote from its
    // own, and none of the copies 2.0 made of the 1.x messages
    let migrated = of(MIGRATED);
    let legacy_messages: HashSet<String> = fixture_json()["final"]["message"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect();
    assert!(migrated.iter().all(|m| {
        m.id().is_none_or(|id| {
            !legacy_messages.contains(id.as_ref())
                || m.content().iter().all(|c| matches!(c, Content::Error(_)))
        })
    }));
    let described: Vec<String> = migrated.iter().map(|m| line(m)).collect();
    for expected in [
        "User msg_0d499b0ae001OhH90RwBX8LSo5: \"continue after upgrade TOOL please\"",
        "User msg_0d49d483e001yXP8yr1CNUG6Zn: ! echo shell-hi",
    ] {
        assert!(described.contains(&expected.to_owned()), "{expected} missing from {described:#?}");
    }
    // 1.18.32 continued it after the migration
    assert!(
        described
            .iter()
            .any(|line| line.ends_with("\"one eighteen continues a migrated session\""))
    );
    assert!(described.iter().any(|line| line.ends_with("\"hello THINK there\"")));
}

/// A 1.x database captured live, then migrated by opencode 2.0 and written by both: nothing is
/// delivered twice, and nothing 2.0 copied is delivered again.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_migration_from_1x_delivers_nothing_twice(#[future] mixed: Db) {
    let db = mixed.await;
    let stored = Stored::default();
    let fixture = fixture_json();
    db.load(&fixture["legacy"]).await;
    let mut stream = events(&db.path, ReplayBehavior::All, &stored);
    let mut got = Vec::new();
    // 1.18.32's two sessions: their titles, and prompts, reasoning, a tool call, texts, steps
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(2), stream.next()).await {
        got.push(event.unwrap().message);
    }
    assert!(got.len() > 10);

    // opencode 2.0 migrates: the event log goes, the copies come, and both write on
    db.execute("DELETE FROM event").await;
    db.load(&fixture["final"]).await;
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(3), stream.next()).await {
        got.push(event.unwrap().message);
    }
    let ids: Vec<String> = got.iter().filter_map(Message::id).map(String::from).collect();
    let repeated: Vec<&String> =
        ids.iter().filter(|id| ids.iter().filter(|other| other == id).count() > 1).collect();
    // a title repeats when a later session info names it again; nothing else does
    assert!(repeated.iter().all(|id| id.contains(":title:")), "delivered twice: {repeated:?}");
    let copies: HashSet<&str> = fixture["final"]["session_message"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| {
            fixture["legacy"]["message"].as_array().unwrap().iter().any(|m| m["id"] == row["id"])
        })
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(copies.len(), 7);
    assert!(got.iter().all(|m| m.id().is_none_or(|id| !copies.contains(id.as_ref())
        || m.content().iter().all(|c| matches!(c, Content::Error(_))))));
    // what 2.0 wrote is there, the subagent's and the fork's sessions among it
    for id in [
        "msg_0d499b0ae001OhH90RwBX8LSo5",
        "msg_0d499dbb9001m36u1ujOgffABQ",
        "msg_0d49a04ee001ZdRtMZSRXWjxt0_4",
    ] {
        assert!(ids.contains(&id.to_owned()), "{id} missing");
    }
}

/// One session in both layouts, live: 1.x's event rows and 2.0's rows are one stream, each
/// delivered once, and a checkpoint holding both places resumes both.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_in_both_layouts_is_one_session(#[future] mixed: Db) {
    let db = mixed.await;
    let stored = Stored::default();
    let text_event = |part: &str, text: &str| {
        json!({"part": {"id": part, "messageID": "msg_old", "sessionID": "ses_m", "type": "text", "text": text}, "time": 1_000})
            .to_string()
    };
    let legacy_event = |id: &'static str, kind: &'static str, data: String| {
        let db = &db;
        async move {
            let mut tx = db.sqlite.pool().begin().await.unwrap();
            let seq = Db::event(&mut tx, "ses_m").await;
            query::<sqlx::Sqlite>(
                "INSERT INTO event (id, aggregate_id, seq, type, data) VALUES (?1, 'ses_m', ?2, \
                 ?3, ?4)",
            )
            .bind(id)
            .bind(seq)
            .bind(kind)
            .bind(data)
            .execute(&mut *tx)
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }
    };
    db.execute(
        "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, \
         time_updated) VALUES ('ses_m', 'global', 's', '/work/proj', 'Old', '1.18.32', 0, 0)",
    )
    .await;
    db.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES \
         ('msg_old', 'ses_m', 0, 0, '{\"role\":\"user\",\"time\":{\"created\":0}}')",
    )
    .await;
    legacy_event(
        "evt_1",
        "message.updated.1",
        json!({"info": {"id": "msg_old", "role": "user"}}).to_string(),
    )
    .await;
    legacy_event("evt_2", "message.part.updated.1", text_event("prt_1", "from one point x")).await;
    // 2.0 migrates the session: its copy of the 1.x message, then rows of its own
    db.session("ses_m", Some("Old"), None).await;
    db.append("ses_m", "msg_old", "user", user("from one point x")).await;
    db.append("ses_m", "msg_new", "user", user("from two point oh")).await;

    let mut stream = events(&db.path, ReplayBehavior::All, &stored);
    let mut got = lines(&next_n(&mut stream, &stored, 3).await);
    got.sort();
    assert_eq!(got, [
        "System ses_m:title:Old: ",
        "User msg_new: from two point oh",
        "User prt_1: from one point x",
    ]);
    quiet(&mut stream).await;
    drop(stream);

    // both write on while nothing captures, then capture resumes from what it stored
    legacy_event("evt_3", "message.part.updated.1", text_event("prt_2", "one point x again")).await;
    db.append("ses_m", "msg_newer", "user", user("two point oh again")).await;
    let at = stored.lock()[&SessionId::from("ses_m".to_owned())];
    assert_ne!(at.at & BOTH, 0, "the checkpoint holds both places");
    let mut stream = events(&db.path, ReplayBehavior::All, &stored);
    let mut got = lines(&next_n(&mut stream, &stored, 3).await);
    got.sort();
    assert_eq!(got, [
        "System ses_m:title:Old: ",
        "User msg_newer: two point oh again",
        "User prt_2: one point x again",
    ]);
    quiet(&mut stream).await;
}
