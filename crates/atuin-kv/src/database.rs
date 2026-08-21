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

    fn kv(namespace: &str, key: &str, value: &str) -> KvEntry {
        KvEntry {
            namespace: namespace.into(),
            key: key.into(),
            value: value.into(),
        }
    }

    /// The only thing this layer adds over `TableView` (exhaustively tested in
    /// `atuin-common`) is the wiring, which those tests can't reach: the real
    /// migration DDL must match the `table!(KvEntry)` schema, `FromRow` must map
    /// each column correctly, and the wrapper must pass the composite
    /// `(namespace, key)` in the right order. Distinct per-column values catch a
    /// column/`FromRow` drift; two entries sharing a namespace exercise the
    /// composite key and `list` filtering.
    #[rstest]
    #[tokio::test]
    async fn kventry_roundtrips_through_real_schema(#[future(awt)] db: Database) {
        db.save(&kv("ns", "a", "va")).await.unwrap();
        db.save(&kv("ns", "b", "vb")).await.unwrap();
        db.save(&kv("other", "a", "vo")).await.unwrap();

        // The full composite key disambiguates same-key/different-namespace rows.
        assert_eq!(db.load("ns", "a").await.unwrap().unwrap(), kv("ns", "a", "va"));

        // `list(namespace)` filters to one namespace, ordered by key.
        let ns: Vec<KvEntry> = db.list("ns").try_collect().await.unwrap();
        assert_eq!(ns.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), ["a", "b"]);

        // `list_all` streams every namespace.
        assert_eq!(db.list_all().try_collect::<Vec<_>>().await.unwrap().len(), 3);

        db.delete("ns", "a").await.unwrap();
        assert!(db.load("ns", "a").await.unwrap().is_none());
        assert_eq!(db.list_all().try_collect::<Vec<_>>().await.unwrap().len(), 2);
    }
}
