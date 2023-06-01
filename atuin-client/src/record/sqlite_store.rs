// Here we are using sqlite as a pretty dumb store, and will not be running any complex queries.
// Multiple stores of multiple types are all stored in one chonky table (for now), and we just index
// by tag/host

use std::path::Path;
use std::str::FromStr;

use async_trait::async_trait;
use eyre::Result;
use fs_err as fs;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow},
    Row,
};

use atuin_common::record::Record;

use super::store::Store;

pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    pub async fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        debug!("opening sqlite database at {:?}", path);

        let create = !path.exists();
        if create {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
        }

        let opts = SqliteConnectOptions::from_str(path.as_os_str().to_str().unwrap())?
            .journal_mode(SqliteJournalMode::Wal)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new().connect_with(opts).await?;

        Self::setup_db(&pool).await?;

        Ok(Self { pool })
    }

    async fn setup_db(pool: &SqlitePool) -> Result<()> {
        debug!("running sqlite database setup");

        sqlx::migrate!("./record-migrations").run(pool).await?;

        Ok(())
    }

    async fn save_raw(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, r: &Record) -> Result<()> {
        // In sqlite, we are "limited" to i64. But that is still fine, until 2262.
        sqlx::query(
            "insert or ignore into records(id, host, timestamp, tag, version, data)
                values(?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(r.id.as_str())
        .bind(r.host.as_str())
        .bind(r.timestamp as i64)
        .bind(r.tag.as_str())
        .bind(r.version.as_str())
        .bind(r.data.as_slice())
        .execute(tx)
        .await?;

        Ok(())
    }

    fn query_row(row: SqliteRow) -> Record {
        let timestamp: i64 = row.get("timestamp");

        Record {
            id: row.get("id"),
            host: row.get("host"),
            timestamp: timestamp as u64,
            tag: row.get("tag"),
            version: row.get("version"),
            data: row.get("data"),
        }
    }
}

#[async_trait]
impl Store for SqliteStore {
    async fn push(&self, record: Record) -> Result<Record> {
        // TODO: batch inserts
        let mut tx = self.pool.begin().await?;
        Self::save_raw(&mut tx, &record).await?;
        tx.commit().await?;

        Ok(record)
    }

    async fn get(&self, id: &str) -> Result<Record> {
        println!("querying {}", id);
        let res = sqlx::query("select * from records where id = ?1")
            .bind(id)
            .map(Self::query_row)
            .fetch_one(&self.pool)
            .await?;

        Ok(res)
    }

    async fn len(&self, host: &str, tag: &str) -> Result<u64> {
        let res: (i64,) =
            sqlx::query_as("select count(1) from records where host = ?1 and tag = ?2")
                .bind(host)
                .bind(tag)
                .fetch_one(&self.pool)
                .await?;

        Ok(res.0 as u64)
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::record::Record;

    use crate::record::store::Store;

    use super::SqliteStore;

    fn test_record() -> Record {
        Record::new(
            String::from(atuin_common::utils::uuid_v7().simple().to_string()),
            String::from("v1"),
            String::from(atuin_common::utils::uuid_v7().simple().to_string()),
            vec![0, 1, 2, 3],
        )
    }

    #[tokio::test]
    async fn create_db() {
        let db = SqliteStore::new(":memory:").await;

        assert!(
            db.is_ok(),
            "db could not be created, {:?}",
            db.err().unwrap()
        );
    }

    #[tokio::test]
    async fn push_record() {
        let db = SqliteStore::new(":memory:").await.unwrap();
        let record = test_record();

        let record = db.push(record).await;

        assert!(
            record.is_ok(),
            "failed to insert record: {:?}",
            record.unwrap_err()
        );
    }

    #[tokio::test]
    async fn get_record() {
        let db = SqliteStore::new(":memory:").await.unwrap();
        let record = test_record();
        let record = db.push(record).await.unwrap();

        let new_record = db.get(record.id.as_str()).await;

        assert!(
            new_record.is_ok(),
            "failed to fetch record: {:?}",
            new_record.unwrap_err()
        );

        assert_eq!(record, new_record.unwrap(), "records are not equal");
    }

    #[tokio::test]
    async fn len() {
        let db = SqliteStore::new(":memory:").await.unwrap();
        let record = test_record();
        let record = db.push(record).await.unwrap();

        let len = db.len(record.host.as_str(), record.tag.as_str()).await;

        assert!(
            len.is_ok(),
            "failed to get store len: {:?}",
            len.unwrap_err()
        );

        assert_eq!(len.unwrap(), 1, "expected length of 1 after insert");
    }

    #[tokio::test]
    async fn len_different_tags() {
        let db = SqliteStore::new(":memory:").await.unwrap();

        // these have different tags, so the len should be the same
        // we model multiple stores within one database
        // new store = new tag = independent length
        let first = db.push(test_record()).await.unwrap();
        let second = db.push(test_record()).await.unwrap();

        let first_len = db
            .len(first.host.as_str(), first.tag.as_str())
            .await
            .unwrap();
        let second_len = db
            .len(second.host.as_str(), second.tag.as_str())
            .await
            .unwrap();

        assert_eq!(first_len, 1, "expected length of 1 after insert");
        assert_eq!(second_len, 1, "expected length of 1 after insert");
    }
}
