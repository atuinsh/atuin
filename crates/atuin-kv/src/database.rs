use std::path::Path;
use std::time::Duration;

use atuin_common::sqlite::Sqlite;
use sqlx::sqlite::{SqlitePool, SqliteRow};
use sqlx::{Result, Row};
use tracing::debug;

use crate::store::entry::KvEntry;

#[derive(Debug, Clone)]
pub struct Database {
    sqlite: Sqlite,
}

impl Database {
    pub async fn new(path: impl AsRef<Path>, timeout: Duration) -> Result<Self> {
        let path = path.as_ref();
        debug!("opening KV sqlite database at {:?}", path);

        let sqlite = Sqlite::builder()
            .file(path)
            .timeout(timeout)
            .open()
            .await
            .map_err(|e| sqlx::Error::Configuration(Box::new(e)))?;

        Self::setup_db(sqlite.pool()).await?;
        Ok(Self { sqlite })
    }

    pub async fn in_memory(timeout: Duration) -> Result<Self> {
        let sqlite = Sqlite::builder()
            .memory()
            .timeout(timeout)
            .open()
            .await
            .map_err(|e| sqlx::Error::Configuration(Box::new(e)))?;

        Self::setup_db(sqlite.pool()).await?;
        Ok(Self { sqlite })
    }

    pub async fn sqlite_version(&self) -> Result<String> {
        sqlx::query_scalar("SELECT sqlite_version()").fetch_one(self.sqlite.pool()).await
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
        let mut tx = self.sqlite.pool().begin().await?;
        Self::save_raw(&mut tx, e).await?;
        tx.commit().await?;

        Ok(())
    }

    pub async fn delete(&self, namespace: &str, key: &str) -> Result<()> {
        debug!("deleting kv entry {namespace}/{key}");

        let mut tx = self.sqlite.pool().begin().await?;
        Self::delete_raw(&mut tx, namespace, key).await?;
        tx.commit().await?;

        Ok(())
    }

    fn query_kv_entry(row: &SqliteRow) -> KvEntry {
        let namespace = row.get("namespace");
        let key = row.get("key");
        let value = row.get("value");

        KvEntry::builder().namespace(namespace).key(key).value(value).build()
    }

    pub async fn load(&self, namespace: &str, key: &str) -> Result<Option<KvEntry>> {
        debug!("loading kv entry {namespace}.{key}");

        let res = sqlx::query("select * from kv where namespace = ?1 and key = ?2")
            .bind(namespace)
            .bind(key)
            .map(|row| Self::query_kv_entry(&row))
            .fetch_optional(self.sqlite.pool())
            .await?;

        Ok(res)
    }

    pub async fn list(&self, namespace: Option<&str>) -> Result<Vec<KvEntry>> {
        debug!("listing kv entries");

        let res = if let Some(namespace) = namespace {
            sqlx::query("select * from kv where namespace = ?1 order by key asc")
                .bind(namespace)
                .map(|row| Self::query_kv_entry(&row))
                .fetch_all(self.sqlite.pool())
                .await?
        } else {
            sqlx::query("select * from kv order by namespace, key asc")
                .map(|row| Self::query_kv_entry(&row))
                .fetch_all(self.sqlite.pool())
                .await?
        };

        Ok(res)
    }
}

#[cfg(test)]
mod test {
    use rstest::*;

    use super::*;

    #[fixture]
    async fn db() -> Database {
        Database::in_memory(Duration::from_secs(1)).await.unwrap()
    }

    #[fixture]
    fn entry() -> KvEntry {
        KvEntry::builder()
            .namespace("test".to_string())
            .key("test".to_string())
            .value("test".to_string())
            .build()
    }

    #[rstest]
    #[tokio::test]
    async fn test_list(#[future] db: Database, entry: KvEntry) {
        let db = db.await;

        let scripts = db.list(None).await.unwrap();
        assert_eq!(scripts.len(), 0);

        db.save(&entry).await.unwrap();

        let entries = db.list(None).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].namespace, "test");
        assert_eq!(entries[0].key, "test");
        assert_eq!(entries[0].value, "test");
    }

    #[rstest]
    #[tokio::test]
    async fn test_save_load(#[future] db: Database, entry: KvEntry) {
        let db = db.await;

        db.save(&entry).await.unwrap();

        let loaded = db.load(&entry.namespace, &entry.key).await.unwrap().unwrap();

        assert_eq!(loaded, entry);
    }

    #[rstest]
    #[tokio::test]
    async fn test_delete(#[future] db: Database, entry: KvEntry) {
        let db = db.await;

        db.save(&entry).await.unwrap();

        assert_eq!(db.list(None).await.unwrap().len(), 1);
        db.delete(&entry.namespace, &entry.key).await.unwrap();

        let loaded = db.list(None).await.unwrap();
        assert_eq!(loaded.len(), 0);
    }
}
