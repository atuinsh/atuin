// Here we are using sqlite as a pretty dumb store, and will not be running any complex queries.
// Multiple stores of multiple types are all stored in one chonky table (for now), and we just index
// by tag/host

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use atuin_common::encryption::paseto_v4;
use atuin_common::utils;
use atuin_domain::record::{
    Host, HostId, Record, RecordId, RecordIdx, RecordStatus, RecordTag, RecordVersion,
};
use eyre::{Result, eyre};
use fs_err as fs;
use sqlx::Row;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow,
    SqliteSynchronous,
};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    pub async fn new(path: impl AsRef<Path>, timeout: f64) -> Result<Self> {
        let path = path.as_ref();

        debug!("opening sqlite database at {path:?}");

        if utils::broken_symlink(path) {
            eprintln!(
                "Atuin: Sqlite db path ({path:?}) is a broken symlink. Unable to read or create \
                 replacement."
            );
            std::process::exit(1);
        }

        if !path.exists()
            && let Some(dir) = path.parent()
        {
            fs::create_dir_all(dir)?;
        }

        let opts = SqliteConnectOptions::from_str(path.as_os_str().to_str().unwrap())?
            .journal_mode(SqliteJournalMode::Wal)
            .optimize_on_close(true, None)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .acquire_timeout(Duration::try_from_secs_f64(timeout)?)
            .connect_with(opts)
            .await?;

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
        r: &Record<paseto_v4::EncryptedData>,
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
        .bind(r.data.raw.as_str())
        .bind(r.data.cek.as_str())
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    fn query_row(row: &SqliteRow) -> Record<paseto_v4::EncryptedData> {
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
            tag: RecordTag::from(row.get::<String, _>("tag")),
            version: RecordVersion::from(row.get::<String, _>("version")),
            data: paseto_v4::EncryptedData {
                raw: row.get("data"),
                cek: row.get("cek"),
            },
        }
    }

    async fn load_all(&self) -> Result<Vec<Record<paseto_v4::EncryptedData>>> {
        let res = sqlx::query("select * from store ")
            .map(|row| Self::query_row(&row))
            .fetch_all(&self.pool)
            .await?;

        Ok(res)
    }

    pub async fn push(&self, record: &Record<paseto_v4::EncryptedData>) -> Result<()> {
        self.push_batch(std::iter::once(record)).await
    }

    pub async fn push_batch(
        &self,
        records: impl Iterator<Item = &Record<paseto_v4::EncryptedData>> + Send + Sync,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        for record in records {
            Self::save_raw(&mut tx, record).await?;
        }

        tx.commit().await?;

        Ok(())
    }

    pub async fn get(&self, id: RecordId) -> Result<Record<paseto_v4::EncryptedData>> {
        let res = sqlx::query("select * from store where store.id = ?1")
            .bind(id.0.as_hyphenated().to_string())
            .map(|row| Self::query_row(&row))
            .fetch_one(&self.pool)
            .await?;

        Ok(res)
    }

    pub async fn delete(&self, id: RecordId) -> Result<()> {
        sqlx::query("delete from store where id = ?1")
            .bind(id.0.as_hyphenated().to_string())
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn delete_all(&self) -> Result<()> {
        sqlx::query("delete from store").execute(&self.pool).await?;

        Ok(())
    }

    pub async fn last(
        &self,
        host: HostId,
        tag: &RecordTag,
    ) -> Result<Option<Record<paseto_v4::EncryptedData>>> {
        let res =
            sqlx::query("select * from store where host=?1 and tag=?2 order by idx desc limit 1")
                .bind(host.0.as_hyphenated().to_string())
                .bind(tag.as_str())
                .map(|row| Self::query_row(&row))
                .fetch_one(&self.pool)
                .await;

        match res {
            Err(sqlx::Error::RowNotFound) => Ok(None),
            Err(e) => Err(eyre!("an error occurred: {}", e)),
            Ok(record) => Ok(Some(record)),
        }
    }

    pub async fn first(
        &self,
        host: HostId,
        tag: &RecordTag,
    ) -> Result<Option<Record<paseto_v4::EncryptedData>>> {
        self.idx(host, tag, 0).await
    }

    pub async fn len_all(&self) -> Result<u64> {
        let res: Result<(i64,), sqlx::Error> =
            sqlx::query_as("select count(*) from store").fetch_one(&self.pool).await;
        match res {
            Err(e) => Err(eyre!("failed to fetch local store len: {}", e)),
            Ok(v) => Ok(v.0 as u64),
        }
    }

    pub async fn len_tag(&self, tag: &RecordTag) -> Result<u64> {
        let res: Result<(i64,), sqlx::Error> =
            sqlx::query_as("select count(*) from store where tag=?1")
                .bind(tag.as_str())
                .fetch_one(&self.pool)
                .await;
        match res {
            Err(e) => Err(eyre!("failed to fetch local store len: {}", e)),
            Ok(v) => Ok(v.0 as u64),
        }
    }

    pub async fn len(&self, host: HostId, tag: &RecordTag) -> Result<u64> {
        let last = self.last(host, tag).await?;

        if let Some(last) = last {
            return Ok(last.idx + 1);
        }

        Ok(0)
    }

    /// The smallest `idx >= 0` with no record for `(host, tag)`: Unlike `last().idx + 1`, this
    /// points at an interior hole when one exists.
    pub async fn first_gap(&self, host: HostId, tag: &RecordTag) -> Result<RecordIdx> {
        let gap: Option<i64> = sqlx::query_scalar(
            "select min(idx) from (
                 select idx + 1 as idx from store where host = ?1 and tag = ?2
                 union
                 select 0
             ) as candidates
             where idx not in (select idx from store where host = ?1 and tag = ?2)",
        )
        .bind(host.0.as_hyphenated().to_string())
        .bind(tag.as_str())
        .fetch_one(&self.pool)
        .await?;

        Ok(gap.unwrap_or(0) as u64)
    }

    pub async fn next(
        &self,
        host: HostId,
        tag: &RecordTag,
        idx: RecordIdx,
        limit: u64,
    ) -> Result<Vec<Record<paseto_v4::EncryptedData>>> {
        let res = sqlx::query(
            "select * from store where idx >= ?1 and host = ?2 and tag = ?3 order by idx asc \
             limit ?4",
        )
        .bind(idx as i64)
        .bind(host.0.as_hyphenated().to_string())
        .bind(tag.as_str())
        .bind(limit as i64)
        .map(|row| Self::query_row(&row))
        .fetch_all(&self.pool)
        .await?;

        Ok(res)
    }

    pub async fn idx(
        &self,
        host: HostId,
        tag: &RecordTag,
        idx: RecordIdx,
    ) -> Result<Option<Record<paseto_v4::EncryptedData>>> {
        let res = sqlx::query("select * from store where idx = ?1 and host = ?2 and tag = ?3")
            .bind(idx as i64)
            .bind(host.0.as_hyphenated().to_string())
            .bind(tag.as_str())
            .map(|row| Self::query_row(&row))
            .fetch_one(&self.pool)
            .await;

        match res {
            Err(sqlx::Error::RowNotFound) => Ok(None),
            Err(e) => Err(eyre!("an error occurred: {}", e)),
            Ok(v) => Ok(Some(v)),
        }
    }

    pub async fn status(&self) -> Result<RecordStatus> {
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

            status.set_raw(host, RecordTag::from(i.1), i.2 as u64);
        }

        Ok(status)
    }

    pub async fn all_tagged(
        &self,
        tag: &RecordTag,
    ) -> Result<Vec<Record<paseto_v4::EncryptedData>>> {
        let res = sqlx::query("select * from store where tag = ?1 order by timestamp asc")
            .bind(tag.as_str())
            .map(|row| Self::query_row(&row))
            .fetch_all(&self.pool)
            .await?;

        Ok(res)
    }

    /// Reencrypt every single item in this store with a new key
    /// Be careful - this may mess with sync.
    pub async fn re_encrypt(
        &self,
        old_key: &paseto_v4::Key,
        new_key: &paseto_v4::Key,
    ) -> Result<()> {
        // Load all the records
        // In memory like some of the other code here
        // This will never be called in a hot loop, and only under the following circumstances
        // 1. The user has logged into a new account, with a new key. They are unlikely to have a
        //    lot of data
        // 2. The user has encountered some sort of issue, and runs a maintenance command that
        //    invokes this
        let all = self.load_all().await?;

        let re_encrypted = all
            .into_iter()
            .map(|record| {
                let data = paseto_v4::reencrypt_sync(&record.data, old_key, new_key)?;
                Ok(record.with_data(data))
            })
            .collect::<Result<Vec<_>>>()?;

        // next up, we delete all the old data and reinsert the new stuff
        // do it in one transaction, so if anything fails we rollback OK

        let mut tx = self.pool.begin().await?;

        let res = sqlx::query("delete from store").execute(&mut *tx).await?;

        let rows = res.rows_affected();
        debug!("deleted {rows} rows");

        // don't call push_batch, as it will start its own transaction
        // call the underlying save_raw

        for record in re_encrypted {
            Self::save_raw(&mut tx, &record).await?;
        }

        tx.commit().await?;

        Ok(())
    }

    /// Verify that every record in this store can be decrypted with the current key
    /// Someday maybe also check each tag/record can be deserialized, but not for now.
    pub async fn verify(&self, key: &paseto_v4::Key) -> Result<()> {
        let all = self.load_all().await?;

        all.into_iter().map(|record| record.decrypt(key)).collect::<Result<Vec<_>>>()?;

        Ok(())
    }

    /// Verify that every record in this store can be decrypted with the current key
    /// Someday maybe also check each tag/record can be deserialized, but not for now.
    pub async fn purge(&self, key: &paseto_v4::Key) -> Result<()> {
        let all = self.load_all().await?;

        for record in &all {
            match record.clone().decrypt(key) {
                Ok(_) => continue,
                Err(_) => {
                    println!("Failed to decrypt {}, deleting", record.id.0.as_hyphenated());

                    self.delete(record.id).await?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::encryption::paseto_v4;
    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::{DecryptedData, Host, HostId, Record, RecordTag, RecordVersion};
    use rstest::{fixture, rstest};

    use super::SqliteStore;
    use crate::settings::test_local_timeout;

    #[fixture]
    async fn store() -> SqliteStore {
        SqliteStore::new(":memory:", test_local_timeout()).await.unwrap()
    }

    #[fixture]
    fn record() -> Record<paseto_v4::EncryptedData> {
        Record::builder()
            .host(Host::new(HostId(uuid_v7())))
            .version("v1".into())
            .tag(RecordTag::Other(uuid_v7().simple().to_string()))
            .data(paseto_v4::EncryptedData {
                raw: "1234".into(),
                cek: "1234".into(),
            })
            .idx(0)
            .build()
    }

    #[rstest]
    #[tokio::test]
    async fn create_db(#[future(awt)] store: SqliteStore) {
        // the `store` fixture opens/creates the db and unwraps; a successful
        // injection proves creation succeeded. confirm it is queryable.
        assert_eq!(store.len_all().await.unwrap(), 0, "db could not be created");
    }

    #[rstest]
    #[tokio::test]
    async fn push_record(
        #[future(awt)] store: SqliteStore,
        record: Record<paseto_v4::EncryptedData>,
    ) {
        store.push(&record).await.expect("failed to insert record");
    }

    #[rstest]
    #[tokio::test]
    async fn get_record(
        #[future(awt)] store: SqliteStore,
        record: Record<paseto_v4::EncryptedData>,
    ) {
        store.push(&record).await.unwrap();
        let fetched = store.get(record.id).await.expect("failed to fetch record");
        assert_eq!(fetched, record, "records are not equal");
    }

    #[rstest]
    #[tokio::test]
    async fn last(#[future(awt)] store: SqliteStore, record: Record<paseto_v4::EncryptedData>) {
        store.push(&record).await.unwrap();
        let last = store.last(record.host.id, &record.tag).await.unwrap();
        assert_eq!(last.unwrap().id, record.id, "did not get the inserted record");
    }

    #[rstest]
    #[tokio::test]
    async fn first(#[future(awt)] store: SqliteStore, record: Record<paseto_v4::EncryptedData>) {
        store.push(&record).await.unwrap();
        let first = store.first(record.host.id, &record.tag).await.unwrap();
        assert_eq!(first.unwrap().id, record.id, "did not get the inserted record");
    }

    #[rstest]
    #[tokio::test]
    async fn first_gap_finds_the_contiguous_frontier(#[future(awt)] store: SqliteStore) {
        let host = HostId(uuid_v7());
        let tag = RecordTag::History;

        let at = |idx: u64| {
            Record::builder()
                .host(Host::new(host))
                .version("v1".into())
                .tag(tag.clone())
                .idx(idx)
                .data(paseto_v4::EncryptedData {
                    raw: "x".into(),
                    cek: "x".into(),
                })
                .build()
        };

        // Empty stream -> frontier is 0.
        assert_eq!(store.first_gap(host, &tag).await.unwrap(), 0);

        // Contiguous 0,1,2 -> frontier is the next idx, 3.
        for idx in [0, 1, 2] {
            store.push(&at(idx)).await.unwrap();
        }
        assert_eq!(store.first_gap(host, &tag).await.unwrap(), 3);

        // Add 4,5 but not 3: the frontier drops back to the hole, not the head + 1.
        for idx in [4, 5] {
            store.push(&at(idx)).await.unwrap();
        }
        assert_eq!(store.first_gap(host, &tag).await.unwrap(), 3);
    }

    #[rstest]
    #[tokio::test]
    async fn len(#[future(awt)] store: SqliteStore, record: Record<paseto_v4::EncryptedData>) {
        store.push(&record).await.unwrap();
        let len = store.len(record.host.id, &record.tag).await.unwrap();
        assert_eq!(len, 1, "expected length of 1 after insert");
    }

    #[rstest]
    #[tokio::test]
    async fn len_tag(#[future(awt)] store: SqliteStore, record: Record<paseto_v4::EncryptedData>) {
        store.push(&record).await.unwrap();
        let len = store.len_tag(&record.tag).await.unwrap();
        assert_eq!(len, 1, "expected length of 1 after insert");
    }

    #[rstest]
    #[tokio::test]
    async fn len_different_tags(#[future(awt)] store: SqliteStore) {
        // different tags model independent stores in one database, so each
        // is length 1 despite sharing a table
        let first = record();
        let second = record();
        store.push(&first).await.unwrap();
        store.push(&second).await.unwrap();

        assert_eq!(store.len(first.host.id, &first.tag).await.unwrap(), 1);
        assert_eq!(store.len(second.host.id, &second.tag).await.unwrap(), 1);
    }

    #[rstest]
    #[tokio::test]
    async fn append_a_bunch(#[future(awt)] store: SqliteStore) {
        let mut tail = record();
        store.push(&tail).await.expect("failed to push record");

        for _ in 1..100 {
            tail = tail.append(vec![1, 2, 3, 4]).encrypt(&[0; 32].into());
            store.push(&tail).await.unwrap();
        }

        assert_eq!(
            store.len(tail.host.id, &tail.tag).await.unwrap(),
            100,
            "failed to insert 100 records"
        );
        assert_eq!(store.len_tag(&tail.tag).await.unwrap(), 100, "failed to insert 100 records");
    }

    #[rstest]
    #[tokio::test]
    async fn append_a_big_bunch(#[future(awt)] store: SqliteStore) {
        let mut records: Vec<Record<paseto_v4::EncryptedData>> = Vec::with_capacity(10000);

        let mut tail = record();
        records.push(tail.clone());
        for _ in 1..10000 {
            tail = tail.append(vec![1, 2, 3]).encrypt(&[0; 32].into());
            records.push(tail.clone());
        }

        store.push_batch(records.iter()).await.unwrap();

        assert_eq!(
            store.len(tail.host.id, &tail.tag).await.unwrap(),
            10000,
            "failed to insert 10k records"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn re_encrypt(#[future(awt)] store: SqliteStore) {
        let key = paseto_v4::Key::generate();
        let data = vec![0u8, 1u8, 2u8, 3u8];
        let host_id = HostId(uuid_v7());

        for i in 0..10 {
            let record = Record::builder()
                .host(Host::new(host_id))
                .version(RecordVersion::Other("test".to_owned()))
                .tag(RecordTag::Other("test".to_owned()))
                .idx(i)
                .data(DecryptedData(data.clone()))
                .build()
                .encrypt(&key);
            store.push(&record).await.expect("failed to push encrypted record");
        }

        // the data decrypts with the current key
        let all = store.all_tagged(&RecordTag::Other("test".to_owned())).await.unwrap();
        assert_eq!(all.len(), 10, "failed to fetch all records");
        for record in all {
            let decrypted = record.decrypt(&key).unwrap();
            assert_eq!(decrypted.data.0, data);
        }

        // after re-encrypting: the old key fails, the new key works
        let new_key = paseto_v4::Key::generate();
        store.re_encrypt(&key, &new_key).await.expect("failed to re-encrypt store");

        let all = store.all_tagged(&RecordTag::Other("test".to_owned())).await.unwrap();
        for record in &all {
            assert!(
                record.clone().decrypt(&key).is_err(),
                "old key still decrypts after re-encrypt"
            );
        }
        for record in all {
            let decrypted = record.decrypt(&new_key).unwrap();
            assert_eq!(decrypted.data.0, data);
        }

        assert_eq!(store.len(host_id, &RecordTag::Other("test".to_owned())).await.unwrap(), 10);
    }
}
