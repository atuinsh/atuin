//! Local SQLite store. UUIDs are 16-byte blobs, enums their `u8` discriminant, timestamps unix
//! millis. Inserts are idempotent on `(agent, session_id, source_id)`.

use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use atuin_common::db;
use atuin_common::db::sqlite::{Sqlite, SqliteBuilder};
use eyre::{Result, eyre};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Agent, Message, Role, Session, StopReason, Tokens, ToolCall};

#[derive(Debug, Clone)]
pub struct Store {
    sqlite: Sqlite,
}

impl Store {
    pub async fn open(path: &Path, timeout: Duration) -> Result<Self> {
        Self::from_builder(Sqlite::builder(path.as_os_str()), timeout).await
    }

    pub async fn in_memory() -> Result<Self> {
        Self::from_builder(Sqlite::builder_in_memory(), Duration::from_secs(1)).await
    }

    async fn from_builder(builder: SqliteBuilder<'_>, timeout: Duration) -> Result<Self> {
        let sqlite = builder.timeout(timeout).foreign_keys(true).open().await?;
        db::migrate!(sqlite.pool(), "./migrations").await?;
        let store = Self { sqlite };
        store.compact().await?;
        Ok(store)
    }

    /// Compress anything stored before compression existed, then give the space back. A no-op
    /// on a store that is already compact, so it is safe to run on every open.
    async fn compact(&self) -> Result<()> {
        let pool = self.sqlite.pool();
        let mut changed = false;
        loop {
            let rows = db::query(
                "select id, content from messages \
                 where role = ?1 and content_z is null and length(content) >= ?2 limit 2000",
            )
            .bind(Role::Tool as u8)
            .bind(i64::try_from(COMPRESS_MIN)?)
            .fetch_all(pool)
            .await?;
            if rows.is_empty() {
                break;
            }
            changed = true;
            let mut tx = pool.begin().await?;
            for row in &rows {
                db::query("update messages set content = '', content_z = ?2 where id = ?1")
                    .bind(row.get::<&[u8], _>(0))
                    .bind(zstd::stream::encode_all(row.get::<&str, _>(1).as_bytes(), ZSTD_LEVEL)?)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
        }
        loop {
            let rows = db::query(
                "select message_id, id, input from tool_calls \
                 where input_z is null and length(input) >= ?1 limit 2000",
            )
            .bind(i64::try_from(COMPRESS_MIN)?)
            .fetch_all(pool)
            .await?;
            if rows.is_empty() {
                break;
            }
            changed = true;
            let mut tx = pool.begin().await?;
            for row in &rows {
                db::query("update tool_calls set input = '', input_z = ?3 where message_id = ?1 and id = ?2")
                    .bind(row.get::<&[u8], _>(0))
                    .bind(row.get::<&str, _>(1))
                    .bind(zstd::stream::encode_all(row.get::<&str, _>(2).as_bytes(), ZSTD_LEVEL)?)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
        }
        if changed {
            db::query("vacuum").execute(pool).await?;
        }
        Ok(())
    }

    /// Insert messages, skipping ones already present. Returns how many were new.
    pub async fn insert(&self, messages: &[Message]) -> Result<usize> {
        let mut tx = self.sqlite.pool().begin().await?;
        let mut inserted = 0;
        for m in messages {
            // Tool results are nearly all the bytes and compress well; prose stays plain so the
            // session queries can read it in SQL.
            let (content, content_z) = if m.role == Role::Tool {
                squeeze(&m.content)?
            } else {
                (m.content.as_str(), None)
            };
            let rows = db::query(
                "insert or ignore into messages (id, agent, session_id, parent_session_id, thread, \
                 source_id, parent_source_id, timestamp, role, content, tool_use_id, cwd, \
                 git_branch, model, tokens_input, tokens_output, tokens_cache_read, \
                 tokens_cache_write, is_error, stop_reason, content_z, parent_agent) values \
                 (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
                 ?19, ?20, ?21, ?22)",
            )
            .bind(m.id.as_bytes().as_slice())
            .bind(m.agent as u8)
            .bind(&m.session_id)
            .bind(&m.parent_session_id)
            .bind(&m.thread)
            .bind(&m.source_id)
            .bind(&m.parent_source_id)
            .bind(millis(m.timestamp))
            .bind(m.role as u8)
            .bind(content)
            .bind(&m.tool_use_id)
            .bind(&m.cwd)
            .bind(&m.git_branch)
            .bind(&m.model)
            .bind(m.tokens.map(|t| i64::try_from(t.input).unwrap_or(i64::MAX)))
            .bind(m.tokens.map(|t| i64::try_from(t.output).unwrap_or(i64::MAX)))
            .bind(m.tokens.map(|t| i64::try_from(t.cache_read).unwrap_or(i64::MAX)))
            .bind(m.tokens.map(|t| i64::try_from(t.cache_write).unwrap_or(i64::MAX)))
            .bind(m.is_error)
            .bind(m.stop_reason.map(|r| r as u8))
            .bind(content_z)
            .bind(m.parent_agent.map(|a| a as u8))
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if rows == 0 {
                continue;
            }
            inserted += 1;
            for tc in &m.tool_calls {
                let (input, input_z) = squeeze(&tc.input)?;
                db::query(
                    "insert or ignore into tool_calls (message_id, id, name, input, input_z) \
                     values (?1, ?2, ?3, ?4, ?5)",
                )
                .bind(m.id.as_bytes().as_slice())
                .bind(&tc.id)
                .bind(&tc.name)
                .bind(input)
                .bind(input_z)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(inserted)
    }

    /// Every message, oldest first, with tool calls attached.
    pub async fn all(&self) -> Result<Vec<Message>> {
        let rows = db::query(
            "select id, agent, session_id, parent_session_id, thread, source_id, \
                 parent_source_id, timestamp, role, content, tool_use_id, cwd, git_branch, model, \
                 tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, is_error, stop_reason, \
             content_z, parent_agent \
                 from messages order by timestamp, id",
        )
        .fetch_all(self.sqlite.pool())
        .await?;
        let mut messages = rows.iter().map(message_from_row).collect::<Result<Vec<_>>>()?;
        let calls = db::query("select message_id, id, name, input, input_z from tool_calls")
            .fetch_all(self.sqlite.pool())
            .await?;
        attach_tool_calls(&mut messages, &calls)?;
        Ok(messages)
    }

    /// One session's messages, oldest first, with tool calls attached.
    pub async fn session_messages(&self, agent: Agent, session_id: &str) -> Result<Vec<Message>> {
        let rows = db::query(
            "select id, agent, session_id, parent_session_id, thread, source_id, \
             parent_source_id, timestamp, role, content, tool_use_id, cwd, git_branch, model, \
             tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, is_error, stop_reason, \
             content_z, parent_agent \
             from messages where agent = ?1 and session_id = ?2 order by timestamp, id",
        )
        .bind(agent as u8)
        .bind(session_id)
        .fetch_all(self.sqlite.pool())
        .await?;
        let mut messages = rows.iter().map(message_from_row).collect::<Result<Vec<_>>>()?;
        let calls = db::query(
            "select tc.message_id, tc.id, tc.name, tc.input, tc.input_z from tool_calls tc \
             join messages m on m.id = tc.message_id where m.agent = ?1 and m.session_id = ?2",
        )
        .bind(agent as u8)
        .bind(session_id)
        .fetch_all(self.sqlite.pool())
        .await?;
        attach_tool_calls(&mut messages, &calls)?;
        Ok(messages)
    }

    /// A session's messages preceded by those of every session it was handed off from, oldest
    /// first. What a reopen needs: an agent's continuation of a handoff holds only the turns
    /// that agent added, and the rest lives in its ancestors.
    pub async fn session_history(&self, agent: Agent, session_id: &str) -> Result<Vec<Message>> {
        let handed_from = |messages: &[Message]| {
            messages.iter().find_map(|m| m.parent_agent.zip(m.parent_session_id.clone()))
        };
        let mut history = self.session_messages(agent, session_id).await?;
        let mut parent = handed_from(&history);
        // ponytail: a fixed bound instead of cycle detection; handoff chains are a few links
        for _ in 0..8 {
            let Some((parent_agent, parent_id)) = parent else {
                break;
            };
            let mut older = self.session_messages(parent_agent, &parent_id).await?;
            parent = handed_from(&older);
            older.append(&mut history);
            history = older;
        }
        Ok(history)
    }

    /// The last `limit` main-thread messages of a session, oldest first, bodies cut to
    /// `max_chars`. For previews: a long session costs the same as a short one.
    pub async fn session_tail(
        &self,
        agent: Agent,
        session_id: &str,
        limit: u32,
        max_chars: u32,
    ) -> Result<Vec<Message>> {
        let rows = db::query(
            "select id, agent, session_id, parent_session_id, thread, source_id, parent_source_id, \
             timestamp, role, content, tool_use_id, cwd, git_branch, model, \
             tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, is_error, stop_reason, \
             content_z, parent_agent \
             from messages where agent = ?1 and session_id = ?2 and thread is null and role != 5 \
             order by timestamp desc, id desc limit ?3",
        )
        .bind(agent as u8)
        .bind(session_id)
        .bind(limit)
        .fetch_all(self.sqlite.pool())
        .await?;
        let mut messages = rows.iter().rev().map(message_from_row).collect::<Result<Vec<_>>>()?;
        // Cut after decompressing: a compressed body cannot be shortened in SQL.
        for m in &mut messages {
            if let Some((at, _)) = m.content.char_indices().nth(max_chars as usize) {
                m.content.truncate(at);
            }
        }
        let calls = db::query(
            "select message_id, id, name, input, input_z from tool_calls where message_id in \
             (select id from messages where agent = ?1 and session_id = ?2 and thread is null \
              and role != 5 order by timestamp desc, id desc limit ?3)",
        )
        .bind(agent as u8)
        .bind(session_id)
        .bind(limit)
        .fetch_all(self.sqlite.pool())
        .await?;
        attach_tool_calls(&mut messages, &calls)?;
        Ok(messages)
    }

    /// Every session, derived in SQL, newest activity first. Same shape as [`crate::sessions`]
    /// without loading message bodies.
    pub async fn sessions(&self) -> Result<Vec<Session>> {
        let pool = self.sqlite.pool();
        let tool_counts = db::query(
            "select m.agent, m.session_id, count(*) from tool_calls tc \
             join messages m on m.id = tc.message_id group by m.agent, m.session_id",
        )
        .fetch_all(pool)
        .await?;
        let tool_counts: std::collections::HashMap<(i64, String), i64> =
            tool_counts.iter().map(|r| ((r.get(0), r.get(1)), r.get(2))).collect();

        let rows = db::query(
            "select m.agent, m.session_id, min(m.timestamp), max(m.timestamp), count(*), \
                 count(distinct m.thread), \
                 coalesce(sum(m.tokens_input), 0), coalesce(sum(m.tokens_output), 0), \
                 coalesce(sum(m.tokens_cache_read), 0), coalesce(sum(m.tokens_cache_write), 0), \
                 (select content from messages t where t.agent = m.agent and t.session_id = m.session_id \
                     and t.role = 5 \
                     order by t.source_id like 'custom-title:%' desc, t.timestamp desc limit 1), \
                 (select content from messages t where t.agent = m.agent and t.session_id = m.session_id \
                     and t.role = 1 and t.thread is null \
                     and substr(ltrim(t.content, ' ' || char(9) || char(10) || char(13)), 1, 1) \
                         not in ('', '<', '[') \
                     order by t.timestamp limit 1), \
                 (select cwd from messages t where t.agent = m.agent and t.session_id = m.session_id \
                     and t.cwd is not null order by t.timestamp desc limit 1), \
                 (select git_branch from messages t where t.agent = m.agent and t.session_id = m.session_id \
                     and t.git_branch is not null order by t.timestamp desc limit 1), \
                 (select model from messages t where t.agent = m.agent and t.session_id = m.session_id \
                     and t.model is not null order by t.timestamp desc limit 1), \
                 (select parent_session_id from messages t where t.agent = m.agent \
                     and t.session_id = m.session_id and t.parent_session_id is not null \
                     order by t.timestamp desc limit 1), \
                 (select stop_reason from messages t where t.agent = m.agent \
                     and t.session_id = m.session_id and t.role = 2 and t.thread is null \
                     order by t.timestamp desc limit 1) \
             from messages m group by m.agent, m.session_id order by max(m.timestamp) desc",
        )
        .fetch_all(pool)
        .await?;

        rows.iter()
            .map(|r| {
                let agent_repr: i64 = r.get(0);
                let session_id: String = r.get(1);
                let agent = u8::try_from(agent_repr)
                    .ok()
                    .and_then(Agent::from_repr)
                    .ok_or_else(|| eyre!("unknown agent {agent_repr}"))?;
                let count = |i: usize| usize::try_from(r.get::<i64, _>(i)).unwrap_or(0);
                let tokens = |i: usize| u64::try_from(r.get::<i64, _>(i)).unwrap_or(0);
                let title: Option<String> = r.get(10);
                let first_prompt: Option<String> = r.get(11);
                Ok(Session {
                    agent,
                    tool_calls: tool_counts
                        .get(&(agent_repr, session_id.clone()))
                        .map_or(0, |&n| usize::try_from(n).unwrap_or(0)),
                    session_id,
                    parent_session_id: r.get(15),
                    last_stop: r
                        .get::<Option<i64>, _>(16)
                        .and_then(|v| u8::try_from(v).ok())
                        .and_then(StopReason::from_repr),
                    title: title.or(first_prompt).map(|t| {
                        t.lines().next().unwrap_or("").chars().take(80).collect::<String>()
                    }),
                    cwd: r.get(12),
                    git_branch: r.get(13),
                    model: r.get(14),
                    started_at: crate::ts_millis(r.get(2)).ok_or_else(|| eyre!("bad timestamp"))?,
                    ended_at: crate::ts_millis(r.get(3)).ok_or_else(|| eyre!("bad timestamp"))?,
                    messages: count(4),
                    threads: count(5),
                    tokens: Tokens {
                        input: tokens(6),
                        output: tokens(7),
                        cache_read: tokens(8),
                        cache_write: tokens(9),
                    },
                })
            })
            .collect()
    }

    pub async fn file_offset(&self, agent: Agent, path: &OsStr) -> Result<u64> {
        let offset: Option<i64> =
            db::query_scalar("select offset from ingest_files where agent = ?1 and path = ?2")
                .bind(agent as u8)
                .bind(path.to_string_lossy().as_ref())
                .fetch_optional(self.sqlite.pool())
                .await?;
        Ok(offset.and_then(|o| u64::try_from(o).ok()).unwrap_or(0))
    }

    pub async fn set_file_offset(&self, agent: Agent, path: &OsStr, offset: u64) -> Result<()> {
        db::query(
            "insert into ingest_files (agent, path, offset) values (?1, ?2, ?3) \
             on conflict (agent, path) do update set offset = excluded.offset",
        )
        .bind(agent as u8)
        .bind(path.to_string_lossy().as_ref())
        .bind(i64::try_from(offset)?)
        .execute(self.sqlite.pool())
        .await?;
        Ok(())
    }
}

/// Text shorter than this is stored plain: a zstd frame costs more than it saves.
const COMPRESS_MIN: usize = 256;
/// Per-row level 9 and a trained dictionary were each within 15% of this on real tool results.
const ZSTD_LEVEL: i32 = 3;

/// What to store for `text`: itself, or an empty string and its zstd frame.
fn squeeze(text: &str) -> Result<(&str, Option<Vec<u8>>)> {
    if text.len() < COMPRESS_MIN {
        return Ok((text, None));
    }
    Ok(("", Some(zstd::stream::encode_all(text.as_bytes(), ZSTD_LEVEL)?)))
}

fn unsqueeze(text: String, z: Option<Vec<u8>>) -> Result<String> {
    match z {
        Some(z) => Ok(String::from_utf8(zstd::stream::decode_all(z.as_slice())?)?),
        None => Ok(text),
    }
}

fn attach_tool_calls(messages: &mut [Message], calls: &[SqliteRow]) -> Result<()> {
    let by_id: std::collections::HashMap<Uuid, usize> =
        messages.iter().enumerate().map(|(i, m)| (m.id, i)).collect();
    for row in calls {
        let id = Uuid::from_slice(row.get::<&[u8], _>(0))?;
        if let Some(&i) = by_id.get(&id) {
            messages[i].tool_calls.push(ToolCall {
                id: row.get(1),
                name: row.get(2),
                input: unsqueeze(row.get(3), row.get(4))?,
            });
        }
    }
    Ok(())
}

fn millis(ts: OffsetDateTime) -> i64 {
    i64::try_from(ts.unix_timestamp_nanos() / 1_000_000).unwrap_or(i64::MAX)
}

fn message_from_row(row: &SqliteRow) -> Result<Message> {
    let repr = |col: &str| -> Result<u8> {
        u8::try_from(row.get::<i64, _>(col)).map_err(|e| eyre!("{col}: {e}"))
    };
    let agent = Agent::from_repr(repr("agent")?).ok_or_else(|| eyre!("unknown agent"))?;
    let role = Role::from_repr(repr("role")?).ok_or_else(|| eyre!("unknown role"))?;
    let timestamp = crate::ts_millis(row.get("timestamp")).ok_or_else(|| eyre!("bad timestamp"))?;
    let tok = |col: &str| row.get::<Option<i64>, _>(col).and_then(|v| u64::try_from(v).ok());
    Ok(Message {
        id: Uuid::from_slice(row.get::<&[u8], _>("id"))?,
        agent,
        session_id: row.get("session_id"),
        parent_session_id: row.get("parent_session_id"),
        parent_agent: row
            .get::<Option<i64>, _>("parent_agent")
            .and_then(|a| u8::try_from(a).ok())
            .and_then(Agent::from_repr),
        thread: row.get("thread"),
        source_id: row.get("source_id"),
        parent_source_id: row.get("parent_source_id"),
        timestamp,
        role,
        content: unsqueeze(row.get("content"), row.get("content_z"))?,
        tool_calls: Vec::new(),
        tool_use_id: row.get("tool_use_id"),
        is_error: row.get("is_error"),
        stop_reason: row
            .get::<Option<i64>, _>("stop_reason")
            .and_then(|r| u8::try_from(r).ok())
            .and_then(StopReason::from_repr),
        cwd: row.get("cwd"),
        git_branch: row.get("git_branch"),
        model: row.get("model"),
        tokens: tok("tokens_input").map(|input| Tokens {
            input,
            output: tok("tokens_output").unwrap_or(0),
            cache_read: tok("tokens_cache_read").unwrap_or(0),
            cache_write: tok("tokens_cache_write").unwrap_or(0),
        }),
    })
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    #[tokio::test]
    async fn insert_is_idempotent_and_round_trips() {
        let store = Store::in_memory().await.unwrap();
        let mut a = Message::new(
            Agent::ClaudeCode,
            "s1",
            "u1",
            datetime!(2026-09-01 10:00:00.123 UTC),
            Role::Assistant,
        );
        a.content = "run it".into();
        a.cwd = Some("/p".into());
        a.tokens = Some(Tokens {
            input: 3,
            output: 1,
            cache_read: 0,
            cache_write: 2,
        });
        a.tool_calls.push(ToolCall {
            id: "t1".into(),
            name: "Bash".into(),
            input: "{}".into(),
        });
        let mut b =
            Message::new(Agent::Codex, "th", "128", datetime!(2026-09-01 10:00:01 UTC), Role::Tool);
        b.tool_use_id = Some("c1".into());
        b.is_error = true;
        a.stop_reason = Some(StopReason::Aborted);

        b.content = "line of output\n".repeat(400); // 6KB: stored as a zstd frame
        a.tool_calls[0].input = format!(r#"{{"content":"{}"}}"#, "x".repeat(1000));

        assert_eq!(store.insert(&[a.clone(), b.clone()]).await.unwrap(), 2);
        let (plain, packed): (i64, i64) = {
            let r =
                db::query("select length(content), length(content_z) from messages where role = 4")
                    .fetch_one(store.sqlite.pool())
                    .await
                    .unwrap();
            (r.get(0), r.get(1))
        };
        assert!(plain == 0 && packed > 0 && packed < 200, "{plain} {packed}");

        // A row written before compression existed is packed on the next open, and reads the same.
        db::query("update messages set content = ?1, content_z = null where role = 4")
            .bind(&b.content)
            .execute(store.sqlite.pool())
            .await
            .unwrap();
        store.compact().await.unwrap();
        let packed: i64 =
            db::query_scalar("select count(*) from messages where content_z is not null")
                .fetch_one(store.sqlite.pool())
                .await
                .unwrap();
        assert_eq!(packed, 1);
        // Same dedupe key, fresh Atuin id: still a duplicate.
        let mut again = a.clone();
        again.id = Uuid::now_v7();
        assert_eq!(store.insert(&[again, b.clone()]).await.unwrap(), 0);

        assert_eq!(store.all().await.unwrap(), vec![a.clone(), b.clone()]);
        assert_eq!(store.session_messages(Agent::ClaudeCode, "s1").await.unwrap(), vec![a.clone()]);
        assert_eq!(store.sessions().await.unwrap(), crate::sessions([&a, &b]));
        let tail = store.session_tail(Agent::ClaudeCode, "s1", 10, 3).await.unwrap();
        assert_eq!((tail.len(), tail[0].content.as_str(), tail[0].tool_calls.len()), (1, "run", 1));

        let path = OsStr::new("/x/y.jsonl");
        assert_eq!(store.file_offset(Agent::Codex, path).await.unwrap(), 0);
        store.set_file_offset(Agent::Codex, path, 4096).await.unwrap();
        store.set_file_offset(Agent::Codex, path, 8192).await.unwrap();
        assert_eq!(store.file_offset(Agent::Codex, path).await.unwrap(), 8192);
        assert_eq!(store.file_offset(Agent::ClaudeCode, path).await.unwrap(), 0);
    }
}
