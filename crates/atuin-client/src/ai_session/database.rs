use std::path::{Path, PathBuf};

use atuin_common::db::sqlite::{Sqlite, SqliteOpenOrCreateError};
use atuin_common::db::{self};
use atuin_common::harnesstools::session::{Content, ReadFrom, Role, Usage};
use time::OffsetDateTime;

use super::{HarnessKind, HarnessSession, Message, NativeSessionId, Session};

const COMPRESS_THRESHOLD: usize = 256;
const ZSTD_LEVEL: i32 = 3;

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
        let (content, content_z) = self.split_content(content_json)?;
        let stop_reason_json = msg.stop_reason.as_ref().map(serde_json::to_string).transpose()?;
        let (usage_input, usage_output, usage_cache_read, usage_cache_write) =
            Self::fold_usage(msg.usage.as_ref());
        let cwd = msg.cwd.as_ref().map(|p| p.to_string_lossy().into_owned());

        let inserted = db::query(
            "INSERT INTO messages (
                id, harness, session_id, source_id, parent_harness, parent_session_id,
                parent_source_id, thread, timestamp, role, content, content_z, cwd, git_branch,
                model, usage_input, usage_output, usage_cache_read, usage_cache_write, stop_reason
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .execute(&mut *tx)
        .await?;

        if inserted.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(Appended::Duplicate);
        }

        let title = msg.thread.clone();
        let preview = Self::preview_text(msg);

        db::query(
            "INSERT INTO sessions (
                harness, session_id, parent_harness, parent_session_id, cwd, git_branch, model,
                started_at, updated_at, message_count, usage_input, usage_output,
                usage_cache_read, usage_cache_write, title, preview
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(harness, session_id) DO UPDATE SET
                parent_harness = excluded.parent_harness,
                parent_session_id = excluded.parent_session_id,
                cwd = COALESCE(excluded.cwd, sessions.cwd),
                git_branch = COALESCE(excluded.git_branch, sessions.git_branch),
                model = COALESCE(excluded.model, sessions.model),
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

    pub async fn checkpoint(
        &self,
        harness: HarnessKind,
        session: &NativeSessionId,
    ) -> Result<ReadFrom, DbError> {
        let offset: Option<i64> = db::query_scalar(
            "SELECT \"offset\" FROM checkpoints WHERE harness = ? AND session_id = ?",
        )
        .bind(harness as i64)
        .bind(session.as_ref())
        .fetch_optional(self.db.pool())
        .await?;

        Ok(match offset {
            Some(offset) => ReadFrom::Offset(u64::try_from(offset).unwrap_or(0)),
            None => ReadFrom::Beginning,
        })
    }

    pub async fn set_checkpoint(
        &self,
        harness: HarnessKind,
        session: &NativeSessionId,
        offset: u64,
    ) -> Result<(), DbError> {
        db::query(
            "INSERT INTO checkpoints (harness, session_id, \"offset\") VALUES (?, ?, ?) \
             ON CONFLICT(harness, session_id) DO UPDATE SET \"offset\" = excluded.\"offset\"",
        )
        .bind(harness as i64)
        .bind(session.as_ref())
        .bind(i64::try_from(offset).unwrap_or(i64::MAX))
        .execute(self.db.pool())
        .await?;

        Ok(())
    }

    fn split_content(&self, json: String) -> Result<(String, Option<Vec<u8>>), DbError> {
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

    fn session_from_row(row: SessionRow) -> Result<Session, DbError> {
        let harness = Self::harness_from_repr(row.harness)?;
        let parent = match (row.parent_harness, row.parent_session_id) {
            (Some(harness), Some(session_id)) => Some(HarnessSession {
                harness: Self::harness_from_repr(harness)?,
                session: NativeSessionId::from(session_id),
            }),
            _ => None,
        };

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
    use atuin_common::harnesstools::session::{Content, ReadFrom, Role};
    use atuin_domain::record::RecordId;
    use rstest::rstest;
    use time::OffsetDateTime;

    use super::{AiSessionDatabase, Appended};
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

    #[rstest]
    #[tokio::test]
    async fn checkpoint_roundtrips() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let (h, sid) = (HarnessKind::Codex, NativeSessionId::from("t".to_string()));
        assert!(matches!(db.checkpoint(h, &sid).await.unwrap(), ReadFrom::Beginning));
        db.set_checkpoint(h, &sid, 42).await.unwrap();
        assert!(matches!(db.checkpoint(h, &sid).await.unwrap(), ReadFrom::Offset(42)));
    }
}
