use std::ffi::OsStr;
use std::time::Duration;

use atuin_common::db;
use atuin_common::db::sqlite::{Sqlite, SqliteBuilder};
use sqlx::Result;
use sqlx::sqlite::SqlitePool;
use tracing::debug;

use crate::store::entry::KvEntry;

const KV_COLUMNS: &str = "namespace, key, value";

#[derive(Debug, Clone)]
pub struct Database {
    sqlite: Sqlite,
}

impl Database {
    pub async fn new(path: impl AsRef<OsStr>, timeout: Duration) -> eyre::Result<Self> {
        let path = path.as_ref();
        debug!("opening KV sqlite database at {:?}", path);

        Self::from_builder(Sqlite::builder(path), timeout).await
    }

    pub async fn in_memory(timeout: Duration) -> eyre::Result<Self> {
        Self::from_builder(Sqlite::builder_in_memory(), timeout).await
    }

    async fn from_builder(builder: SqliteBuilder<'_>, timeout: Duration) -> eyre::Result<Self> {
        let sqlite = builder.timeout(timeout).regexp().open().await?;

        Self::setup_db(sqlite.pool()).await?;

        Ok(Self { sqlite })
    }

    pub async fn sqlite_version(&self) -> eyre::Result<semver::Version> {
        Ok(self.sqlite.info().await.version?)
    }

    async fn setup_db(pool: &SqlitePool) -> Result<()> {
        debug!("running sqlite database setup");

        db::migrate!(pool, "./migrations").await?;

        Ok(())
    }

    async fn save_raw(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, e: &KvEntry) -> Result<()> {
        db::query(
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
        db::query("delete from kv where namespace = ?1 and key = ?2")
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

    pub async fn load(&self, namespace: &str, key: &str) -> Result<Option<KvEntry>> {
        debug!("loading kv entry {namespace}.{key}");

        let res = db::query_as::<_, KvEntry>(sqlx::AssertSqlSafe(format!(
            "select {KV_COLUMNS} from kv where namespace = ?1 and key = ?2"
        )))
        .bind(namespace)
        .bind(key)
        .fetch_optional(self.sqlite.pool())
        .await?;

        Ok(res)
    }

    pub async fn list(&self, namespace: Option<&str>) -> Result<Vec<KvEntry>> {
        debug!("listing kv entries");

        let res = if let Some(namespace) = namespace {
            db::query_as::<_, KvEntry>(sqlx::AssertSqlSafe(format!(
                "select {KV_COLUMNS} from kv where namespace = ?1 order by key asc"
            )))
            .bind(namespace)
            .fetch_all(self.sqlite.pool())
            .await?
        } else {
            db::query_as::<_, KvEntry>(sqlx::AssertSqlSafe(format!(
                "select {KV_COLUMNS} from kv order by namespace, key asc"
            )))
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
        KvEntry {
            namespace: "test".to_string(),
            key: "test".to_string(),
            value: "test".to_string(),
        }
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
