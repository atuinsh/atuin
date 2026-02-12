use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use atuin_common::record::HostId;
use eyre::{Result, eyre};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::sync::OnceCell;
use uuid::Uuid;

// Filenames for the legacy plain-text files that we migrate from.
const LEGACY_HOST_ID_FILENAME: &str = "host_id";
const LEGACY_LAST_SYNC_FILENAME: &str = "last_sync_time";
const LEGACY_LAST_VERSION_CHECK_FILENAME: &str = "last_version_check_time";
const LEGACY_LATEST_VERSION_FILENAME: &str = "latest_version";
const LEGACY_SESSION_FILENAME: &str = "session";

const KEY_HOST_ID: &str = "host_id";
const KEY_LAST_SYNC: &str = "last_sync_time";
const KEY_LAST_VERSION_CHECK: &str = "last_version_check_time";
const KEY_LATEST_VERSION: &str = "latest_version";
const KEY_SESSION: &str = "session";
const KEY_HUB_SESSION: &str = "hub_session";
const KEY_FILES_MIGRATED: &str = "files_migrated";

pub struct MetaStore {
    pool: SqlitePool,
    cached_host_id: OnceCell<HostId>,
}

impl MetaStore {
    pub async fn new(path: impl AsRef<Path>, timeout: f64) -> Result<Self> {
        let path = path.as_ref();
        let path_str = path
            .as_os_str()
            .to_str()
            .ok_or_else(|| eyre!("meta database path is not valid UTF-8: {path:?}"))?;
        debug!("opening meta sqlite database at {path:?}");

        let is_memory = path_str.contains(":memory:");

        if !is_memory
            && !path.exists()
            && let Some(dir) = path.parent()
        {
            fs_err::create_dir_all(dir)?;
        }

        // Use DELETE journal mode instead of WAL. This is a small, infrequently-
        // written KV store — WAL's concurrency benefits aren't needed, and DELETE
        // mode avoids creating auxiliary -wal/-shm files that complicate
        // permission handling.
        let opts = SqliteConnectOptions::from_str(path_str)?
            .journal_mode(SqliteJournalMode::Delete)
            .optimize_on_close(true, None)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .acquire_timeout(Duration::from_secs_f64(timeout))
            .connect_with(opts)
            .await?;

        sqlx::migrate!("./meta-migrations").run(&pool).await?;

        // Session tokens are stored in this database, so restrict permissions.
        #[cfg(unix)]
        if !is_memory {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }

        let store = Self {
            pool,
            cached_host_id: OnceCell::const_new(),
        };

        if !is_memory {
            store.migrate_files().await?;
        }

        Ok(store)
    }

    // Generic key-value operations

    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT value FROM meta WHERE key = ?1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|r| r.0))
    }

    pub async fn set(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO meta (key, value, updated_at) VALUES (?1, ?2, strftime('%s', 'now'))
             ON CONFLICT(key) DO UPDATE SET value = ?2, updated_at = strftime('%s', 'now')",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        sqlx::query("DELETE FROM meta WHERE key = ?1")
            .bind(key)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    // Typed accessors

    pub async fn host_id(&self) -> Result<HostId> {
        self.cached_host_id
            .get_or_try_init(|| async {
                if let Some(id) = self.get(KEY_HOST_ID).await? {
                    let parsed = Uuid::from_str(id.as_str())
                        .map_err(|e| eyre!("failed to parse host ID: {e}"))?;
                    return Ok(HostId(parsed));
                }

                let uuid = atuin_common::utils::uuid_v7();
                self.set(KEY_HOST_ID, uuid.as_simple().to_string().as_ref())
                    .await?;

                Ok(HostId(uuid))
            })
            .await
            .copied()
    }

    pub async fn last_sync(&self) -> Result<OffsetDateTime> {
        match self.get(KEY_LAST_SYNC).await? {
            Some(v) => Ok(OffsetDateTime::parse(v.as_str(), &Rfc3339)?),
            None => Ok(OffsetDateTime::UNIX_EPOCH),
        }
    }

    pub async fn save_sync_time(&self) -> Result<()> {
        self.set(
            KEY_LAST_SYNC,
            OffsetDateTime::now_utc().format(&Rfc3339)?.as_str(),
        )
        .await
    }

    pub async fn last_version_check(&self) -> Result<OffsetDateTime> {
        match self.get(KEY_LAST_VERSION_CHECK).await? {
            Some(v) => Ok(OffsetDateTime::parse(v.as_str(), &Rfc3339)?),
            None => Ok(OffsetDateTime::UNIX_EPOCH),
        }
    }

    pub async fn save_version_check_time(&self) -> Result<()> {
        self.set(
            KEY_LAST_VERSION_CHECK,
            OffsetDateTime::now_utc().format(&Rfc3339)?.as_str(),
        )
        .await
    }

    pub async fn latest_version(&self) -> Result<Option<String>> {
        self.get(KEY_LATEST_VERSION).await
    }

    pub async fn save_latest_version(&self, version: &str) -> Result<()> {
        self.set(KEY_LATEST_VERSION, version).await
    }

    pub async fn session_token(&self) -> Result<Option<String>> {
        self.get(KEY_SESSION).await
    }

    pub async fn save_session(&self, token: &str) -> Result<()> {
        self.set(KEY_SESSION, token).await
    }

    pub async fn delete_session(&self) -> Result<()> {
        self.delete(KEY_SESSION).await
    }

    pub async fn logged_in(&self) -> Result<bool> {
        Ok(self.session_token().await?.is_some())
    }

    // Hub session methods (separate from sync session, used for Hub-specific features like AI)

    pub async fn hub_session_token(&self) -> Result<Option<String>> {
        self.get(KEY_HUB_SESSION).await
    }

    pub async fn save_hub_session(&self, token: &str) -> Result<()> {
        self.set(KEY_HUB_SESSION, token).await
    }

    pub async fn delete_hub_session(&self) -> Result<()> {
        self.delete(KEY_HUB_SESSION).await
    }

    pub async fn hub_logged_in(&self) -> Result<bool> {
        Ok(self.hub_session_token().await?.is_some())
    }

    // File migration: on first open, migrate old plain-text files into the database.
    // Old files are left in place for safe downgrades.

    async fn migrate_files(&self) -> Result<()> {
        if self.get(KEY_FILES_MIGRATED).await?.is_some() {
            return Ok(());
        }

        let data_dir = crate::settings::Settings::effective_data_dir();

        // host_id — validate as UUID
        let host_id_path = data_dir.join(LEGACY_HOST_ID_FILENAME);
        if host_id_path.exists()
            && let Ok(value) = fs_err::read_to_string(&host_id_path)
        {
            let value = value.trim();
            if !value.is_empty() {
                if Uuid::from_str(value).is_ok() {
                    self.set(KEY_HOST_ID, value).await?;
                } else {
                    warn!("skipping migration of host_id: invalid UUID {value:?}");
                }
            }
        }

        // last_sync_time — validate as RFC3339
        let sync_path = data_dir.join(LEGACY_LAST_SYNC_FILENAME);
        if sync_path.exists()
            && let Ok(value) = fs_err::read_to_string(&sync_path)
        {
            let value = value.trim();
            if !value.is_empty() {
                if OffsetDateTime::parse(value, &Rfc3339).is_ok() {
                    self.set(KEY_LAST_SYNC, value).await?;
                } else {
                    warn!("skipping migration of last_sync_time: invalid RFC3339 {value:?}");
                }
            }
        }

        // last_version_check_time — validate as RFC3339
        let version_check_path = data_dir.join(LEGACY_LAST_VERSION_CHECK_FILENAME);
        if version_check_path.exists()
            && let Ok(value) = fs_err::read_to_string(&version_check_path)
        {
            let value = value.trim();
            if !value.is_empty() {
                if OffsetDateTime::parse(value, &Rfc3339).is_ok() {
                    self.set(KEY_LAST_VERSION_CHECK, value).await?;
                } else {
                    warn!(
                        "skipping migration of last_version_check_time: invalid RFC3339 {value:?}"
                    );
                }
            }
        }

        // latest_version — no strict validation, just non-empty
        let latest_version_path = data_dir.join(LEGACY_LATEST_VERSION_FILENAME);
        if latest_version_path.exists()
            && let Ok(value) = fs_err::read_to_string(&latest_version_path)
        {
            let value = value.trim();
            if !value.is_empty() {
                self.set(KEY_LATEST_VERSION, value).await?;
            }
        }

        // session token — no strict validation, just non-empty
        let session_path = data_dir.join(LEGACY_SESSION_FILENAME);
        if session_path.exists()
            && let Ok(value) = fs_err::read_to_string(&session_path)
        {
            let value = value.trim();
            if !value.is_empty() {
                self.set(KEY_SESSION, value).await?;
            }
        }

        self.set(KEY_FILES_MIGRATED, "true").await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn new_test_store() -> MetaStore {
        MetaStore::new("sqlite::memory:", 2.0).await.unwrap()
    }

    #[tokio::test]
    async fn test_get_set_delete() {
        let store = new_test_store().await;

        assert_eq!(store.get("foo").await.unwrap(), None);

        store.set("foo", "bar").await.unwrap();
        assert_eq!(store.get("foo").await.unwrap(), Some("bar".to_string()));

        store.set("foo", "baz").await.unwrap();
        assert_eq!(store.get("foo").await.unwrap(), Some("baz".to_string()));

        store.delete("foo").await.unwrap();
        assert_eq!(store.get("foo").await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_host_id_generation_and_stability() {
        let store = new_test_store().await;

        let id1 = store.host_id().await.unwrap();
        let id2 = store.host_id().await.unwrap();

        assert_eq!(id1, id2, "host_id should be stable across calls");
    }

    #[tokio::test]
    async fn test_sync_time() {
        let store = new_test_store().await;

        let t = store.last_sync().await.unwrap();
        assert_eq!(t, OffsetDateTime::UNIX_EPOCH);

        store.save_sync_time().await.unwrap();
        let t = store.last_sync().await.unwrap();
        assert!(t > OffsetDateTime::UNIX_EPOCH);
    }

    #[tokio::test]
    async fn test_version_check_time() {
        let store = new_test_store().await;

        let t = store.last_version_check().await.unwrap();
        assert_eq!(t, OffsetDateTime::UNIX_EPOCH);

        store.save_version_check_time().await.unwrap();
        let t = store.last_version_check().await.unwrap();
        assert!(t > OffsetDateTime::UNIX_EPOCH);
    }

    #[tokio::test]
    async fn test_session_crud() {
        let store = new_test_store().await;

        assert!(!store.logged_in().await.unwrap());
        assert_eq!(store.session_token().await.unwrap(), None);

        store.save_session("tok123").await.unwrap();
        assert!(store.logged_in().await.unwrap());
        assert_eq!(
            store.session_token().await.unwrap(),
            Some("tok123".to_string())
        );

        store.delete_session().await.unwrap();
        assert!(!store.logged_in().await.unwrap());
    }

    #[tokio::test]
    async fn test_latest_version() {
        let store = new_test_store().await;

        assert_eq!(store.latest_version().await.unwrap(), None);

        store.save_latest_version("1.2.3").await.unwrap();
        assert_eq!(
            store.latest_version().await.unwrap(),
            Some("1.2.3".to_string())
        );
    }
}
