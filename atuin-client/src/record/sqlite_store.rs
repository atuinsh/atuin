// Here we are using sqlite as a pretty dumb store, and will not be running any complex queries.
// Multiple stores of multiple types are all stored in one chonky table (for now), and we just index
// by tag/host

use std::path::Path;
use std::str::FromStr;

use async_trait::async_trait;
use eyre::{eyre, Result};
use fs_err as fs;

use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow},
    Row,
};

use atuin_common::record::{
    EncryptedData, Host, HostId, Record, RecordId, RecordIdx, RecordStatus,
};
use uuid::Uuid;

use super::store::Store;

#[derive(Debug, Clone)]
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
            .foreign_keys(true)
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
            "insert or ignore into store(id, idx, host, tag, timestamp, version, data, cek)
                values(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(r.id.0.as_hyphenated().to_string())
        .bind(r.idx as i64)
        .bind(r.host.id.0.as_hyphenated().to_string())
        .bind(r.tag.as_str())
        .bind(r.timestamp as i64)
        .bind(r.version.as_str())
        .bind(r.data.data.as_str())
        .bind(r.data.content_encryption_key.as_str())
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    fn query_row(row: SqliteRow) -> Record<EncryptedData> {
        let idx: i64 = row.get("idx");
        let timestamp: i64 = row.get("timestamp");

        // tbh at this point things are pretty fucked so just panic
        let id = Uuid::from_str(row.get("id")).expect("invalid id UUID format in sqlite DB");
        let host = Uuid::from_str(row.get("host")).expect("invalid host UUID format in sqlite DB");

        Record {
            id: RecordId(id),
            idx: idx as u64,
            host: Host::new(HostId(host)),
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
        let res = sqlx::query("select * from store where store.id = ?1")
            .bind(id.0.as_hyphenated().to_string())
            .map(Self::query_row)
            .fetch_one(&self.pool)
            .await?;

        Ok(res)
    }

    async fn last(&self, host: HostId, tag: &str) -> Result<Option<Record<EncryptedData>>> {
        let res =
            sqlx::query("select * from store where host=?1 and tag=?2 order by idx desc limit 1")
                .bind(host.0.as_hyphenated().to_string())
                .bind(tag)
                .map(Self::query_row)
                .fetch_one(&self.pool)
                .await;

        match res {
            Err(sqlx::Error::RowNotFound) => Ok(None),
            Err(e) => Err(eyre!("an error occured: {}", e)),
            Ok(record) => Ok(Some(record)),
        }
    }

    async fn first(&self, host: HostId, tag: &str) -> Result<Option<Record<EncryptedData>>> {
        self.idx(host, tag, 0).await
    }

    async fn len(&self, host: HostId, tag: &str) -> Result<u64> {
        let last = self.last(host, tag).await?;

        if let Some(last) = last {
            return Ok(last.idx + 1);
        }

        return Ok(0);
    }

    async fn next(
        &self,
        host: HostId,
        tag: &str,
        idx: RecordIdx,
        limit: u64,
    ) -> Result<Vec<Record<EncryptedData>>> {
        let res =
            sqlx::query("select * from store where idx >= ?1 and host = ?2 and tag = ?3 limit ?4")
                .bind(idx as i64)
                .bind(host.0.as_hyphenated().to_string())
                .bind(tag)
                .bind(limit as i64)
                .map(Self::query_row)
                .fetch_all(&self.pool)
                .await?;

        Ok(res)
    }

    async fn idx(
        &self,
        host: HostId,
        tag: &str,
        idx: RecordIdx,
    ) -> Result<Option<Record<EncryptedData>>> {
        let res = sqlx::query("select * from store where idx = ?1 and host = ?2 and tag = ?3")
            .bind(idx as i64)
            .bind(host.0.as_hyphenated().to_string())
            .bind(tag)
            .map(Self::query_row)
            .fetch_one(&self.pool)
            .await;

        match res {
            Err(sqlx::Error::RowNotFound) => Ok(None),
            Err(e) => Err(eyre!("an error occured: {}", e)),
            Ok(v) => Ok(Some(v)),
        }
    }

    async fn status(&self) -> Result<RecordStatus> {
        let mut status = RecordStatus::new();

        let res: Result<Vec<(String, String, i64)>, sqlx::Error> =
            sqlx::query_as("select host, tag, max(idx) from store group by host, tag")
                .fetch_all(&self.pool)
                .await;

        let res = match res {
            Err(e) => return Err(eyre!("failed to fetch local store status: {}", e)),
            Ok(v) => v,
        };

        for i in res {
            let host = HostId(
                Uuid::from_str(i.0.as_str()).expect("failed to parse uuid for local store status"),
            );

            status.set_raw(host, i.1, i.2 as u64);
        }

        Ok(status)
    }

    async fn all_tagged(&self, tag: &str) -> Result<Vec<Record<EncryptedData>>> {
        let res = sqlx::query("select * from store where tag = ?1 order by timestamp asc")
            .bind(tag)
            .map(Self::query_row)
            .fetch_all(&self.pool)
            .await?;

        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::record::{EncryptedData, Host, HostId, Record};

    use crate::record::{encryption::PASETO_V4, store::Store};

    use super::SqliteStore;

    fn test_record() -> Record<EncryptedData> {
        Record::builder()
            .host(Host::new(HostId(atuin_common::utils::uuid_v7())))
            .version("v1".into())
            .tag(atuin_common::utils::uuid_v7().simple().to_string())
            .data(EncryptedData {
                data: "1234".into(),
                content_encryption_key: "1234".into(),
            })
            .idx(0)
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
    async fn last() {
        let db = SqliteStore::new(":memory:").await.unwrap();
        let record = test_record();
        db.push(&record).await.unwrap();

        let last = db
            .last(record.host.id, record.tag.as_str())
            .await
            .expect("failed to get store len");

        assert_eq!(
            last.unwrap().id,
            record.id,
            "expected to get back the same record that was inserted"
        );
    }

    #[tokio::test]
    async fn first() {
        let db = SqliteStore::new(":memory:").await.unwrap();
        let record = test_record();
        db.push(&record).await.unwrap();

        let first = db
            .first(record.host.id, record.tag.as_str())
            .await
            .expect("failed to get store len");

        assert_eq!(
            first.unwrap().id,
            record.id,
            "expected to get back the same record that was inserted"
        );
    }

    #[tokio::test]
    async fn len() {
        let db = SqliteStore::new(":memory:").await.unwrap();
        let record = test_record();
        db.push(&record).await.unwrap();

        let len = db
            .len(record.host.id, record.tag.as_str())
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

        let first_len = db.len(first.host.id, first.tag.as_str()).await.unwrap();
        let second_len = db.len(second.host.id, second.tag.as_str()).await.unwrap();

        assert_eq!(first_len, 1, "expected length of 1 after insert");
        assert_eq!(second_len, 1, "expected length of 1 after insert");
    }

    #[tokio::test]
    async fn append_a_bunch() {
        let db = SqliteStore::new(":memory:").await.unwrap();

        let mut tail = test_record();
        db.push(&tail).await.expect("failed to push record");

        for _ in 1..100 {
            tail = tail.append(vec![1, 2, 3, 4]).encrypt::<PASETO_V4>(&[0; 32]);
            db.push(&tail).await.unwrap();
        }

        assert_eq!(
            db.len(tail.host.id, tail.tag.as_str()).await.unwrap(),
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
            tail = tail.append(vec![1, 2, 3]).encrypt::<PASETO_V4>(&[0; 32]);
            records.push(tail.clone());
        }

        db.push_batch(records.iter()).await.unwrap();

        assert_eq!(
            db.len(tail.host.id, tail.tag.as_str()).await.unwrap(),
            10000,
            "failed to insert 10k records"
        );
    }
}
