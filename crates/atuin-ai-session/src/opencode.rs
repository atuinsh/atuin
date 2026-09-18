//! OpenCode: SQLite at `~/.local/share/opencode/opencode.db`, tables `session`, `message` and
//! `part`, each with a JSON `data` column. Subagent sessions carry `parent_id`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use atuin_common::db::query;
use eyre::Result;
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};
use uuid::Uuid;

use crate::ingest::{Stats, whole};
use crate::store::Store;
use crate::{
    Agent, FromNative, Message, Role, Session, StopReason, ToNative, Tokens, ToolCall, cap_output,
    ts_millis, turns, working_dir,
};

pub struct OpenCode;

impl FromNative for OpenCode {
    const AGENT: Agent = Agent::OpenCode;

    async fn ingest(store: &Store) -> Result<Stats> {
        let mut stats = Stats::default();
        let db = db_path();
        if db.is_file() {
            whole(store, &scan(&db).await?, &mut stats).await?;
        }
        Ok(stats)
    }
}

impl ToNative for OpenCode {
    const AGENT: Agent = Agent::OpenCode;

    async fn write(session: &Session, messages: &[Message]) -> Result<String> {
        write_to(&db_path(), session, messages).await
    }

    fn resume(native_id: &str) -> Command {
        let mut c = Command::new("opencode");
        c.args(["--session", native_id]);
        c
    }
}

/// Insert `session` into OpenCode's database unless it is already there.
///
/// Rows mirror what OpenCode 1.18 writes: a `session`, one `message` per turn with a JSON
/// `data` column, and one text `part` per message. The model is whatever OpenCode used last,
/// so the resumed session continues on a provider the user has configured.
async fn write_to(db: &Path, session: &Session, messages: &[Message]) -> Result<String> {
    let mut conn =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(db)).await?;
    if session.agent == Agent::OpenCode
        && query("select 1 from session where id = ?1")
            .bind(&session.session_id)
            .fetch_optional(&mut conn)
            .await?
            .is_some()
    {
        return Ok(session.session_id.clone()); // OpenCode's own session, still there
    }
    // Imported earlier: the row is tagged with where it came from and how many messages we
    // wrote. While it still holds exactly those, it is ours to refresh; more means OpenCode
    // has continued it.
    let imported: Option<(String, Option<i64>, i64)> = query(
        "select s.id, json_extract(s.metadata, '$.atuin.messages'), \
             (select count(*) from message m where m.session_id = s.id) \
         from session s where json_extract(s.metadata, '$.atuin.agent') = ?1 \
             and json_extract(s.metadata, '$.atuin.session_id') = ?2",
    )
    .bind(session.agent.to_string())
    .bind(&session.session_id)
    .fetch_optional(&mut conn)
    .await?
    .map(|r| (r.get(0), r.get(1), r.get(2)));
    let refresh = match imported {
        Some((id, wrote, holds)) if wrote != Some(holds) => return Ok(id),
        Some((id, ..)) => Some(id),
        None => None,
    };

    let cwd = working_dir(session).to_string_lossy().into_owned();
    let project_id: String = query("select id from project where worktree = ?1")
        .bind(&cwd)
        .fetch_optional(&mut conn)
        .await?
        .map_or_else(|| "global".to_owned(), |r| r.get(0));
    let (provider, model): (String, String) = query(
        "select json_extract(data, '$.providerID'), json_extract(data, '$.modelID') from message \
         where json_extract(data, '$.role') = 'assistant' and json_extract(data, '$.modelID') is not null \
         order by time_created desc limit 1",
    )
    .fetch_optional(&mut conn)
    .await?
    .map_or_else(|| ("openai".to_owned(), "gpt-5.5".to_owned()), |r| (r.get(0), r.get(1)));
    let model_json = json!({"providerID": provider, "modelID": model});

    let mut counter = 0u64;
    let mut ids = move |prefix: &str, ms: i64| {
        counter += 1;
        // OpenCode's id shape: prefix, 12 hex chars that sort by time, 14 random alphanumerics.
        let time_part = (u64::try_from(ms).unwrap_or(0) & 0xF_FFFF_FFFF) << 12 | (counter & 0xFFF);
        let random: String = Uuid::new_v4().simple().to_string().chars().take(14).collect();
        format!("{prefix}_{time_part:012x}{random}")
    };
    let millis = |ts: time::OffsetDateTime| {
        i64::try_from(ts.unix_timestamp_nanos() / 1_000_000).unwrap_or(0)
    };
    let now = millis(time::OffsetDateTime::now_utc());
    let session_id = refresh.clone().unwrap_or_else(|| ids("ses", millis(session.started_at)));
    let turns = turns(messages);

    let mut tx = conn.begin().await?;
    if refresh.is_some() {
        for table in ["part", "message"] {
            query(sqlx::AssertSqlSafe(format!("delete from {table} where session_id = ?1")))
                .bind(&session_id)
                .execute(&mut *tx)
                .await?;
        }
        query("delete from session where id = ?1").bind(&session_id).execute(&mut *tx).await?;
    }
    query("insert or ignore into project (id, worktree, time_created, time_updated, sandboxes) values ('global', '/', ?1, ?1, '[]')")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    query(
        "insert into session (id, project_id, slug, directory, title, version, time_created, time_updated, \
         agent, model, cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write, \
         summary_additions, summary_deletions, summary_files, path, metadata) \
         values (?1, ?2, ?3, ?4, ?5, 'atuin', ?6, ?7, 'build', ?8, 0, 0, 0, 0, 0, 0, 0, 0, 0, '', ?9)",
    )
    .bind(&session_id)
    .bind(&project_id)
    .bind(format!("atuin-{}", &session_id[4..10]))
    .bind(&cwd)
    .bind(session.title.as_deref().unwrap_or("Imported session"))
    .bind(millis(session.started_at))
    .bind(millis(session.ended_at))
    .bind(json!({"id": model, "providerID": provider}).to_string())
    .bind(
        json!({"atuin": {
            "agent": session.agent.to_string(),
            "session_id": session.session_id,
            "messages": turns.len(),
        }})
        .to_string(),
    )
    .execute(&mut *tx)
    .await?;

    let mut parent: Option<String> = None;
    for turn in turns {
        let ms = millis(turn.timestamp);
        let message_id = ids("msg", ms);
        let data = match turn.role {
            Role::Assistant => json!({
                "role": "assistant", "parentID": parent, "agent": "build", "mode": "build",
                "path": {"cwd": cwd, "root": cwd},
                "cost": 0, "tokens": {"input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}},
                "modelID": model, "providerID": provider,
                "time": {"created": ms, "completed": ms},
            }),
            _ => {
                json!({"role": "user", "agent": "build", "model": model_json, "time": {"created": ms}})
            }
        };
        query("insert into message (id, session_id, time_created, time_updated, data) values (?1, ?2, ?3, ?3, ?4)")
            .bind(&message_id)
            .bind(&session_id)
            .bind(ms)
            .bind(data.to_string())
            .execute(&mut *tx)
            .await?;
        // A text part, then one completed tool part per call: OpenCode keeps a call and its
        // output in the same part.
        let mut parts = Vec::new();
        if !turn.text.trim().is_empty() {
            parts
                .push(json!({"type": "text", "text": turn.text, "time": {"start": ms, "end": ms}}));
        }
        for call in &turn.calls {
            let (tool, input) = match call.shell_command() {
                Some(command) => {
                    ("bash".to_owned(), json!({"command": command, "description": ""}))
                }
                None => (call.name.clone(), call.input.clone()),
            };
            parts.push(json!({
                "type": "tool", "tool": tool, "callID": call.id,
                "state": {
                    "status": "completed", "input": input, "output": call.output,
                    "title": "", "metadata": {}, "time": {"start": ms, "end": ms},
                },
            }));
        }
        for part in parts {
            query("insert into part (id, message_id, session_id, time_created, time_updated, data) values (?1, ?2, ?3, ?4, ?4, ?5)")
                .bind(ids("prt", ms))
                .bind(&message_id)
                .bind(&session_id)
                .bind(ms)
                .bind(part.to_string())
                .execute(&mut *tx)
                .await?;
        }
        if turn.role == Role::User {
            parent = Some(message_id);
        }
    }
    tx.commit().await?;
    Ok(session_id)
}

pub fn db_path() -> PathBuf {
    std::env::var_os("OPENCODE_SQLITE_DB")
        .map(PathBuf::from)
        .or_else(|| atuin_common::utils::env_abspath("XDG_DATA_HOME"))
        .unwrap_or_else(|| atuin_common::utils::home_dir().join(".local/share"))
        .join("opencode/opencode.db")
}

/// Read every session. Small enough to re-read whole; dedupe happens in the store.
// ponytail: full re-read each ingest, filter on session.time_updated if the db gets large
pub async fn scan(db: &Path) -> Result<Vec<Message>> {
    let mut conn =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(db).read_only(true))
            .await?;
    let mut out = Vec::new();

    let mut cwd_of: HashMap<String, Option<String>> = HashMap::new();
    let mut parent_of: HashMap<String, Option<String>> = HashMap::new();
    let mut origin_of: HashMap<String, (Agent, String, u64)> = HashMap::new();
    let mut seen: HashMap<String, u64> = HashMap::new();
    for row in atuin_common::db::query(
        "select id, parent_id, directory, title, time_created, metadata from session",
    )
    .fetch_all(&mut conn)
    .await?
    {
        let id: String = row.get(0);
        let parent: Option<String> = row.get(1);
        let dir: Option<String> = row.get(2);
        let title: String = row.get(3);
        // A session Atuin wrote here: its first messages, and its title, are a copy.
        let stamp = row
            .get::<Option<String>, _>(5)
            .and_then(|m| serde_json::from_str::<Value>(&m).ok())
            .map(|m| m["atuin"].clone())
            .filter(Value::is_object);
        if let Some(stamp) = &stamp
            && let Some(agent) = stamp["agent"].as_str().and_then(|a| a.parse::<Agent>().ok())
        {
            let source = stamp["session_id"].as_str().unwrap_or_default().to_owned();
            let wrote = stamp["messages"].as_u64().unwrap_or(u64::MAX);
            origin_of.insert(id.clone(), (agent, source, wrote));
        }
        if let Some(ts) = ts_millis(row.get(4))
            && !title.trim().is_empty()
            && stamp.is_none()
        {
            let mut m = Message::new(Agent::OpenCode, &id, "title", ts, Role::Title);
            m.parent_session_id.clone_from(&parent);
            m.cwd.clone_from(&dir);
            m.content = title;
            out.push(m);
        }
        cwd_of.insert(id.clone(), dir);
        parent_of.insert(id, parent);
    }

    let mut index_of: HashMap<String, usize> = HashMap::new();
    for row in atuin_common::db::query(
        "select id, session_id, time_created, data from message order by time_created, id",
    )
    .fetch_all(&mut conn)
    .await?
    {
        let id: String = row.get(0);
        let session_id: String = row.get(1);
        let Some(ts) = ts_millis(row.get(2)) else {
            continue;
        };
        let Ok(d) = serde_json::from_str::<Value>(&row.get::<String, _>(3)) else {
            continue;
        };
        let role = match d["role"].as_str() {
            Some("user") => Role::User,
            Some("assistant") => Role::Assistant,
            _ => continue,
        };
        let mut m = Message::new(Agent::OpenCode, &session_id, &id, ts, role);
        m.parent_session_id = parent_of.get(&session_id).cloned().flatten();
        if let Some((agent, source, wrote)) = origin_of.get(&session_id) {
            let nth = seen.entry(session_id.clone()).or_default();
            *nth += 1;
            if *nth <= *wrote {
                continue; // our copy; its parts are skipped with it
            }
            m.parent_agent = Some(*agent);
            m.parent_session_id = Some(source.clone());
        }
        m.cwd = cwd_of.get(&session_id).cloned().flatten();
        m.parent_source_id = d["parentID"].as_str().map(str::to_owned);
        m.model = d["modelID"].as_str().map(str::to_owned);
        m.stop_reason = match (d["error"]["name"].as_str(), d["finish"].as_str()) {
            (Some("MessageAbortedError"), _) => Some(StopReason::Aborted),
            (Some(_), _) => Some(StopReason::Error),
            (None, Some("stop")) => Some(StopReason::EndTurn),
            (None, Some("tool-calls")) => Some(StopReason::ToolUse),
            (None, Some("length")) => Some(StopReason::MaxTokens),
            _ => None,
        };
        if let Some(t) = d["tokens"].as_object() {
            let n = |v: &Value| v.as_u64().unwrap_or(0);
            m.tokens = Some(Tokens {
                input: n(&t["input"]),
                output: n(&t["output"]),
                cache_read: n(&t["cache"]["read"]),
                cache_write: n(&t["cache"]["write"]),
            });
        }
        index_of.insert(id, out.len());
        out.push(m);
    }

    let mut tool_results = Vec::new();
    for row in query(
        "select id, message_id, time_created, data from part \
         where json_extract(data, '$.type') in ('text', 'tool', 'compaction') \
         order by time_created, id",
    )
    .fetch_all(&mut conn)
    .await?
    {
        let id: String = row.get(0);
        let Some(&at) = index_of.get(&row.get::<String, _>(1)) else {
            continue;
        };
        let Ok(d) = serde_json::from_str::<Value>(&row.get::<String, _>(3)) else {
            continue;
        };
        let parent = &out[at];
        match d["type"].as_str() {
            Some("text") => out[at].content.push_str(d["text"].as_str().unwrap_or_default()),
            Some("compaction") => {
                let mut m = Message::new(
                    Agent::OpenCode,
                    &parent.session_id,
                    &id,
                    parent.timestamp,
                    Role::System,
                );
                m.cwd.clone_from(&parent.cwd);
                "context compacted".clone_into(&mut m.content);
                tool_results.push(m);
            }
            Some("tool") => {
                let call_id = d["callID"].as_str().unwrap_or_default().to_owned();
                let state = &d["state"];
                let ts =
                    state["time"]["end"].as_i64().and_then(ts_millis).unwrap_or(parent.timestamp);
                let mut result =
                    Message::new(Agent::OpenCode, &parent.session_id, &id, ts, Role::Tool);
                result.cwd.clone_from(&parent.cwd);
                result.tool_use_id = Some(call_id.clone());
                // A failed call has no output; what went wrong is in `error`.
                result.is_error = state["status"] == "error";
                result.content = cap_output(
                    state["output"]
                        .as_str()
                        .or(state["error"].as_str())
                        .unwrap_or_default()
                        .to_owned(),
                );
                tool_results.push(result);
                out[at].tool_calls.push(ToolCall {
                    id: call_id,
                    name: d["tool"].as_str().unwrap_or_default().to_owned(),
                    input: state["input"].to_string(),
                });
            }
            _ => {}
        }
    }

    out.extend(tool_results);
    out.retain(|m| !m.content.trim().is_empty() || !m.tool_calls.is_empty());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    /// OpenCode 1.18's tables, as far as the writer touches them.
    const SCHEMA: &str = "
        create table project (id text primary key, worktree text not null, vcs text, name text, icon_url text,
            icon_color text, time_created integer not null, time_updated integer not null, time_initialized integer,
            sandboxes text, commands text, icon_url_override text);
        create table session (id text primary key, project_id text not null, parent_id text, slug text not null,
            directory text not null, title text not null, version text not null, share_url text,
            summary_additions integer, summary_deletions integer, summary_files integer, summary_diffs text,
            revert text, permission text, time_created integer not null, time_updated integer not null,
            time_compacting integer, time_archived integer, workspace_id text, path text, agent text, model text,
            cost real default 0 not null, tokens_input integer default 0 not null, tokens_output integer default 0 not null,
            tokens_reasoning integer default 0 not null, tokens_cache_read integer default 0 not null,
            tokens_cache_write integer default 0 not null, metadata text);
        create table message (id text primary key, session_id text not null, time_created integer not null,
            time_updated integer not null, data text not null);
        create table part (id text primary key, message_id text not null, session_id text not null,
            time_created integer not null, time_updated integer not null, data text not null);";

    #[tokio::test]
    async fn to_native_round_trips_through_from_native() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        let mut conn = SqliteConnection::connect_with(
            &SqliteConnectOptions::new().filename(&db).create_if_missing(true),
        )
        .await
        .unwrap();
        for stmt in SCHEMA.split(';').filter(|s| !s.trim().is_empty()) {
            sqlx::raw_sql(stmt).execute(&mut conn).await.unwrap();
        }
        drop(conn);

        let t = datetime!(2026-09-01 10:00 UTC);
        let session = Session {
            agent: Agent::Codex,
            session_id: "th1".into(),
            parent_session_id: None,
            title: Some("Listing".into()),
            cwd: Some("/".into()),
            git_branch: None,
            model: None,
            started_at: t,
            ended_at: t,
            messages: 2,
            tool_calls: 0,
            threads: 0,
            tokens: Tokens::default(),
            last_stop: None,
        };
        let mut user = Message::new(Agent::Codex, "th1", "1", t, Role::User);
        user.content = "ls please".into();
        let mut reply = Message::new(Agent::Codex, "th1", "2", t, Role::Assistant);
        reply.content = "Done.".into();
        reply.tool_calls.push(ToolCall {
            id: "c1".into(),
            name: "exec_command".into(),
            input: r#"{"cmd":"ls"}"#.into(),
        });
        let mut result = Message::new(Agent::Codex, "th1", "3", t, Role::Tool);
        result.tool_use_id = Some("c1".into());
        result.content = "a\nb".into();

        let messages = [user, reply, result];
        let id = write_to(&db, &session, &messages).await.unwrap();
        assert!(id.starts_with("ses_") && id.len() == 4 + 12 + 14, "{id}");

        // Ingest skips what Atuin wrote here: it is a copy of a session already in the store.
        assert!(scan(&db).await.unwrap().is_empty());

        // Untouched, it is refreshed in place under the same id.
        assert_eq!(write_to(&db, &session, &messages).await.unwrap(), id);
        let mut conn = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&db))
            .await
            .unwrap();
        let rows: (i64, i64) = {
            let r = query("select (select count(*) from session), (select count(*) from message)")
                .fetch_one(&mut conn)
                .await
                .unwrap();
            (r.get(0), r.get(1))
        };
        assert_eq!(rows, (1, 2));

        // OpenCode carries the conversation on. Only that turn is ingested, linked to its source.
        query("insert into message values ('msg_zzz', ?1, 9999999999999, 9999999999999, ?2)")
            .bind(&id)
            .bind(json!({"role": "user", "time": {"created": 1}}).to_string())
            .execute(&mut conn)
            .await
            .unwrap();
        query(
            "insert into part values ('prt_zzz', 'msg_zzz', ?1, 9999999999999, 9999999999999, ?2)",
        )
        .bind(&id)
        .bind(json!({"type": "text", "text": "and now tests"}).to_string())
        .execute(&mut conn)
        .await
        .unwrap();
        let continued = scan(&db).await.unwrap();
        assert_eq!(continued.len(), 1);
        assert_eq!(continued[0].content, "and now tests");
        assert_eq!(
            (continued[0].parent_agent, continued[0].parent_session_id.as_deref()),
            (Some(Agent::Codex), Some("th1"))
        );
        // And now that OpenCode has added to it, a rewrite leaves it alone.
        assert_eq!(write_to(&db, &session, &[]).await.unwrap(), id);
        assert_eq!(scan(&db).await.unwrap().len(), 1);

        // With the stamp removed the same rows read back as the conversation that was written.
        query("update session set metadata = null").execute(&mut conn).await.unwrap();
        let back = scan(&db).await.unwrap();
        assert_eq!(
            back.iter().map(|m| (m.role, m.content.as_str())).collect::<Vec<_>>(),
            [
                (Role::Title, "Listing"),
                (Role::User, "ls please"),
                (Role::Assistant, "Done."),
                (Role::User, "and now tests"),
                (Role::Tool, "a\nb"),
            ]
        );
        assert_eq!(back[2].tool_calls[0].name, "bash");
        assert_eq!(back[2].tool_calls[0].input, r#"{"command":"ls","description":""}"#);
        assert_eq!(back[4].tool_use_id.as_deref(), Some("c1"));
        assert_eq!(back[1].cwd.as_deref(), Some("/"));
        assert_eq!(back[2].model.as_deref(), Some("gpt-5.5")); // nothing used before: the default
        assert_eq!(back[2].parent_source_id.as_deref(), Some(back[1].source_id.as_str()));

        // An OpenCode session that OpenCode still has is never written over.
        let mut own = session.clone();
        own.agent = Agent::OpenCode;
        own.session_id = id.clone();
        assert_eq!(write_to(&db, &own, &[]).await.unwrap(), id);
        assert_eq!(scan(&db).await.unwrap().len(), back.len());
    }
}
