use std::path::Path;
use std::time::Duration;

use atuin_common::sqlite::{Sqlite, TableView};
use atuin_common::table;
use sqlx::Result;
use tracing::debug;

use crate::store::entry::KvEntry;

table!(KvEntry {
    name: "kv",
    key: ["namespace", "key"],
    conflict: upsert,
    columns: {
        namespace => |e| e.namespace.as_str(),
        key       => |e| e.key.as_str(),
        value     => |e| e.value.as_str(),
    },
});

#[derive(Debug, Clone)]
pub struct Database {
    sqlite: Sqlite,
    table: TableView<KvEntry>,
}

impl Database {
    pub async fn new(path: impl AsRef<Path>, timeout: Duration) -> eyre::Result<Self> {
        let path = path.as_ref();
        debug!("opening KV sqlite database at {:?}", path);

        let sqlite = Sqlite::builder()
            .file(path)
            .timeout(timeout)
            .with_migrations(sqlx::migrate!("./migrations"))
            .open()
            .await?;

        let table = TableView::new(sqlite.clone());
        Ok(Self { sqlite, table })
    }

    pub async fn in_memory(timeout: Duration) -> eyre::Result<Self> {
        let sqlite = Sqlite::builder()
            .memory()
            .timeout(timeout)
            .with_migrations(sqlx::migrate!("./migrations"))
            .open()
            .await?;

        let table = TableView::new(sqlite.clone());
        Ok(Self { sqlite, table })
    }

    pub async fn save(&self, e: &KvEntry) -> Result<()> {
        debug!("saving kv entry to sqlite");
        self.table.insert_one(e).await
    }

    pub async fn delete(&self, namespace: &str, key: &str) -> Result<()> {
        debug!("deleting kv entry {namespace}/{key}");
        self.table.delete((namespace, key)).await
    }

    pub async fn load(&self, namespace: &str, key: &str) -> Result<Option<KvEntry>> {
        debug!("loading kv entry {namespace}.{key}");
        self.table.get((namespace, key)).await
    }

    pub async fn list(&self, namespace: Option<&str>) -> Result<Vec<KvEntry>> {
        debug!("listing kv entries");

        let res = if let Some(namespace) = namespace {
            sqlx::query_as::<_, KvEntry>("select * from kv where namespace = ?1 order by key asc")
                .bind(namespace)
                .fetch_all(self.sqlite.pool())
                .await?
        } else {
            sqlx::query_as::<_, KvEntry>("select * from kv order by namespace, key asc")
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
            namespace: "test".into(),
            key: "test".into(),
            value: "test".into(),
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

    #[tokio::test]
    async fn tableview_roundtrip() {
        let db = Database::in_memory(Duration::from_secs(1)).await.unwrap();

        let entry = KvEntry {
            namespace: "n".into(),
            key: "k".into(),
            value: "v".into(),
        };

        db.save(&entry).await.unwrap();
        assert_eq!(db.load("n", "k").await.unwrap().unwrap().value, "v");
        db.delete("n", "k").await.unwrap();
        assert!(db.load("n", "k").await.unwrap().is_none());
    }
}
