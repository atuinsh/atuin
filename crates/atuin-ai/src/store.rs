use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use eyre::{Result, eyre};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use time::OffsetDateTime;

// Database row mappings — all columns are kept even if not yet read in
// non-test code, since they're part of the schema and used in tests.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct StoredSession {
    pub id: String,
    pub head_id: Option<String>,
    pub server_session_id: Option<String>,
    pub directory: Option<String>,
    pub git_root: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub archived_at: Option<i64>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct StoredEvent {
    pub id: String,
    pub session_id: String,
    pub parent_id: Option<String>,
    pub invocation_id: String,
    pub event_type: String,
    pub event_data: String,
    pub created_at: i64,
}

/// Row type returned by session queries (avoids clippy::type_complexity).
type SessionRow = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    i64,
    Option<i64>,
);

/// Row type returned by event queries.
type EventRow = (String, String, Option<String>, String, String, String, i64);

pub(crate) struct AiSessionStore {
    pool: SqlitePool,
}

impl AiSessionStore {
    pub async fn new(path: impl AsRef<Path>, timeout: f64) -> Result<Self> {
        let path = path.as_ref();
        let path_str = path
            .as_os_str()
            .to_str()
            .ok_or_else(|| eyre!("AI session database path is not valid UTF-8: {path:?}"))?;

        let is_memory = path_str.contains(":memory:");

        if !is_memory
            && !path.exists()
            && let Some(dir) = path.parent()
        {
            fs_err::create_dir_all(dir)?;
        }

        let opts = SqliteConnectOptions::from_str(path_str)?
            .journal_mode(SqliteJournalMode::Wal)
            .optimize_on_close(true, None)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .acquire_timeout(Duration::from_secs_f64(timeout))
            .connect_with(opts)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        #[cfg(unix)]
        if !is_memory {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }

        Ok(Self { pool })
    }

    pub async fn create_session(
        &self,
        id: &str,
        directory: Option<&str>,
        git_root: Option<&str>,
    ) -> Result<StoredSession> {
        let now = OffsetDateTime::now_utc().unix_timestamp();

        sqlx::query(
            "INSERT INTO sessions (id, directory, git_root, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
        )
        .bind(id)
        .bind(directory)
        .bind(git_root)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(StoredSession {
            id: id.to_string(),
            head_id: None,
            server_session_id: None,
            directory: directory.map(String::from),
            git_root: git_root.map(String::from),
            created_at: now,
            updated_at: now,
            archived_at: None,
        })
    }

    #[allow(dead_code)] // used in tests; will be used by daemon service
    pub async fn get_session(&self, id: &str) -> Result<Option<StoredSession>> {
        let row: Option<SessionRow> = sqlx::query_as(
            "SELECT id, head_id, server_session_id, directory, git_root,
                    created_at, updated_at, archived_at
             FROM sessions WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(
                id,
                head_id,
                server_session_id,
                directory,
                git_root,
                created_at,
                updated_at,
                archived_at,
            )| {
                StoredSession {
                    id,
                    head_id,
                    server_session_id,
                    directory,
                    git_root,
                    created_at,
                    updated_at,
                    archived_at,
                }
            },
        ))
    }

    /// Find the most recent non-archived session matching the given directory or git
    /// root, updated within `max_age_secs` seconds.
    pub async fn find_resumable_session(
        &self,
        directory: Option<&str>,
        git_root: Option<&str>,
        max_age_secs: i64,
    ) -> Result<Option<StoredSession>> {
        let cutoff = OffsetDateTime::now_utc().unix_timestamp() - max_age_secs;

        let row: Option<SessionRow> = sqlx::query_as(
            "SELECT id, head_id, server_session_id, directory, git_root,
                    created_at, updated_at, archived_at
             FROM sessions
             WHERE archived_at IS NULL
               AND updated_at > ?1
               AND (directory = ?2 OR (git_root IS NOT NULL AND git_root = ?3))
             ORDER BY updated_at DESC
             LIMIT 1",
        )
        .bind(cutoff)
        .bind(directory)
        .bind(git_root)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(
                id,
                head_id,
                server_session_id,
                directory,
                git_root,
                created_at,
                updated_at,
                archived_at,
            )| {
                StoredSession {
                    id,
                    head_id,
                    server_session_id,
                    directory,
                    git_root,
                    created_at,
                    updated_at,
                    archived_at,
                }
            },
        ))
    }

    /// Append a single event and update the session's `head_id` and `updated_at`.
    pub async fn append_event(
        &self,
        session_id: &str,
        event_id: &str,
        parent_id: Option<&str>,
        invocation_id: &str,
        event_type: &str,
        event_data: &str,
    ) -> Result<()> {
        let now = OffsetDateTime::now_utc().unix_timestamp();

        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO session_events (id, session_id, parent_id, invocation_id, event_type, event_data, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(event_id)
        .bind(session_id)
        .bind(parent_id)
        .bind(invocation_id)
        .bind(event_type)
        .bind(event_data)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        sqlx::query("UPDATE sessions SET head_id = ?1, updated_at = ?2 WHERE id = ?3")
            .bind(event_id)
            .bind(now)
            .bind(session_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Load all events for a session, ordered chronologically.
    pub async fn load_events(&self, session_id: &str) -> Result<Vec<StoredEvent>> {
        let rows: Vec<EventRow> = sqlx::query_as(
            "SELECT id, session_id, parent_id, invocation_id, event_type, event_data, created_at
                 FROM session_events
                 WHERE session_id = ?1
                 ORDER BY created_at ASC, rowid ASC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(id, session_id, parent_id, invocation_id, event_type, event_data, created_at)| {
                    StoredEvent {
                        id,
                        session_id,
                        parent_id,
                        invocation_id,
                        event_type,
                        event_data,
                        created_at,
                    }
                },
            )
            .collect())
    }

    pub async fn update_server_session_id(
        &self,
        session_id: &str,
        server_session_id: &str,
    ) -> Result<()> {
        sqlx::query("UPDATE sessions SET server_session_id = ?1 WHERE id = ?2")
            .bind(server_session_id)
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn archive_session(&self, session_id: &str) -> Result<()> {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        sqlx::query("UPDATE sessions SET archived_at = ?1 WHERE id = ?2")
            .bind(now)
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn new_test_store() -> AiSessionStore {
        AiSessionStore::new("sqlite::memory:", 2.0).await.unwrap()
    }

    #[tokio::test]
    async fn test_create_and_get_session() {
        let store = new_test_store().await;

        let session = store
            .create_session("s1", Some("/home/user/project"), Some("/home/user/project"))
            .await
            .unwrap();
        assert_eq!(session.id, "s1");
        assert!(session.head_id.is_none());
        assert!(session.archived_at.is_none());

        let loaded = store.get_session("s1").await.unwrap().unwrap();
        assert_eq!(loaded.id, "s1");
        assert_eq!(loaded.directory.as_deref(), Some("/home/user/project"));
    }

    #[tokio::test]
    async fn test_get_nonexistent_session() {
        let store = new_test_store().await;
        assert!(store.get_session("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_append_and_load_events() {
        let store = new_test_store().await;
        store
            .create_session("s1", Some("/tmp"), None)
            .await
            .unwrap();

        store
            .append_event(
                "s1",
                "e1",
                None,
                "inv1",
                "user_message",
                r#"{"content":"hello"}"#,
            )
            .await
            .unwrap();
        store
            .append_event(
                "s1",
                "e2",
                Some("e1"),
                "inv1",
                "text",
                r#"{"content":"hi there"}"#,
            )
            .await
            .unwrap();

        let events = store.load_events("s1").await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "e1");
        assert!(events[0].parent_id.is_none());
        assert_eq!(events[0].invocation_id, "inv1");
        assert_eq!(events[1].id, "e2");
        assert_eq!(events[1].parent_id.as_deref(), Some("e1"));

        let session = store.get_session("s1").await.unwrap().unwrap();
        assert_eq!(session.head_id.as_deref(), Some("e2"));
    }

    #[tokio::test]
    async fn test_find_resumable_session() {
        let store = new_test_store().await;
        store
            .create_session("s1", Some("/home/user/project"), None)
            .await
            .unwrap();

        let found = store
            .find_resumable_session(Some("/home/user/project"), None, 3600)
            .await
            .unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "s1");
    }

    #[tokio::test]
    async fn test_find_resumable_by_git_root() {
        let store = new_test_store().await;
        store
            .create_session(
                "s1",
                Some("/home/user/project/sub"),
                Some("/home/user/project"),
            )
            .await
            .unwrap();

        let found = store
            .find_resumable_session(Some("/different/dir"), Some("/home/user/project"), 3600)
            .await
            .unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "s1");
    }

    #[tokio::test]
    async fn test_find_resumable_skips_archived() {
        let store = new_test_store().await;
        store
            .create_session("s1", Some("/tmp"), None)
            .await
            .unwrap();
        store.archive_session("s1").await.unwrap();

        let found = store
            .find_resumable_session(Some("/tmp"), None, 3600)
            .await
            .unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_find_resumable_no_match_different_dir() {
        let store = new_test_store().await;
        store
            .create_session("s1", Some("/home/user/project"), None)
            .await
            .unwrap();

        let found = store
            .find_resumable_session(Some("/other/dir"), None, 3600)
            .await
            .unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_archive_session() {
        let store = new_test_store().await;
        store
            .create_session("s1", Some("/tmp"), None)
            .await
            .unwrap();

        store.archive_session("s1").await.unwrap();

        let session = store.get_session("s1").await.unwrap().unwrap();
        assert!(session.archived_at.is_some());
    }

    #[tokio::test]
    async fn test_update_server_session_id() {
        let store = new_test_store().await;
        store
            .create_session("s1", Some("/tmp"), None)
            .await
            .unwrap();

        store
            .update_server_session_id("s1", "server-abc")
            .await
            .unwrap();

        let session = store.get_session("s1").await.unwrap().unwrap();
        assert_eq!(session.server_session_id.as_deref(), Some("server-abc"));
    }

    #[tokio::test]
    async fn test_find_resumable_does_not_mutate() {
        let store = new_test_store().await;
        store
            .create_session("s1", Some("/tmp"), None)
            .await
            .unwrap();

        let before = store.get_session("s1").await.unwrap().unwrap();
        store
            .find_resumable_session(Some("/tmp"), None, 3600)
            .await
            .unwrap()
            .unwrap();
        let after = store.get_session("s1").await.unwrap().unwrap();

        assert_eq!(before.updated_at, after.updated_at);
    }

    #[tokio::test]
    async fn test_events_ordered_chronologically() {
        let store = new_test_store().await;
        store
            .create_session("s1", Some("/tmp"), None)
            .await
            .unwrap();

        store
            .append_event("s1", "e1", None, "inv1", "user_message", "{}")
            .await
            .unwrap();
        store
            .append_event("s1", "e2", Some("e1"), "inv1", "text", "{}")
            .await
            .unwrap();
        store
            .append_event("s1", "e3", Some("e2"), "inv2", "user_message", "{}")
            .await
            .unwrap();

        let events = store.load_events("s1").await.unwrap();
        assert_eq!(events.len(), 3);
        assert!(events[0].created_at <= events[1].created_at);
        assert!(events[1].created_at <= events[2].created_at);
        assert_eq!(events[2].invocation_id, "inv2");
    }
}
