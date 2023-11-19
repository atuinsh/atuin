// Here we are using sqlite as a pretty dumb store, and will not be running any complex queries.
// Multiple stores of multiple types are all stored in one chonky table (for now), and we just index
// by tag/host

use std::path::Path;
use std::str::FromStr;

use async_trait::async_trait;
use eyre::{eyre, Result};
use fs_err as fs;
use futures::TryStreamExt;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow},
    Row,
};

use atuin_common::record::{EncryptedData, HostId, Record, RecordId, RecordIndex};
use uuid::Uuid;

use super::store::Store;

#[derive(Debug)]
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

    async fn save_raw(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        r: &Record<EncryptedData>,
    ) -> Result<()> {
        // In sqlite, we are "limited" to i64. But that is still fine, until 2262.
        sqlx::query(
            "insert or ignore into records(id, host, tag, timestamp, parent, version, data, cek)
                values(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(r.id.0.as_simple().to_string())
        .bind(r.host.0.as_simple().to_string())
        .bind(r.tag.as_str())
        .bind(r.timestamp as i64)
        .bind(r.parent.map(|p| p.0.as_simple().to_string()))
        .bind(r.version.as_str())
        .bind(r.data.data.as_str())
        .bind(r.data.content_encryption_key.as_str())
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    fn query_row(row: SqliteRow) -> Record<EncryptedData> {
        let timestamp: i64 = row.get("timestamp");

        // tbh at this point things are pretty fucked so just panic
        let id = Uuid::from_str(row.get("id")).expect("invalid id UUID format in sqlite DB");
        let host = Uuid::from_str(row.get("host")).expect("invalid host UUID format in sqlite DB");
        let parent: Option<&str> = row.get("parent");

        let parent = parent
            .map(|parent| Uuid::from_str(parent).expect("invalid parent UUID format in sqlite DB"));

        Record {
            id: RecordId(id),
            host: HostId(host),
            parent: parent.map(RecordId),
            timestamp: timestamp as u64,
            tag: row.get("tag"),
            version: row.get("version"),
            data: EncryptedData {
                data: row.get("data"),
                content_encryption_key: row.get("cek"),
            },
        }
    }
}

#[async_trait]
impl Store for SqliteStore {
    async fn push_batch(
        &self,
        records: impl Iterator<Item = &Record<EncryptedData>> + Send + Sync,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        for record in records {
            Self::save_raw(&mut tx, record).await?;
        }

        tx.commit().await?;

        Ok(())
    }

    async fn get(&self, id: RecordId) -> Result<Record<EncryptedData>> {
        let res = sqlx::query("select * from records where id = ?1")
            .bind(id.0.as_simple().to_string())
            .map(Self::query_row)
            .fetch_one(&self.pool)
            .await?;

        Ok(res)
    }

    async fn len(&self, host: HostId, tag: &str) -> Result<u64> {
        let res: (i64,) =
            sqlx::query_as("select count(1) from records where host = ?1 and tag = ?2")
                .bind(host.0.as_simple().to_string())
                .bind(tag)
                .fetch_one(&self.pool)
                .await?;

        Ok(res.0 as u64)
    }

    async fn next(&self, record: &Record<EncryptedData>) -> Result<Option<Record<EncryptedData>>> {
        let res = sqlx::query("select * from records where parent = ?1")
            .bind(record.id.0.as_simple().to_string())
            .map(Self::query_row)
            .fetch_one(&self.pool)
            .await;

        match res {
            Err(sqlx::Error::RowNotFound) => Ok(None),
            Err(e) => Err(eyre!("an error occured: {}", e)),
            Ok(v) => Ok(Some(v)),
        }
    }

    async fn head(&self, host: HostId, tag: &str) -> Result<Option<Record<EncryptedData>>> {
        let res = sqlx::query(
            "select * from records where host = ?1 and tag = ?2 and parent is null limit 1",
        )
        .bind(host.0.as_simple().to_string())
        .bind(tag)
        .map(Self::query_row)
        .fetch_optional(&self.pool)
        .await?;

        Ok(res)
    }

    async fn tail(&self, host: HostId, tag: &str) -> Result<Option<Record<EncryptedData>>> {
        let res = sqlx::query(
            "select * from records rp where tag=?1 and host=?2 and (select count(1) from records where parent=rp.id) = 0;",
        )
        .bind(tag)
        .bind(host.0.as_simple().to_string())
        .map(Self::query_row)
        .fetch_optional(&self.pool)
        .await?;

        Ok(res)
    }

    async fn tag_tails(&self, tag: &str) -> Result<Vec<Record<EncryptedData>>> {
        let res = sqlx::query(
            "select * from records rp where tag=?1 and (select count(1) from records where parent=rp.id) = 0;",
        )
        .bind(tag)
        .map(Self::query_row)
        .fetch_all(&self.pool)
        .await?;

        Ok(res)
    }

    async fn tail_records(&self) -> Result<RecordIndex> {
        let res = sqlx::query(
            "select host, tag, id from records rp where (select count(1) from records where parent=rp.id) = 0;",
        )
        .map(|row: SqliteRow| {
            let host: Uuid= Uuid::from_str(row.get("host")).expect("invalid uuid in db host");
            let tag: String= row.get("tag");
            let id: Uuid= Uuid::from_str(row.get("id")).expect("invalid uuid in db id");

            (HostId(host), tag, RecordId(id))
        })
        .fetch(&self.pool)
        .try_collect()
        .await?;

        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::record::{EncryptedData, HostId, Record};

    use crate::record::{encryption::PASETO_V4, store::Store};

    use super::SqliteStore;

    fn test_record() -> Record<EncryptedData> {
        Record::builder()
            .host(HostId(atuin_common::utils::uuid_v7()))
            .version("v1".into())
            .tag(atuin_common::utils::uuid_v7().simple().to_string())
            .data(EncryptedData {
                data: "1234".into(),
                content_encryption_key: "1234".into(),
            })
            .build()
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

        db.push(&record).await.expect("failed to insert record");
    }

    #[tokio::test]
    async fn get_record() {
        let db = SqliteStore::new(":memory:").await.unwrap();
        let record = test_record();
        db.push(&record).await.unwrap();

        let new_record = db.get(record.id).await.expect("failed to fetch record");

        assert_eq!(record, new_record, "records are not equal");
    }

    #[tokio::test]
    async fn len() {
        let db = SqliteStore::new(":memory:").await.unwrap();
        let record = test_record();
        db.push(&record).await.unwrap();

        let len = db
            .len(record.host, record.tag.as_str())
            .await
            .expect("failed to get store len");

        assert_eq!(len, 1, "expected length of 1 after insert");
    }

    #[tokio::test]
    async fn len_different_tags() {
        let db = SqliteStore::new(":memory:").await.unwrap();

        // these have different tags, so the len should be the same
        // we model multiple stores within one database
        // new store = new tag = independent length
        let first = test_record();
        let second = test_record();

        db.push(&first).await.unwrap();
        db.push(&second).await.unwrap();

        let first_len = db.len(first.host, first.tag.as_str()).await.unwrap();
        let second_len = db.len(second.host, second.tag.as_str()).await.unwrap();

        assert_eq!(first_len, 1, "expected length of 1 after insert");
        assert_eq!(second_len, 1, "expected length of 1 after insert");
    }

    #[tokio::test]
    async fn append_a_bunch() {
        let db = SqliteStore::new(":memory:").await.unwrap();

        let mut tail = test_record();
        db.push(&tail).await.expect("failed to push record");

        for _ in 1..100 {
            tail = tail
                .new_child(vec![1, 2, 3, 4])
                .encrypt::<PASETO_V4>(&[0; 32]);
            db.push(&tail).await.unwrap();
        }

        assert_eq!(
            db.len(tail.host, tail.tag.as_str()).await.unwrap(),
            100,
            "failed to insert 100 records"
        );
    }

    #[tokio::test]
    async fn append_a_big_bunch() {
        let db = SqliteStore::new(":memory:").await.unwrap();

        let mut records: Vec<Record<EncryptedData>> = Vec::with_capacity(10000);

        let mut tail = test_record();
        records.push(tail.clone());

        for _ in 1..10000 {
            tail = tail.new_child(vec![1, 2, 3]).encrypt::<PASETO_V4>(&[0; 32]);
            records.push(tail.clone());
        }

        db.push_batch(records.iter()).await.unwrap();

        assert_eq!(
            db.len(tail.host, tail.tag.as_str()).await.unwrap(),
            10000,
            "failed to insert 10k records"
        );
    }

    #[tokio::test]
    async fn test_chain() {
        let db = SqliteStore::new(":memory:").await.unwrap();

        let mut records: Vec<Record<EncryptedData>> = Vec::with_capacity(1000);

        let mut tail = test_record();
        records.push(tail.clone());

        for _ in 1..1000 {
            tail = tail.new_child(vec![1, 2, 3]).encrypt::<PASETO_V4>(&[0; 32]);
            records.push(tail.clone());
        }

        db.push_batch(records.iter()).await.unwrap();

        let mut record = db
            .head(tail.host, tail.tag.as_str())
            .await
            .expect("in memory sqlite should not fail")
            .expect("entry exists");

        let mut count = 1;

        while let Some(next) = db.next(&record).await.unwrap() {
            assert_eq!(record.id, next.clone().parent.unwrap());
            record = next;

            count += 1;
        }

        assert_eq!(count, 1000);
    }
}
