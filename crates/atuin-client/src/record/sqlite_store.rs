// Here we are using sqlite as a pretty dumb store, and will not be running any complex queries.
// Multiple stores of multiple types are all stored in one chonky table (for now), and we just index
// by tag/host

use std::ffi::OsStr;
use std::str::FromStr;
use std::time::Duration;

use atuin_common::db;
use atuin_common::db::sqlite::{Sqlite, SqliteBuilder};
use atuin_common::encryption::paseto_v4;
use atuin_domain::record::{
    Host, HostId, Record, RecordId, RecordIdx, RecordSeriesKey, RecordStatus, RecordTag,
    RecordVersion,
};
use easy_cast::Conv;
use eyre::{Result, eyre};
use sqlx::Row;
use sqlx::sqlite::{SqlitePool, SqliteRow};
use tracing::instrument;
use uuid::Uuid;

const STORE_COLUMNS: &str = "id, idx, host, tag, timestamp, version, data, cek";

#[derive(Debug, Clone)]
pub struct SqliteStore {
    sqlite: Sqlite,
}

/// Newtype over the foreign `Record<EncryptedData>` so we can `impl FromRow` for
/// it locally (the orphan rule blocks implementing it on `Record` here). Unwrap
/// back to the inner record with `.into()`.
#[derive(derive_more::Into)]
struct DbRecord(Record<paseto_v4::EncryptedData>);

impl<'r> ::sqlx::FromRow<'r, SqliteRow> for DbRecord {
    fn from_row(row: &'r SqliteRow) -> ::sqlx::Result<Self> {
        let idx: i64 = row.try_get("idx")?;
        let timestamp: i64 = row.try_get("timestamp")?;

        // UUIDs are stored as hyphenated TEXT, so decode as a string and parse
        // rather than relying on sqlx's blob-oriented `Uuid` decoding.
        let parse_uuid = |column: &'static str| -> ::sqlx::Result<Uuid> {
            let raw: &str = row.try_get(column)?;
            Uuid::from_str(raw).map_err(|source| ::sqlx::Error::ColumnDecode {
                index: column.to_owned(),
                source: Box::new(source),
            })
        };

        Ok(Self(Record {
            id: RecordId(parse_uuid("id")?),
            idx: u64::conv(idx),
            host: Host::new(HostId(parse_uuid("host")?)),
            timestamp: u64::conv(timestamp),
            tag: RecordTag::from(row.try_get::<String, _>("tag")?),
            version: RecordVersion::from(row.try_get::<String, _>("version")?),
            data: paseto_v4::EncryptedData {
                raw: row.try_get("data")?,
                cek: row.try_get("cek")?,
            },
        }))
    }
}

impl SqliteStore {
    #[instrument(level = "trace", skip_all, fields(timeout = ?timeout), err)]
    pub async fn new(path: impl AsRef<OsStr>, timeout: Duration) -> Result<Self> {
        let path = path.as_ref();

        debug!("opening sqlite database at {path:?}");

        Self::from_builder(Sqlite::builder(path), timeout).await
    }

    pub async fn in_memory(timeout: Duration) -> Result<Self> {
        Self::from_builder(Sqlite::builder_in_memory(), timeout).await
    }

    async fn from_builder(builder: SqliteBuilder<'_>, timeout: Duration) -> Result<Self> {
        let sqlite = builder.timeout(timeout).open().await?;

        Self::setup_db(sqlite.pool()).await?;

        Ok(Self { sqlite })
    }

    #[instrument(level = "trace", skip_all, err)]
    async fn setup_db(pool: &SqlitePool) -> Result<()> {
        debug!("running sqlite database setup");

        db::migrate!(pool, "./record-migrations").await?;

        Ok(())
    }

    async fn save_raw(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        r: &Record<paseto_v4::EncryptedData>,
    ) -> Result<()> {
        // In sqlite, we are "limited" to i64. But that is still fine, until 2262.
        db::query(
            "insert or ignore into store(id, idx, host, tag, timestamp, version, data, cek)
                values(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(r.id.as_hyphenated().to_string())
        .bind(i64::conv(r.idx))
        .bind(r.host.id.as_hyphenated().to_string())
        .bind(r.tag.as_str())
        .bind(i64::conv(r.timestamp))
        .bind(r.version.as_str())
        .bind(r.data.raw.as_str())
        .bind(r.data.cek.as_str())
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    #[instrument(level = "trace", skip_all, err)]
    async fn load_all(&self) -> Result<Vec<Record<paseto_v4::EncryptedData>>> {
        let res = db::query_as::<_, DbRecord>(sqlx::AssertSqlSafe(format!(
            "select {STORE_COLUMNS} from store"
        )))
        .fetch_all(self.sqlite.pool())
        .await?;

        Ok(res.into_iter().map(Into::into).collect())
    }

    #[instrument(level = "trace", skip_all, fields(id = ?record.id, idx = record.idx, host = ?record.host.id, tag = ?record.tag), err)]
    pub async fn push(&self, record: &Record<paseto_v4::EncryptedData>) -> Result<()> {
        self.push_batch(std::iter::once(record)).await
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn push_batch(
        &self,
        records: impl Iterator<Item = &Record<paseto_v4::EncryptedData>> + Send + Sync,
    ) -> Result<()> {
        // `store` has 8 columns, so each row binds 8 parameters; keep a full chunk
        // within the bind-parameter limit. `max(1)` keeps the chunk non-empty on any
        // (implausible) tiny limit.
        const COLUMNS: usize = 8;
        let rows_per_insert = (self.sqlite.info().await.variable_number_limit() / COLUMNS).max(1);

        let mut records = records.peekable();
        let mut tx = self.sqlite.pool().begin().await?;

        while records.peek().is_some() {
            let mut builder = sqlx::QueryBuilder::new(
                "insert or ignore into store(id, idx, host, tag, timestamp, version, data, cek) ",
            );

            builder.push_values(records.by_ref().take(rows_per_insert), |mut b, r| {
                b.push_bind(r.id.0.as_hyphenated().to_string())
                    .push_bind(i64::conv(r.idx))
                    .push_bind(r.host.id.0.as_hyphenated().to_string())
                    .push_bind(r.tag.as_str())
                    .push_bind(i64::conv(r.timestamp))
                    .push_bind(r.version.as_str())
                    .push_bind(r.data.raw.as_str())
                    .push_bind(r.data.cek.as_str());
            });

            builder.build().execute(&mut *tx).await?;
        }

        tx.commit().await?;

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(id = ?id), err)]
    pub async fn get(&self, id: RecordId) -> Result<Record<paseto_v4::EncryptedData>> {
        let res = db::query_as::<_, DbRecord>(sqlx::AssertSqlSafe(format!(
            "select {STORE_COLUMNS} from store where store.id = ?1"
        )))
        .bind(id.as_hyphenated().to_string())
        .fetch_one(self.sqlite.pool())
        .await?;

        Ok(res.into())
    }

    #[instrument(level = "trace", skip_all, fields(id = ?id), err)]
    pub async fn delete(&self, id: RecordId) -> Result<()> {
        db::query("delete from store where id = ?1")
            .bind(id.as_hyphenated().to_string())
            .execute(self.sqlite.pool())
            .await?;

        Ok(())
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn delete_all(&self) -> Result<()> {
        db::query("delete from store").execute(self.sqlite.pool()).await?;

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(host = ?series.host_id, tag = ?series.tag), err)]
    pub async fn last(
        &self,
        series: &RecordSeriesKey,
    ) -> Result<Option<Record<paseto_v4::EncryptedData>>> {
        let res = db::query_as::<_, DbRecord>(sqlx::AssertSqlSafe(format!(
            "select {STORE_COLUMNS} from store where host=?1 and tag=?2 order by idx desc limit 1"
        )))
        .bind(series.host_id.as_hyphenated().to_string())
        .bind(series.tag.as_str())
        .fetch_one(self.sqlite.pool())
        .await;

        match res {
            Err(sqlx::Error::RowNotFound) => Ok(None),
            Err(e) => Err(eyre!("an error occurred: {}", e)),
            Ok(record) => Ok(Some(record.into())),
        }
    }

    #[instrument(level = "trace", skip_all, fields(host = ?series.host_id, tag = ?series.tag), err)]
    pub async fn first(
        &self,
        series: &RecordSeriesKey,
    ) -> Result<Option<Record<paseto_v4::EncryptedData>>> {
        self.idx(series, 0).await
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn len_all(&self) -> Result<u64> {
        let res: Result<(i64,), sqlx::Error> =
            db::query_as("select count(*) from store").fetch_one(self.sqlite.pool()).await;
        match res {
            Err(e) => Err(eyre!("failed to fetch local store len: {}", e)),
            Ok(v) => Ok(u64::conv(v.0)),
        }
    }

    #[instrument(level = "trace", skip_all, fields(tag = ?tag), err)]
    pub async fn len_tag(&self, tag: &RecordTag) -> Result<u64> {
        let res: Result<(i64,), sqlx::Error> =
            db::query_as("select count(*) from store where tag=?1")
                .bind(tag.as_str())
                .fetch_one(self.sqlite.pool())
                .await;
        match res {
            Err(e) => Err(eyre!("failed to fetch local store len: {}", e)),
            Ok(v) => Ok(u64::conv(v.0)),
        }
    }

    #[instrument(level = "trace", skip_all, fields(host = ?series.host_id, tag = ?series.tag), err)]
    pub async fn len(&self, series: &RecordSeriesKey) -> Result<u64> {
        let last = self.last(series).await?;

        if let Some(last) = last {
            return Ok(last.idx + 1);
        }

        Ok(0)
    }

    /// The smallest `idx >= 0` with no record for `(host, tag)`: Unlike `last().idx + 1`, this
    /// points at an interior hole when one exists.
    #[instrument(level = "trace", skip_all, fields(host = ?series.host_id, tag = ?series.tag), err)]
    pub async fn first_gap(&self, series: &RecordSeriesKey) -> Result<RecordIdx> {
        let gap: Option<i64> = db::query_scalar(
            "select min(idx) from (
                 select idx + 1 as idx from store where host = ?1 and tag = ?2
                 union
                 select 0
             ) as candidates
             where idx not in (select idx from store where host = ?1 and tag = ?2)",
        )
        .bind(series.host_id.as_hyphenated().to_string())
        .bind(series.tag.as_str())
        .fetch_one(self.sqlite.pool())
        .await?;

        Ok(u64::conv(gap.unwrap_or(0)))
    }

    #[instrument(level = "trace", skip_all, fields(host = ?series.host_id, tag = ?series.tag, idx, limit), err)]
    pub async fn next(
        &self,
        series: &RecordSeriesKey,
        idx: RecordIdx,
        limit: u64,
    ) -> Result<Vec<Record<paseto_v4::EncryptedData>>> {
        let res = db::query_as::<_, DbRecord>(sqlx::AssertSqlSafe(format!(
            "select {STORE_COLUMNS} from store where idx >= ?1 and host = ?2 and tag = ?3 order \
             by idx asc limit ?4"
        )))
        .bind(i64::conv(idx))
        .bind(series.host_id.as_hyphenated().to_string())
        .bind(series.tag.as_str())
        .bind(i64::conv(limit))
        .fetch_all(self.sqlite.pool())
        .await?;

        Ok(res.into_iter().map(Into::into).collect())
    }

    #[instrument(level = "trace", skip_all, fields(host = ?series.host_id, tag = ?series.tag, idx), err)]
    pub async fn idx(
        &self,
        series: &RecordSeriesKey,
        idx: RecordIdx,
    ) -> Result<Option<Record<paseto_v4::EncryptedData>>> {
        let res = db::query_as::<_, DbRecord>(sqlx::AssertSqlSafe(format!(
            "select {STORE_COLUMNS} from store where idx = ?1 and host = ?2 and tag = ?3"
        )))
        .bind(i64::conv(idx))
        .bind(series.host_id.as_hyphenated().to_string())
        .bind(series.tag.as_str())
        .fetch_one(self.sqlite.pool())
        .await;

        match res {
            Err(sqlx::Error::RowNotFound) => Ok(None),
            Err(e) => Err(eyre!("an error occurred: {}", e)),
            Ok(v) => Ok(Some(v.into())),
        }
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn status(&self) -> Result<RecordStatus> {
        let mut status = RecordStatus::new();

        let res: Result<Vec<(String, String, i64)>, sqlx::Error> =
            db::query_as("select host, tag, max(idx) from store group by host, tag")
                .fetch_all(self.sqlite.pool())
                .await;

        let res = match res {
            Err(e) => return Err(eyre!("failed to fetch local store status: {}", e)),
            Ok(v) => v,
        };

        for i in res {
            let host = HostId(
                Uuid::from_str(i.0.as_str()).expect("failed to parse uuid for local store status"),
            );

            status.set_raw(RecordSeriesKey::new(host, RecordTag::from(i.1)), u64::conv(i.2));
        }

        Ok(status)
    }

    #[instrument(level = "trace", skip_all, fields(tag = ?tag), err)]
    pub async fn all_tagged(
        &self,
        tag: &RecordTag,
    ) -> Result<Vec<Record<paseto_v4::EncryptedData>>> {
        let res = db::query_as::<_, DbRecord>(sqlx::AssertSqlSafe(format!(
            "select {STORE_COLUMNS} from store where tag = ?1 order by timestamp asc"
        )))
        .bind(tag.as_str())
        .fetch_all(self.sqlite.pool())
        .await?;

        Ok(res.into_iter().map(Into::into).collect())
    }

    /// Reencrypt every single item in this store with a new key
    /// Be careful - this may mess with sync.
    #[instrument(level = "trace", skip_all, err)]
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

        let mut tx = self.sqlite.pool().begin().await?;

        let res = db::query("delete from store").execute(&mut *tx).await?;

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
    #[instrument(level = "trace", skip_all, err)]
    pub async fn verify(&self, key: &paseto_v4::Key) -> Result<()> {
        let all = self.load_all().await?;

        all.into_iter().map(|record| record.decrypt(key)).collect::<Result<Vec<_>>>()?;

        Ok(())
    }

    /// Verify that every record in this store can be decrypted with the current key
    /// Someday maybe also check each tag/record can be deserialized, but not for now.
    #[instrument(level = "trace", skip_all, err)]
    pub async fn purge(&self, key: &paseto_v4::Key) -> Result<()> {
        let all = self.load_all().await?;

        for record in &all {
            match record.clone().decrypt(key) {
                Ok(_) => {}
                Err(_) => {
                    println!("Failed to decrypt {}, deleting", record.id.as_hyphenated());

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
    use atuin_domain::record::{
        DecryptedData, Host, HostId, Record, RecordSeriesKey, RecordTag, RecordVersion,
    };
    use rstest::{fixture, rstest};

    use super::SqliteStore;
    use crate::settings::test_local_timeout;

    #[fixture]
    async fn store() -> SqliteStore {
        SqliteStore::in_memory(test_local_timeout()).await.unwrap()
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
        let last = store.last(&record.series_key()).await.unwrap();
        assert_eq!(last.unwrap().id, record.id, "did not get the inserted record");
    }

    #[rstest]
    #[tokio::test]
    async fn first(#[future(awt)] store: SqliteStore, record: Record<paseto_v4::EncryptedData>) {
        store.push(&record).await.unwrap();
        let first = store.first(&record.series_key()).await.unwrap();
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
        assert_eq!(store.first_gap(&RecordSeriesKey::new(host, tag.clone())).await.unwrap(), 0);

        // Contiguous 0,1,2 -> frontier is the next idx, 3.
        for idx in [0, 1, 2] {
            store.push(&at(idx)).await.unwrap();
        }
        assert_eq!(store.first_gap(&RecordSeriesKey::new(host, tag.clone())).await.unwrap(), 3);

        // Add 4,5 but not 3: the frontier drops back to the hole, not the head + 1.
        for idx in [4, 5] {
            store.push(&at(idx)).await.unwrap();
        }
        assert_eq!(store.first_gap(&RecordSeriesKey::new(host, tag.clone())).await.unwrap(), 3);
    }

    #[rstest]
    #[tokio::test]
    async fn len(#[future(awt)] store: SqliteStore, record: Record<paseto_v4::EncryptedData>) {
        store.push(&record).await.unwrap();
        let len = store.len(&record.series_key()).await.unwrap();
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

        assert_eq!(store.len(&first.series_key()).await.unwrap(), 1);
        assert_eq!(store.len(&second.series_key()).await.unwrap(), 1);
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
            store.len(&tail.series_key()).await.unwrap(),
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
            store.len(&tail.series_key()).await.unwrap(),
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

        assert_eq!(
            store
                .len(&RecordSeriesKey::new(host_id, RecordTag::Other("test".to_owned())))
                .await
                .unwrap(),
            10
        );
    }
}
