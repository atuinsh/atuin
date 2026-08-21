use std::path::Path;
use std::time::Duration;

use atuin_common::sqlite::{Sqlite, TableView};
use atuin_common::table;
use sqlx::Result;

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
    table: TableView<KvEntry>,
}

impl Database {
    pub async fn new(path: impl AsRef<Path>, timeout: Duration) -> eyre::Result<Self> {
        let sqlite = Sqlite::builder()
            .file(path)
            .timeout(timeout)
            .with_migrations(sqlx::migrate!("./migrations"))
            .open()
            .await?;

        let table = TableView::new(sqlite);
        Ok(Self { table })
    }

    pub async fn in_memory(timeout: Duration) -> eyre::Result<Self> {
        let sqlite = Sqlite::builder()
            .memory()
            .timeout(timeout)
            .with_migrations(sqlx::migrate!("./migrations"))
            .open()
            .await?;

        let table = TableView::new(sqlite);
        Ok(Self { table })
    }

    pub async fn save(&self, e: &KvEntry) -> Result<()> {
        self.table.insert_one(e).await
    }

    pub async fn delete(&self, namespace: &str, key: &str) -> Result<()> {
        self.table.delete((namespace, key)).await
    }

    pub async fn load(&self, namespace: &str, key: &str) -> Result<Option<KvEntry>> {
        self.table.get((namespace, key)).await
    }

    /// Stream the entries in a single namespace, ordered by key.
    pub fn list<'a>(
        &'a self,
        namespace: &'a str,
    ) -> impl futures::Stream<Item = Result<KvEntry>> + Send + 'a {
        self.table.filter(namespace)
    }

    /// Stream every entry, ordered by namespace then key.
    pub fn list_all(&self) -> impl futures::Stream<Item = Result<KvEntry>> + Send + '_ {
        self.table.all_ordered()
    }
}

#[cfg(test)]
mod test {
    use futures::TryStreamExt;
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

        let scripts: Vec<KvEntry> = db.list_all().try_collect().await.unwrap();
        assert_eq!(scripts.len(), 0);

        db.save(&entry).await.unwrap();

        let entries: Vec<KvEntry> = db.list_all().try_collect().await.unwrap();
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

        assert_eq!(db.list_all().try_collect::<Vec<_>>().await.unwrap().len(), 1);
        db.delete(&entry.namespace, &entry.key).await.unwrap();

        let loaded: Vec<KvEntry> = db.list_all().try_collect().await.unwrap();
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
