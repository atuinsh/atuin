use std::path::{Path, PathBuf};

use atuin_common::db::sqlite::{Sqlite, SqliteOpenOrCreateError};
use atuin_common::db::{self};
use atuin_common::harnesstools::session::{Content, Role, SessionMeta, Usage};
use atuin_domain::record::RecordId;
use futures::{Stream, StreamExt, TryStreamExt};
use time::OffsetDateTime;

use super::{HarnessKind, HarnessSession, Message, NativeSessionId, Session, SourceId};

const COMPRESS_THRESHOLD: usize = 256;
const ZSTD_LEVEL: i32 = 3;

#[derive(Debug, Clone)]
pub struct AiSessionDatabase {
    db: Sqlite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appended {
    New,
    Duplicate,
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("failed to open the ai-session sqlite database: {0}")]
    Open(#[from] SqliteOpenOrCreateError),
    #[error("failed to run ai-session sqlite migrations: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("ai-session sqlite query failed: {0}")]
    Query(#[from] sqlx::Error),
    #[error("failed to serialize ai-session message: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to compress ai-session message content: {0}")]
    Compress(std::io::Error),
    #[error("stored ai-session timestamp is out of range: {0}")]
    Time(#[from] time::error::ComponentRange),
    #[error("unknown ai-session harness discriminant {0}")]
    UnknownHarness(i64),
    #[error("failed to decompress ai-session message content: {0}")]
    Decompress(std::io::Error),
    #[error("stored ai-session content is not valid utf-8")]
    InvalidContentEncoding,
    #[error("stored ai-session record id is not a valid uuid")]
    InvalidRecordId,
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    harness: i64,
    session_id: String,
    parent_harness: Option<i64>,
    parent_session_id: Option<String>,
    cwd: Option<String>,
    git_branch: Option<String>,
    model: Option<String>,
    started_at: i64,
    updated_at: i64,
    message_count: i64,
    usage_input: i64,
    usage_output: i64,
    usage_cache_read: i64,
    usage_cache_write: i64,
    title: Option<String>,
    preview: Option<String>,
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: Vec<u8>,
    harness: i64,
    session_id: String,
    source_id: String,
    parent_harness: Option<i64>,
    parent_session_id: Option<String>,
    parent_source_id: Option<String>,
    thread: Option<String>,
    timestamp: i64,
    role: String,
    content: String,
    content_z: Option<Vec<u8>>,
    cwd: Option<String>,
    git_branch: Option<String>,
    model: Option<String>,
    usage_input: i64,
    usage_output: i64,
    usage_cache_read: i64,
    usage_cache_write: i64,
    stop_reason: Option<String>,
    usage_present: i64,
}

impl AiSessionDatabase {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, DbError> {
        let db = Sqlite::builder(path.as_ref().as_os_str()).restrict_permissions().open().await?;
        let db = Self { db };
        db.migrate().await?;
        Ok(db)
    }

    pub async fn in_memory() -> Result<Self, DbError> {
        let db = Sqlite::builder_in_memory().open().await?;
        let db = Self { db };
        db.migrate().await?;
        Ok(db)
    }

    async fn migrate(&self) -> Result<(), DbError> {
        let pool = self.db.pool();
        db::migrate!(pool, "./src/ai_session/migrations").await?;
        Ok(())
    }

    pub async fn append(&self, msg: &Message) -> Result<Appended, DbError> {
        let mut tx = self.db.pool().begin().await?;

        let harness = msg.session.harness as i64;
        let session_id = msg.session.session.as_ref();
        let source_id = msg.source_id.as_ref();
        let id = msg.id.0.as_bytes().as_slice();

        let (parent_harness, parent_session_id) = match &msg.parent {
            Some(parent) => (Some(parent.harness as i64), Some(parent.session.as_ref().to_owned())),
            None => (None, None),
        };

        let timestamp = Self::millis(msg.timestamp);
        let role_json = serde_json::to_string(&msg.role)?;
        let content_json = serde_json::to_string(&msg.content)?;
        let (content, content_z) = Self::split_content(content_json)?;
        let stop_reason_json = msg.stop_reason.as_ref().map(serde_json::to_string).transpose()?;
        let (usage_input, usage_output, usage_cache_read, usage_cache_write) =
            Self::fold_usage(msg.usage.as_ref());
        let cwd = msg.cwd.as_ref().map(|p| p.to_string_lossy().into_owned());

        let inserted = db::query(
            "INSERT INTO messages (
                id, harness, session_id, source_id, parent_harness, parent_session_id,
                parent_source_id, thread, timestamp, role, content, content_z, cwd, git_branch,
                model, usage_input, usage_output, usage_cache_read, usage_cache_write, stop_reason,
                usage_present
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(harness, session_id, source_id) DO NOTHING",
        )
        .bind(id)
        .bind(harness)
        .bind(session_id)
        .bind(source_id)
        .bind(parent_harness)
        .bind(parent_session_id.clone())
        .bind(msg.parent_source_id.as_ref().map(|s| s.as_ref()))
        .bind(msg.thread.as_deref())
        .bind(timestamp)
        .bind(role_json)
        .bind(content)
        .bind(content_z)
        .bind(cwd.clone())
        .bind(msg.git_branch.as_deref())
        .bind(msg.model.as_deref())
        .bind(usage_input)
        .bind(usage_output)
        .bind(usage_cache_read)
        .bind(usage_cache_write)
        .bind(stop_reason_json)
        .bind(i64::from(msg.usage.is_some()))
        .execute(&mut *tx)
        .await?;

        if inserted.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(Appended::Duplicate);
        }

        let title: Option<String> = None;
        let preview = Self::preview_text(msg);

        db::query(
            "INSERT INTO sessions (
                harness, session_id, parent_harness, parent_session_id, cwd, git_branch, model,
                started_at, updated_at, message_count, usage_input, usage_output,
                usage_cache_read, usage_cache_write, title, preview
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(harness, session_id) DO UPDATE SET
                parent_harness = COALESCE(excluded.parent_harness, sessions.parent_harness),
                parent_session_id = COALESCE(excluded.parent_session_id, \
             sessions.parent_session_id),
                cwd = COALESCE(excluded.cwd, sessions.cwd),
                git_branch = COALESCE(excluded.git_branch, sessions.git_branch),
                model = COALESCE(excluded.model, sessions.model),
                started_at = MIN(sessions.started_at, excluded.started_at),
                updated_at = MAX(sessions.updated_at, excluded.updated_at),
                message_count = sessions.message_count + 1,
                usage_input = sessions.usage_input + excluded.usage_input,
                usage_output = sessions.usage_output + excluded.usage_output,
                usage_cache_read = sessions.usage_cache_read + excluded.usage_cache_read,
                usage_cache_write = sessions.usage_cache_write + excluded.usage_cache_write,
                title = COALESCE(excluded.title, sessions.title),
                preview = COALESCE(sessions.preview, excluded.preview)",
        )
        .bind(harness)
        .bind(session_id)
        .bind(parent_harness)
        .bind(parent_session_id)
        .bind(cwd)
        .bind(msg.git_branch.as_deref())
        .bind(msg.model.as_deref())
        .bind(timestamp)
        .bind(timestamp)
        .bind(usage_input)
        .bind(usage_output)
        .bind(usage_cache_read)
        .bind(usage_cache_write)
        .bind(title)
        .bind(preview)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(Appended::New)
    }

    pub async fn record_session_meta(
        &self,
        handle: &HarnessSession,
        meta: &SessionMeta,
    ) -> Result<(), DbError> {
        let now = Self::millis(OffsetDateTime::now_utc());
        let cwd = meta.cwd.as_ref().map(|p| p.to_string_lossy().into_owned());

        // Seed updated_at at 0, not `now`: appended messages set updated_at via
        // MAX(sessions.updated_at, excluded.updated_at), so a wall-clock seed would pin recency at
        // capture time and outrank every real message timestamp when an old session is replayed.
        db::query(
            "INSERT INTO sessions (
                harness, session_id, cwd, git_branch, model, started_at, updated_at,
                message_count, usage_input, usage_output, usage_cache_read, usage_cache_write, \
             title
            ) VALUES (?, ?, ?, ?, ?, ?, 0, 0, 0, 0, 0, 0, ?)
            ON CONFLICT(harness, session_id) DO UPDATE SET
                cwd = COALESCE(sessions.cwd, excluded.cwd),
                git_branch = COALESCE(sessions.git_branch, excluded.git_branch),
                model = COALESCE(sessions.model, excluded.model),
                title = COALESCE(excluded.title, sessions.title)",
        )
        .bind(handle.harness as i64)
        .bind(handle.session.as_ref())
        .bind(cwd)
        .bind(meta.git_branch.as_deref())
        .bind(meta.model.as_deref())
        .bind(now)
        .bind(meta.title.as_deref())
        .execute(self.db.pool())
        .await?;

        Ok(())
    }

    pub async fn contains_message(
        &self,
        session: &HarnessSession,
        source_id: &SourceId,
    ) -> Result<bool, DbError> {
        let found: Option<(i64,)> = db::query_as(
            "SELECT 1 FROM messages WHERE harness = ? AND session_id = ? AND source_id = ? LIMIT 1",
        )
        .bind(session.harness as i64)
        .bind(session.session.as_ref())
        .bind(source_id.as_ref())
        .fetch_optional(self.db.pool())
        .await?;

        Ok(found.is_some())
    }

    pub async fn get_session(&self, session: &HarnessSession) -> Result<Option<Session>, DbError> {
        let row: Option<SessionRow> = db::query_as(
            "SELECT harness, session_id, parent_harness, parent_session_id, cwd, git_branch, \
             model, started_at, updated_at, message_count, usage_input, usage_output, \
             usage_cache_read, usage_cache_write, title, preview FROM sessions WHERE harness = ? \
             AND session_id = ?",
        )
        .bind(session.harness as i64)
        .bind(session.session.as_ref())
        .fetch_optional(self.db.pool())
        .await?;

        row.map(Self::session_from_row).transpose()
    }

    pub async fn list_sessions(
        &self,
        harness: Option<HarnessKind>,
    ) -> Result<Vec<Session>, DbError> {
        let mut sql = String::from(
            "SELECT harness, session_id, parent_harness, parent_session_id, cwd, git_branch, \
             model, started_at, updated_at, message_count, usage_input, usage_output, \
             usage_cache_read, usage_cache_write, title, preview FROM sessions WHERE 1 = 1",
        );

        if harness.is_some() {
            sql.push_str(" AND harness = ?");
        }
        sql.push_str(" ORDER BY updated_at DESC");

        let mut query = db::query_as::<_, SessionRow>(sqlx::AssertSqlSafe(sql));
        if let Some(harness) = harness {
            query = query.bind(harness as i64);
        }

        let rows: Vec<SessionRow> = query.fetch_all(self.db.pool()).await?;
        rows.into_iter().map(Self::session_from_row).collect()
    }

    pub fn messages(
        &self,
        session: &HarnessSession,
    ) -> impl Stream<Item = Result<Message, DbError>> + Send + 'static {
        let pool = self.db.pool().clone();
        let harness = session.harness as i64;
        let session_id = session.session.as_ref().to_owned();

        async_stream::try_stream! {
            let mut rows = db::query_as::<_, MessageRow>(
                "SELECT id, harness, session_id, source_id, parent_harness, parent_session_id, \
                 parent_source_id, thread, timestamp, role, content, content_z, cwd, git_branch, \
                 model, usage_input, usage_output, usage_cache_read, usage_cache_write, \
                 stop_reason, usage_present FROM messages WHERE harness = ? AND session_id = ? \
                 ORDER BY timestamp, id",
            )
            .bind(harness)
            .bind(session_id)
            .fetch(&pool);

            while let Some(row) = rows.try_next().await? {
                yield Self::message_from_row(row)?;
            }
        }
    }

    pub fn transcript(
        &self,
        session: &HarnessSession,
    ) -> impl Stream<Item = Result<String, DbError>> + Send + 'static {
        self.messages(session).map(|result| result.map(|msg| Self::render_transcript_chunk(&msg)))
    }

    fn split_content(json: String) -> Result<(String, Option<Vec<u8>>), DbError> {
        if json.len() < COMPRESS_THRESHOLD {
            return Ok((json, None));
        }

        let compressed =
            zstd::stream::encode_all(json.as_bytes(), ZSTD_LEVEL).map_err(DbError::Compress)?;
        Ok((String::new(), Some(compressed)))
    }

    fn preview_text(msg: &Message) -> Option<String> {
        if msg.role != Role::User {
            return None;
        }

        msg.content.iter().find_map(|content| match content {
            Content::Text(text) => Some(text.clone()),
            _ => None,
        })
    }

    fn fold_usage(usage: Option<&Usage>) -> (i64, i64, i64, i64) {
        let Some(usage) = usage else {
            return (0, 0, 0, 0);
        };

        (
            i64::try_from(usage.input.unwrap_or(0)).unwrap_or(i64::MAX),
            i64::try_from(usage.output.unwrap_or(0)).unwrap_or(i64::MAX),
            i64::try_from(usage.cache_read.unwrap_or(0)).unwrap_or(i64::MAX),
            i64::try_from(usage.cache_write.unwrap_or(0)).unwrap_or(i64::MAX),
        )
    }

    fn millis(ts: OffsetDateTime) -> i64 {
        i64::try_from(ts.unix_timestamp_nanos() / 1_000_000).unwrap_or(i64::MAX)
    }

    fn time_from_millis(ms: i64) -> Result<OffsetDateTime, DbError> {
        Ok(OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)?)
    }

    fn harness_from_repr(n: i64) -> Result<HarnessKind, DbError> {
        match n {
            0 => Ok(HarnessKind::Unknown),
            1 => Ok(HarnessKind::ClaudeCode),
            2 => Ok(HarnessKind::Codex),
            3 => Ok(HarnessKind::Copilot),
            4 => Ok(HarnessKind::Opencode),
            5 => Ok(HarnessKind::Pi),
            other => Err(DbError::UnknownHarness(other)),
        }
    }

    fn optional_session(
        harness: Option<i64>,
        session_id: Option<String>,
    ) -> Result<Option<HarnessSession>, DbError> {
        match (harness, session_id) {
            (Some(harness), Some(session_id)) => Ok(Some(HarnessSession {
                harness: Self::harness_from_repr(harness)?,
                session: NativeSessionId::from(session_id),
            })),
            _ => Ok(None),
        }
    }

    fn read_content(content: String, content_z: Option<Vec<u8>>) -> Result<Vec<Content>, DbError> {
        let json = match content_z {
            Some(bytes) => {
                let decompressed =
                    zstd::stream::decode_all(bytes.as_slice()).map_err(DbError::Decompress)?;
                String::from_utf8(decompressed).map_err(|_| DbError::InvalidContentEncoding)?
            }
            None => content,
        };

        Ok(serde_json::from_str(&json)?)
    }

    fn message_from_row(row: MessageRow) -> Result<Message, DbError> {
        let harness = Self::harness_from_repr(row.harness)?;
        let parent = Self::optional_session(row.parent_harness, row.parent_session_id)?;
        let content = Self::read_content(row.content, row.content_z)?;
        let role: Role = serde_json::from_str(&row.role)?;
        let stop_reason = row.stop_reason.map(|s| serde_json::from_str(&s)).transpose()?;
        let id = uuid::Uuid::from_slice(&row.id).map_err(|_| DbError::InvalidRecordId)?;

        Ok(Message::builder()
            .id(RecordId(id))
            .session(HarnessSession {
                harness,
                session: NativeSessionId::from(row.session_id),
            })
            .source_id(SourceId::from(row.source_id))
            .parent(parent)
            .parent_source_id(row.parent_source_id.map(SourceId::from))
            .thread(row.thread)
            .timestamp(Self::time_from_millis(row.timestamp)?)
            .role(role)
            .content(content)
            .cwd(row.cwd.map(PathBuf::from))
            .git_branch(row.git_branch)
            .model(row.model)
            .usage((row.usage_present != 0).then(|| Usage {
                input: Some(u64::try_from(row.usage_input).unwrap_or(0)),
                output: Some(u64::try_from(row.usage_output).unwrap_or(0)),
                cache_read: Some(u64::try_from(row.usage_cache_read).unwrap_or(0)),
                cache_write: Some(u64::try_from(row.usage_cache_write).unwrap_or(0)),
            }))
            .stop_reason(stop_reason)
            .build())
    }

    fn render_transcript_chunk(message: &Message) -> String {
        let role = match &message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
            Role::Tool => "tool",
            Role::Other(other) => other.as_str(),
        };

        let body = message
            .content
            .iter()
            .filter_map(|content| match content {
                Content::Text(text) | Content::Reasoning(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Trailing newline: chunks are concatenated verbatim by consumers, so the separator has
        // to live in the chunk or every message would run together on one line.
        format!("{role}: {body}\n")
    }

    fn session_from_row(row: SessionRow) -> Result<Session, DbError> {
        let harness = Self::harness_from_repr(row.harness)?;
        let parent = Self::optional_session(row.parent_harness, row.parent_session_id)?;

        Ok(Session::builder()
            .handle(HarnessSession {
                harness,
                session: NativeSessionId::from(row.session_id),
            })
            .parent(parent)
            .cwd(row.cwd.map(PathBuf::from))
            .git_branch(row.git_branch)
            .model(row.model)
            .started_at(Self::time_from_millis(row.started_at)?)
            .updated_at(Self::time_from_millis(row.updated_at)?)
            .message_count(u64::try_from(row.message_count).unwrap_or(0))
            .usage(Usage {
                input: Some(u64::try_from(row.usage_input).unwrap_or(0)),
                output: Some(u64::try_from(row.usage_output).unwrap_or(0)),
                cache_read: Some(u64::try_from(row.usage_cache_read).unwrap_or(0)),
                cache_write: Some(u64::try_from(row.usage_cache_write).unwrap_or(0)),
            })
            .title(row.title)
            .preview(row.preview)
            .build())
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::harnesstools::session::{Content, Role, SessionMeta, Usage};
    use atuin_domain::record::RecordId;
    use futures::TryStreamExt;
    use rstest::rstest;
    use time::OffsetDateTime;

    use super::{AiSessionDatabase, Appended, COMPRESS_THRESHOLD};
    use crate::ai_session::{HarnessKind, HarnessSession, Message, NativeSessionId, SourceId};

    fn sample_message() -> Message {
        Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(HarnessSession {
                harness: HarnessKind::ClaudeCode,
                session: NativeSessionId::from("native-session".to_owned()),
            })
            .source_id(SourceId::from("source-id".to_owned()))
            .timestamp(OffsetDateTime::UNIX_EPOCH)
            .role(Role::User)
            .content(vec![Content::Text("hello".to_owned())])
            .build()
    }

    #[rstest]
    #[tokio::test]
    async fn append_is_idempotent_on_native_triple() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let m = sample_message();
        assert_eq!(db.append(&m).await.unwrap(), Appended::New);
        assert_eq!(db.append(&m).await.unwrap(), Appended::Duplicate);

        let s = db.get_session(&m.session).await.unwrap().unwrap();
        assert_eq!(s.message_count, 1);
    }

    fn message_in(session: &HarnessSession, index: i64, text: &str) -> Message {
        Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(session.clone())
            .source_id(SourceId::from(format!("source-{index}")))
            .timestamp(OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(index))
            .role(Role::User)
            .content(vec![Content::Text(text.to_owned())])
            .build()
    }

    fn sample_handle() -> HarnessSession {
        HarnessSession {
            harness: HarnessKind::ClaudeCode,
            session: NativeSessionId::from("ordered-session".to_owned()),
        }
    }

    fn three_sessions_oldest_first() -> Vec<Message> {
        (0..3)
            .map(|i| {
                let session = HarnessSession {
                    harness: HarnessKind::ClaudeCode,
                    session: NativeSessionId::from(format!("session-{i}")),
                };
                message_in(&session, i, "hello")
            })
            .collect()
    }

    fn ordered_messages(session: &HarnessSession) -> Vec<Message> {
        (0..3).map(|i| message_in(session, i, &format!("message {i}"))).collect()
    }

    #[rstest]
    #[tokio::test]
    async fn list_is_newest_first() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for m in three_sessions_oldest_first() {
            db.append(&m).await.unwrap();
        }

        let sessions = db.list_sessions(None).await.unwrap();
        assert_eq!(sessions.len(), 3);
        assert!(sessions.windows(2).all(|w| w[0].updated_at >= w[1].updated_at));
    }

    #[rstest]
    #[tokio::test]
    async fn messages_come_back_in_order() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        for m in ordered_messages(&session) {
            db.append(&m).await.unwrap();
        }
        let got: Vec<_> = db.messages(&session).try_collect().await.unwrap();
        assert!(got.windows(2).all(|w| w[0].timestamp <= w[1].timestamp));
    }

    #[rstest]
    #[tokio::test]
    async fn same_timestamp_orders_by_capture_id_not_source_id() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        let ts = OffsetDateTime::UNIX_EPOCH;

        // Two messages sharing a stored timestamp; source_ids sort opposite to capture order, so
        // ordering by source_id would reverse them. The monotonic capture id must break the tie.
        let first = Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(session.clone())
            .source_id(SourceId::from("zzz-first".to_owned()))
            .timestamp(ts)
            .role(Role::User)
            .content(vec![Content::Text("first".to_owned())])
            .build();
        let second = Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(session.clone())
            .source_id(SourceId::from("aaa-second".to_owned()))
            .timestamp(ts)
            .role(Role::User)
            .content(vec![Content::Text("second".to_owned())])
            .build();

        db.append(&first).await.unwrap();
        db.append(&second).await.unwrap();

        let got: Vec<_> = db.messages(&session).try_collect().await.unwrap();
        assert!(
            got.windows(2).all(|w| w[0].id.0 <= w[1].id.0),
            "same-timestamp messages must order by monotonic capture id, not source_id"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn content_z_round_trips_compressed_message() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        let long_text: String =
            (0..64).map(|i| format!("segment-{i:03}-distinct ")).collect::<String>();
        assert!(long_text.len() >= COMPRESS_THRESHOLD);

        let m = message_in(&session, 0, &long_text);
        db.append(&m).await.unwrap();

        let got: Vec<_> = db.messages(&session).try_collect().await.unwrap();
        assert_eq!(got.len(), 1);

        let text = match got[0].content.first() {
            Some(Content::Text(text)) => text.clone(),
            _ => panic!("expected Content::Text"),
        };
        assert_eq!(text, long_text);
    }

    #[rstest]
    #[tokio::test]
    async fn contains_message_reflects_presence() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let m = sample_message();
        assert!(!db.contains_message(&m.session, &m.source_id).await.unwrap());
        db.append(&m).await.unwrap();
        assert!(db.contains_message(&m.session, &m.source_id).await.unwrap());
    }

    #[rstest]
    #[tokio::test]
    async fn started_at_tracks_earliest_message() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        // Later message arrives first, then an earlier one (out-of-order sync / replay).
        db.append(&message_in(&session, 100, "later")).await.unwrap();
        db.append(&message_in(&session, 50, "earlier")).await.unwrap();

        let s = db.get_session(&session).await.unwrap().unwrap();
        assert_eq!(s.started_at, OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(50));
    }

    #[rstest]
    #[tokio::test]
    async fn updated_at_tracks_last_message_not_capture_time() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        // Metadata is recorded first (a rediscovered old session), then historical messages replay.
        // updated_at must reflect the newest message, not the wall-clock capture instant.
        db.record_session_meta(&session, &SessionMeta::default()).await.unwrap();
        db.append(&message_in(&session, 50, "old")).await.unwrap();

        let s = db.get_session(&session).await.unwrap().unwrap();
        let ts = OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(50);
        assert_eq!(s.updated_at, ts);
        assert_eq!(s.started_at, ts);
    }

    #[rstest]
    #[tokio::test]
    async fn usage_absence_round_trips_as_none() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();

        // A message with no usage block must not come back as Some(zeros).
        db.append(&message_in(&session, 0, "no usage")).await.unwrap();

        let with_usage = Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(session.clone())
            .source_id(SourceId::from("with-usage".to_owned()))
            .timestamp(OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1))
            .role(Role::Assistant)
            .content(vec![Content::Text("has usage".to_owned())])
            .usage(Some(Usage {
                input: Some(0),
                output: Some(0),
                cache_read: Some(0),
                cache_write: Some(0),
            }))
            .build();
        db.append(&with_usage).await.unwrap();

        let got: Vec<_> = db.messages(&session).try_collect().await.unwrap();
        assert_eq!(got[0].usage, None, "absent usage must not become Some(zeros)");
        assert!(got[1].usage.is_some(), "reported usage must survive the round trip");
    }

    #[rstest]
    #[tokio::test]
    async fn transcript_separates_messages() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        for m in ordered_messages(&session) {
            db.append(&m).await.unwrap();
        }

        let chunks: Vec<String> = db.transcript(&session).try_collect().await.unwrap();
        assert!(chunks.iter().all(|c| c.ends_with('\n')), "each chunk must be newline-terminated");
        assert_eq!(chunks.concat().lines().count(), 3, "messages must not run together");
    }
}
