// Here we are using sqlite as a pretty dumb store, and will not be running any complex queries.
// Multiple stores of multiple types are all stored in one chonky table (for now), and we just index
// by tag/host

use std::str::FromStr;
use std::{path::Path, time::Duration};

use async_trait::async_trait;
use eyre::{Result, eyre};
use fs_err as fs;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sqlx::{
    Row, TypeInfo, ValueRef,
    sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow,
        SqliteSynchronous,
    },
};

use atuin_common::record::{
    EncryptedData, Host, HostId, Record, RecordId, RecordIdx, RecordStatus,
};
use atuin_common::utils;
use uuid::Uuid;

use super::encryption::PASETO_V4;
use super::store::Store;

// Storage codec: encrypted payloads reach us as base64 text (a PASETO token
// and a JSON-wrapped PASERK key), which costs ~35% over the raw bytes - tens
// of MB on a large store. Payloads matching these exact shapes are stored
// with the base64 decoded to a blob; anything else is stored as text
// verbatim, and the column's storage type says which form a row is in. The
// sync wire format is unchanged: reads rebuild the original string, and
// compaction is only applied when re-expansion reproduces it byte for byte.
const DATA_PREFIX: &str = "v4.local.";
const CEK_PREFIX: &str = "{\"wpk\":\"k4.local-wrap.pie.";
const CEK_INFIX: &str = "\",\"kid\":\"k4.lid.";
const CEK_SUFFIX: &str = "\"}";

fn compact_data(data: &str) -> Option<Vec<u8>> {
    let payload = data.strip_prefix(DATA_PREFIX)?;
    // a '.' would mean the token carries a footer, which expand_data can't rebuild
    if payload.contains('.') {
        return None;
    }
    let blob = URL_SAFE_NO_PAD.decode(payload).ok()?;
    (expand_data(&blob) == data).then_some(blob)
}

fn expand_data(blob: &[u8]) -> String {
    format!("{DATA_PREFIX}{}", URL_SAFE_NO_PAD.encode(blob))
}

fn compact_cek(cek: &str) -> Option<Vec<u8>> {
    let (wpk, kid) = cek
        .strip_prefix(CEK_PREFIX)?
        .strip_suffix(CEK_SUFFIX)?
        .split_once(CEK_INFIX)?;
    let wpk = URL_SAFE_NO_PAD.decode(wpk).ok()?;
    let kid = URL_SAFE_NO_PAD.decode(kid).ok()?;

    let mut blob = Vec::with_capacity(1 + wpk.len() + kid.len());
    blob.push(u8::try_from(wpk.len()).ok()?);
    blob.extend_from_slice(&wpk);
    blob.extend_from_slice(&kid);
    (expand_cek(&blob).as_deref() == Some(cek)).then_some(blob)
}

fn expand_cek(blob: &[u8]) -> Option<String> {
    let (&wpk_len, rest) = blob.split_first()?;
    let (wpk, kid) = rest.split_at_checked(wpk_len as usize)?;
    Some(format!(
        "{CEK_PREFIX}{}{CEK_INFIX}{}{CEK_SUFFIX}",
        URL_SAFE_NO_PAD.encode(wpk),
        URL_SAFE_NO_PAD.encode(kid)
    ))
}

fn column_is_blob(row: &SqliteRow, col: &str) -> bool {
    row.try_get_raw(col)
        .expect("missing column in store row")
        .type_info()
        .name()
        == "BLOB"
}

/// Read a payload column, re-expanding the compact blob form to its original text.
fn column_payload(row: &SqliteRow, col: &str, expand: impl Fn(&[u8]) -> Option<String>) -> String {
    if column_is_blob(row, col) {
        expand(&row.get::<Vec<u8>, _>(col)).expect("invalid compact payload in sqlite DB")
    } else {
        row.get(col)
    }
}

/// Read a uuid column: a 16-byte blob, or hyphenated text from before the
/// compact-ids migration (which converts everything, but be lenient).
fn column_uuid(row: &SqliteRow, col: &str) -> Uuid {
    if column_is_blob(row, col) {
        Uuid::from_slice(&row.get::<Vec<u8>, _>(col)).expect("invalid uuid blob in sqlite DB")
    } else {
        Uuid::from_str(row.get(col)).expect("invalid uuid format in sqlite DB")
    }
}

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
                "Atuin: Sqlite db path ({path:?}) is a broken symlink. Unable to read or create replacement."
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
            .acquire_timeout(Duration::from_secs_f64(timeout))
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
        r: &Record<EncryptedData>,
    ) -> Result<()> {
        // In sqlite, we are "limited" to i64. But that is still fine, until 2262.
        let query = sqlx::query(
            "insert or ignore into store(id, idx, host, tag, timestamp, version, data, cek)
                values(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(r.id.0.as_bytes().as_slice())
        .bind(r.idx as i64)
        .bind(r.host.id.0.as_bytes().as_slice())
        .bind(r.tag.as_str())
        .bind(r.timestamp as i64)
        .bind(r.version.as_str());

        let query = match compact_data(&r.data.data) {
            Some(blob) => query.bind(blob),
            None => query.bind(r.data.data.as_str()),
        };
        let query = match compact_cek(&r.data.content_encryption_key) {
            Some(blob) => query.bind(blob),
            None => query.bind(r.data.content_encryption_key.as_str()),
        };

        query.execute(&mut **tx).await?;

        Ok(())
    }

    fn query_row(row: SqliteRow) -> Record<EncryptedData> {
        let idx: i64 = row.get("idx");
        let timestamp: i64 = row.get("timestamp");

        // tbh at this point things are pretty fucked so just panic
        let id = column_uuid(&row, "id");
        let host = column_uuid(&row, "host");

        Record {
            id: RecordId(id),
            idx: idx as u64,
            host: Host::new(HostId(host)),
            timestamp: timestamp as u64,
            tag: row.get("tag"),
            version: row.get("version"),
            data: EncryptedData {
                data: column_payload(&row, "data", |blob| Some(expand_data(blob))),
                content_encryption_key: column_payload(&row, "cek", expand_cek),
            },
        }
    }

    async fn load_all(&self) -> Result<Vec<Record<EncryptedData>>> {
        let res = sqlx::query("select * from store ")
            .map(Self::query_row)
            .fetch_all(&self.pool)
            .await?;

        Ok(res)
    }

    /// Rewrite rows saved before the compact encoding existed (base64 text
    /// payloads) into the blob form, then vacuum to give the space back to the
    /// filesystem. Returns the number of rows rewritten. One-off maintenance:
    /// new rows are always written compact.
    pub async fn compact(&self) -> Result<u64> {
        let mut rewritten = 0u64;
        let mut cursor = 0i64;

        loop {
            let rows = sqlx::query(
                "select rowid, data, cek from store
                    where rowid > ?1 and (typeof(data) = 'text' or typeof(cek) = 'text')
                    order by rowid asc limit 1000",
            )
            .bind(cursor)
            .fetch_all(&self.pool)
            .await?;

            let Some(last) = rows.last() else { break };
            cursor = last.get("rowid");

            let mut tx = self.pool.begin().await?;
            for row in &rows {
                let rowid: i64 = row.get("rowid");
                let data = compact_data(&column_payload(row, "data", |b| Some(expand_data(b))));
                let cek = compact_cek(&column_payload(row, "cek", expand_cek));
                if data.is_none() && cek.is_none() {
                    // not in a shape we can compact - leave it as text
                    continue;
                }

                sqlx::query(
                    "update store set data = coalesce(?2, data), cek = coalesce(?3, cek)
                        where rowid = ?1",
                )
                .bind(rowid)
                .bind(data)
                .bind(cek)
                .execute(&mut *tx)
                .await?;
                rewritten += 1;
            }
            tx.commit().await?;
        }

        sqlx::query("vacuum").execute(&self.pool).await?;
        sqlx::query("pragma wal_checkpoint(truncate)")
            .execute(&self.pool)
            .await?;

        Ok(rewritten)
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
            .bind(id.0.as_bytes().as_slice())
            .map(Self::query_row)
            .fetch_one(&self.pool)
            .await?;

        Ok(res)
    }

    async fn delete(&self, id: RecordId) -> Result<()> {
        sqlx::query("delete from store where id = ?1")
            .bind(id.0.as_bytes().as_slice())
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn delete_all(&self) -> Result<()> {
        sqlx::query("delete from store").execute(&self.pool).await?;

        Ok(())
    }

    async fn last(&self, host: HostId, tag: &str) -> Result<Option<Record<EncryptedData>>> {
        let res =
            sqlx::query("select * from store where host=?1 and tag=?2 order by idx desc limit 1")
                .bind(host.0.as_bytes().as_slice())
                .bind(tag)
                .map(Self::query_row)
                .fetch_one(&self.pool)
                .await;

        match res {
            Err(sqlx::Error::RowNotFound) => Ok(None),
            Err(e) => Err(eyre!("an error occurred: {}", e)),
            Ok(record) => Ok(Some(record)),
        }
    }

    async fn first(&self, host: HostId, tag: &str) -> Result<Option<Record<EncryptedData>>> {
        self.idx(host, tag, 0).await
    }

    async fn len_all(&self) -> Result<u64> {
        let res: Result<(i64,), sqlx::Error> = sqlx::query_as("select count(*) from store")
            .fetch_one(&self.pool)
            .await;
        match res {
            Err(e) => Err(eyre!("failed to fetch local store len: {}", e)),
            Ok(v) => Ok(v.0 as u64),
        }
    }

    async fn len_tag(&self, tag: &str) -> Result<u64> {
        let res: Result<(i64,), sqlx::Error> =
            sqlx::query_as("select count(*) from store where tag=?1")
                .bind(tag)
                .fetch_one(&self.pool)
                .await;
        match res {
            Err(e) => Err(eyre!("failed to fetch local store len: {}", e)),
            Ok(v) => Ok(v.0 as u64),
        }
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
        let res = sqlx::query(
            "select * from store where idx >= ?1 and host = ?2 and tag = ?3 order by idx asc limit ?4",
        )
        .bind(idx as i64)
        .bind(host.0.as_bytes().as_slice())
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
            .bind(host.0.as_bytes().as_slice())
            .bind(tag)
            .map(Self::query_row)
            .fetch_one(&self.pool)
            .await;

        match res {
            Err(sqlx::Error::RowNotFound) => Ok(None),
            Err(e) => Err(eyre!("an error occurred: {}", e)),
            Ok(v) => Ok(Some(v)),
        }
    }

    async fn status(&self) -> Result<RecordStatus> {
        let mut status = RecordStatus::new();

        let res = sqlx::query("select host, tag, max(idx) as idx from store group by host, tag")
            .fetch_all(&self.pool)
            .await;

        let res = match res {
            Err(e) => return Err(eyre!("failed to fetch local store status: {}", e)),
            Ok(v) => v,
        };

        for row in res {
            let host = HostId(column_uuid(&row, "host"));
            let tag: String = row.get("tag");
            let idx: i64 = row.get("idx");

            status.set_raw(host, tag, idx as u64);
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

    /// Reencrypt every single item in this store with a new key
    /// Be careful - this may mess with sync.
    async fn re_encrypt(&self, old_key: &[u8; 32], new_key: &[u8; 32]) -> Result<()> {
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
            .map(|record| record.re_encrypt::<PASETO_V4>(old_key, new_key))
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
    async fn verify(&self, key: &[u8; 32]) -> Result<()> {
        let all = self.load_all().await?;

        all.into_iter()
            .map(|record| record.decrypt::<PASETO_V4>(key))
            .collect::<Result<Vec<_>>>()?;

        Ok(())
    }

    /// Verify that every record in this store can be decrypted with the current key
    /// Someday maybe also check each tag/record can be deserialized, but not for now.
    async fn purge(&self, key: &[u8; 32]) -> Result<()> {
        let all = self.load_all().await?;

        for record in all.iter() {
            match record.clone().decrypt::<PASETO_V4>(key) {
                Ok(_) => continue,
                Err(_) => {
                    println!(
                        "Failed to decrypt {}, deleting",
                        record.id.0.as_hyphenated()
                    );

                    self.delete(record.id).await?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::{
        record::{DecryptedData, EncryptedData, Host, HostId, Record},
        utils::uuid_v7,
    };

    use crate::{
        encryption::generate_encoded_key,
        record::{encryption::PASETO_V4, store::Store},
        settings::test_local_timeout,
    };

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

    #[test]
    fn codec_round_trips_real_encrypted_record() {
        let record = Record::builder()
            .host(Host::new(HostId(uuid_v7())))
            .version("v0".into())
            .tag("history".into())
            .idx(0)
            .data(DecryptedData(vec![1, 2, 3, 4]))
            .build()
            .encrypt::<PASETO_V4>(&[7; 32]);

        let data_blob = super::compact_data(&record.data.data).expect("real token should compact");
        assert_eq!(super::expand_data(&data_blob), record.data.data);
        assert!(data_blob.len() < record.data.data.len());

        let cek_blob = super::compact_cek(&record.data.content_encryption_key)
            .expect("real cek should compact");
        assert_eq!(
            super::expand_cek(&cek_blob).as_deref(),
            Some(record.data.content_encryption_key.as_str())
        );
        assert!(cek_blob.len() < record.data.content_encryption_key.len());
    }

    #[test]
    fn codec_leaves_unknown_shapes_alone() {
        // not a token at all, a token with a footer, invalid base64
        assert_eq!(super::compact_data("1234"), None);
        assert_eq!(super::compact_data("v4.local.abc.Zm9vdGVy"), None);
        assert_eq!(super::compact_data("v4.local.!!!"), None);
        // base64 that would not round-trip byte for byte (padding)
        assert_eq!(super::compact_data("v4.local.YQ=="), None);
        assert_eq!(super::compact_cek("1234"), None);
        assert_eq!(super::compact_cek("{\"wpk\":\"nope\"}"), None);
    }

    #[tokio::test]
    async fn legacy_text_rows_read_back_and_compact() {
        let db = SqliteStore::new(":memory:", test_local_timeout())
            .await
            .unwrap();

        // a realistic encrypted record, inserted the way pre-codec atuin
        // stored it: uuids and payloads all as text
        let record = Record::builder()
            .host(Host::new(HostId(uuid_v7())))
            .version("v0".into())
            .tag("history".into())
            .idx(0)
            .data(DecryptedData(vec![1, 2, 3, 4]))
            .build()
            .encrypt::<PASETO_V4>(&[7; 32]);

        sqlx::query(
            "insert into store(id, idx, host, tag, timestamp, version, data, cek)
                values(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(record.id.0.as_hyphenated().to_string())
        .bind(record.idx as i64)
        .bind(record.host.id.0.as_hyphenated().to_string())
        .bind(record.tag.as_str())
        .bind(record.timestamp as i64)
        .bind(record.version.as_str())
        .bind(record.data.data.as_str())
        .bind(record.data.content_encryption_key.as_str())
        .execute(&db.pool)
        .await
        .unwrap();

        let read = db.load_all().await.unwrap();
        assert_eq!(read, vec![record.clone()], "text row should read unchanged");

        let rewritten = db.compact().await.unwrap();
        assert_eq!(rewritten, 1);

        let read = db.load_all().await.unwrap();
        assert_eq!(read, vec![record], "compacted row should read unchanged");

        // compact again: nothing left to rewrite
        assert_eq!(db.compact().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn create_db() {
        let db = SqliteStore::new(":memory:", test_local_timeout()).await;

        assert!(
            db.is_ok(),
            "db could not be created, {:?}",
            db.err().unwrap()
        );
    }

    #[tokio::test]
    async fn push_record() {
        let db = SqliteStore::new(":memory:", test_local_timeout())
            .await
            .unwrap();
        let record = test_record();

        db.push(&record).await.expect("failed to insert record");
    }

    #[tokio::test]
    async fn get_record() {
        let db = SqliteStore::new(":memory:", test_local_timeout())
            .await
            .unwrap();
        let record = test_record();
        db.push(&record).await.unwrap();

        let new_record = db.get(record.id).await.expect("failed to fetch record");

        assert_eq!(record, new_record, "records are not equal");
    }

    #[tokio::test]
    async fn last() {
        let db = SqliteStore::new(":memory:", test_local_timeout())
            .await
            .unwrap();
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
        let db = SqliteStore::new(":memory:", test_local_timeout())
            .await
            .unwrap();
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
        let db = SqliteStore::new(":memory:", test_local_timeout())
            .await
            .unwrap();
        let record = test_record();
        db.push(&record).await.unwrap();

        let len = db
            .len(record.host.id, record.tag.as_str())
            .await
            .expect("failed to get store len");

        assert_eq!(len, 1, "expected length of 1 after insert");
    }

    #[tokio::test]
    async fn len_tag() {
        let db = SqliteStore::new(":memory:", test_local_timeout())
            .await
            .unwrap();
        let record = test_record();
        db.push(&record).await.unwrap();

        let len = db
            .len_tag(record.tag.as_str())
            .await
            .expect("failed to get store len");

        assert_eq!(len, 1, "expected length of 1 after insert");
    }

    #[tokio::test]
    async fn len_different_tags() {
        let db = SqliteStore::new(":memory:", test_local_timeout())
            .await
            .unwrap();

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
        let db = SqliteStore::new(":memory:", test_local_timeout())
            .await
            .unwrap();

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

        assert_eq!(
            db.len_tag(tail.tag.as_str()).await.unwrap(),
            100,
            "failed to insert 100 records"
        );
    }

    #[tokio::test]
    async fn append_a_big_bunch() {
        let db = SqliteStore::new(":memory:", test_local_timeout())
            .await
            .unwrap();

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

    #[tokio::test]
    async fn re_encrypt() {
        let store = SqliteStore::new(":memory:", test_local_timeout())
            .await
            .unwrap();
        let (key, _) = generate_encoded_key().unwrap();
        let data = vec![0u8, 1u8, 2u8, 3u8];
        let host_id = HostId(uuid_v7());

        for i in 0..10 {
            let record = Record::builder()
                .host(Host::new(host_id))
                .version(String::from("test"))
                .tag(String::from("test"))
                .idx(i)
                .data(DecryptedData(data.clone()))
                .build();

            let record = record.encrypt::<PASETO_V4>(&key.into());
            store
                .push(&record)
                .await
                .expect("failed to push encrypted record");
        }

        // first, check that we can decrypt the data with the current key
        let all = store.all_tagged("test").await.unwrap();

        assert_eq!(all.len(), 10, "failed to fetch all records");

        for record in all {
            let decrypted = record.decrypt::<PASETO_V4>(&key.into()).unwrap();
            assert_eq!(decrypted.data.0, data);
        }

        // reencrypt the store, then check if
        // 1) it cannot be decrypted with the old key
        // 2) it can be decrypted with the new key

        let (new_key, _) = generate_encoded_key().unwrap();
        store
            .re_encrypt(&key.into(), &new_key.into())
            .await
            .expect("failed to re-encrypt store");

        let all = store.all_tagged("test").await.unwrap();

        for record in all.iter() {
            let decrypted = record.clone().decrypt::<PASETO_V4>(&key.into());
            assert!(
                decrypted.is_err(),
                "did not get error decrypting with old key after re-encrypt"
            )
        }

        for record in all {
            let decrypted = record.decrypt::<PASETO_V4>(&new_key.into()).unwrap();
            assert_eq!(decrypted.data.0, data);
        }

        assert_eq!(store.len(host_id, "test").await.unwrap(), 10);
    }
}
