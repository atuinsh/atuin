use std::{path::Path, str::FromStr, time::Duration};

use atuin_common::utils;
use sqlx::{
    Result, Row,
    sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow,
        SqliteSynchronous,
    },
};
use tokio::fs;
use tracing::debug;

use crate::store::entry::KvEntry;

#[derive(Debug, Clone)]
pub struct Database {
    pub pool: SqlitePool,
}

impl Database {
    pub async fn new(path: impl AsRef<Path>, timeout: f64) -> Result<Self> {
        let path = path.as_ref();
        debug!("opening KV sqlite database at {:?}", path);

        if utils::broken_symlink(path) {
            eprintln!(
                "Atuin: KV sqlite db path ({path:?}) is a broken symlink. Unable to read or create replacement."
            );
            std::process::exit(1);
        }

        if !path.exists()
            && let Some(dir) = path.parent()
        {
            fs::create_dir_all(dir).await?;
        }

        let opts = SqliteConnectOptions::from_str(path.as_os_str().to_str().unwrap())?
            .journal_mode(SqliteJournalMode::Wal)
            .optimize_on_close(true, None)
            .synchronous(SqliteSynchronous::Normal)
            .with_regexp()
            .foreign_keys(true)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .acquire_timeout(Duration::from_secs_f64(timeout))
            .connect_with(opts)
            .await?;

        Self::setup_db(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn sqlite_version(&self) -> Result<String> {
        sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(&self.pool)
            .await
    }

    async fn setup_db(pool: &SqlitePool) -> Result<()> {
        debug!("running sqlite database setup");

        sqlx::migrate!("./migrations").run(pool).await?;

        Ok(())
    }

    async fn save_raw(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, e: &KvEntry) -> Result<()> {
        sqlx::query(
            "insert into kv(namespace, key, value)
                values(?1, ?2, ?3)
                on conflict(namespace, key) do update set
                    namespace = excluded.namespace,
                    key = excluded.key,
                    value = excluded.value",
        )
        .bind(e.namespace.as_str())
        .bind(e.key.as_str())
        .bind(e.value.as_str())
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    async fn delete_raw(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        namespace: &str,
        key: &str,
    ) -> Result<()> {
        sqlx::query("delete from kv where namespace = ?1 and key = ?2")
            .bind(namespace)
            .bind(key)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    pub async fn save(&self, e: &KvEntry) -> Result<()> {
        debug!("saving kv entry to sqlite");
        let mut tx = self.pool.begin().await?;
        Self::save_raw(&mut tx, e).await?;
        tx.commit().await?;

        Ok(())
    }

    pub async fn delete(&self, namespace: &str, key: &str) -> Result<()> {
        debug!("deleting kv entry {namespace}/{key}");

        let mut tx = self.pool.begin().await?;
        Self::delete_raw(&mut tx, namespace, key).await?;
        tx.commit().await?;

        Ok(())
    }

    fn query_kv_entry(row: SqliteRow) -> KvEntry {
        let namespace = row.get("namespace");
        let key = row.get("key");
        let value = row.get("value");

        KvEntry::builder()
            .namespace(namespace)
            .key(key)
            .value(value)
            .build()
    }

    pub async fn load(&self, namespace: &str, key: &str) -> Result<Option<KvEntry>> {
        debug!("loading kv entry {namespace}.{key}");

        let res = sqlx::query("select * from kv where namespace = ?1 and key = ?2")
            .bind(namespace)
            .bind(key)
            .map(Self::query_kv_entry)
            .fetch_optional(&self.pool)
            .await?;

        Ok(res)
    }

    pub async fn list(&self, namespace: Option<&str>) -> Result<Vec<KvEntry>> {
        debug!("listing kv entries");

        let res = if let Some(namespace) = namespace {
            sqlx::query("select * from kv where namespace = ?1 order by key asc")
                .bind(namespace)
                .map(Self::query_kv_entry)
                .fetch_all(&self.pool)
                .await?
        } else {
            sqlx::query("select * from kv order by namespace, key asc")
                .map(Self::query_kv_entry)
                .fetch_all(&self.pool)
                .await?
        };

        Ok(res)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn test_list() {
        let db = Database::new("sqlite::memory:", 1.0).await.unwrap();
        let scripts = db.list(None).await.unwrap();
        assert_eq!(scripts.len(), 0);

        let entry = KvEntry::builder()
            .namespace("test".to_string())
            .key("test".to_string())
            .value("test".to_string())
            .build();

        db.save(&entry).await.unwrap();

        let entries = db.list(None).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].namespace, "test");
        assert_eq!(entries[0].key, "test");
        assert_eq!(entries[0].value, "test");
    }

    #[tokio::test]
    async fn test_save_load() {
        let db = Database::new("sqlite::memory:", 1.0).await.unwrap();

        let entry = KvEntry::builder()
            .namespace("test".to_string())
            .key("test".to_string())
            .value("test".to_string())
            .build();

        db.save(&entry).await.unwrap();

        let loaded = db
            .load(&entry.namespace, &entry.key)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(loaded, entry);
    }

    #[tokio::test]
    async fn test_delete() {
        let db = Database::new("sqlite::memory:", 1.0).await.unwrap();

        let entry = KvEntry::builder()
            .namespace("test".to_string())
            .key("test".to_string())
            .value("test".to_string())
            .build();

        db.save(&entry).await.unwrap();

        assert_eq!(db.list(None).await.unwrap().len(), 1);
        db.delete(&entry.namespace, &entry.key).await.unwrap();

        let loaded = db.list(None).await.unwrap();
        assert_eq!(loaded.len(), 0);
    }
}
