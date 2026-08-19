use std::collections::HashSet;
use std::fmt::Write;
use std::num::NonZeroUsize;
use std::time::Duration;

use atuin_common::encryption::paseto_v4;
use atuin_common::futures::stream::chunk_by_bounded;
use atuin_common::rmp::decode::Bytes;
use atuin_domain::record::{
    DecryptedData, Host, HostId, Record, RecordId, RecordIdx, RecordTag, RecordVersion,
};
use eyre::{Result, bail, eyre};
use futures::{Stream, StreamExt, TryStreamExt, future, stream};
use indicatif::{ProgressBar, ProgressState, ProgressStyle};

use super::{History, HistoryId, Version};
use crate::database::{Sqlite, current_context};
use crate::record::sqlite_store::SqliteStore;

#[derive(Debug, Clone)]
pub struct HistoryStore {
    pub store: SqliteStore,
    pub host_id: HostId,
    pub encryption_key: paseto_v4::Key,
}

#[derive(Debug, Eq, PartialEq, Clone, strum_macros::EnumDiscriminants)]
#[strum_discriminants(name(HistoryRecordKind))]
#[allow(
    clippy::large_enum_variant,
    reason = "`Create` records are much more common than `Delete` records; wrapping in a `Box`
        would use more memory overall"
)]
pub enum HistoryRecord {
    Create(History),   // Create a history record
    Delete(HistoryId), // Delete a history record, identified by ID
}

impl HistoryRecord {
    /// Serialize a history record, returning DecryptedData
    /// The record will be of a certain type
    /// We map those like so:
    ///
    /// HistoryRecord::Create -> 0
    /// HistoryRecord::Delete-> 1
    ///
    /// This numeric identifier is then written as the first byte to the buffer. For history, we
    /// append the serialized history right afterwards, to avoid having to handle serialization
    /// twice.
    ///
    /// Deletion simply refers to the history by ID
    pub fn serialize(&self) -> Result<DecryptedData> {
        // probably don't actually need to use rmp here, but if we ever need to extend it, it's a
        // nice wrapper around raw byte stuff
        use atuin_common::rmp::encode;

        let mut output = vec![];

        match self {
            Self::Create(history) => {
                // 0 -> a history create
                encode::write_u8(&mut output, 0)?;

                let bytes = history.serialize()?;

                encode::write_bin(&mut output, &bytes.0)?;
            }
            Self::Delete(id) => {
                // 1 -> a history delete
                encode::write_u8(&mut output, 1)?;
                encode::write_str(&mut output, id.0.as_str())?;
            }
        };

        Ok(DecryptedData(output))
    }

    pub fn deserialize(bytes: &DecryptedData, version: &str) -> Result<Self> {
        use atuin_common::rmp::decode;

        fn error_report<E: std::fmt::Debug>(err: E) -> eyre::Report {
            eyre!("{err:?}")
        }

        let mut bytes = Bytes::new(&bytes.0);

        let record_type = decode::read_u8(&mut bytes).map_err(error_report)?;

        match record_type {
            // 0 -> HistoryRecord::Create
            0 => {
                // not super useful to us atm, but perhaps in the future
                // written by write_bin above
                let _ = decode::read_bin_len(&mut bytes).map_err(error_report)?;

                let record = History::deserialize(bytes.remaining_slice(), version)?;

                Ok(Self::Create(record))
            }

            // 1 -> HistoryRecord::Delete
            1 => {
                let bytes = bytes.remaining_slice();
                let (id, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;

                if !bytes.is_empty() {
                    bail!(
                        "trailing bytes decoding HistoryRecord::Delete - malformed? got {bytes:?}"
                    );
                }

                Ok(Self::Delete(id.to_string().into()))
            }

            n => {
                bail!("unknown HistoryRecord type {n}");
            }
        }
    }
}

/// How many entries `incremental_build` holds in memory, and the most it puts into a single
/// `save_bulk`/`delete_rows` transaction.
const BUILD_BATCH_SIZE: NonZeroUsize = NonZeroUsize::new(5000).unwrap();

/// How many records `incremental_build` decodes concurrently. Decoding is read-then-decrypt per
/// record; overlapping the reads keeps the record store's pool busy without unbounded fan-out.
/// Kept under the store's connection pool size so decodes don't starve other readers.
const DECODE_CONCURRENCY: usize = 4;

impl HistoryStore {
    pub fn new(store: SqliteStore, host_id: HostId, encryption_key: paseto_v4::Key) -> Self {
        Self {
            store,
            host_id,
            encryption_key,
        }
    }

    async fn push_record(&self, record: HistoryRecord) -> Result<(RecordId, RecordIdx)> {
        let bytes = record.serialize()?;
        let idx =
            self.store.last(self.host_id, &RecordTag::History).await?.map_or(0, |p| p.idx + 1);

        let record = Record::builder()
            .host(Host::new(self.host_id))
            .version(RecordVersion::from(Version::LATEST.name()))
            .tag(RecordTag::History)
            .idx(idx)
            .data(bytes)
            .build();

        let id = record.id;

        self.store.push(&record.encrypt(&self.encryption_key)).await?;

        Ok((id, idx))
    }

    async fn push_batch(&self, records: impl Iterator<Item = HistoryRecord>) -> Result<()> {
        let mut ret = Vec::new();

        let idx =
            self.store.last(self.host_id, &RecordTag::History).await?.map_or(0, |p| p.idx + 1);

        // Could probably _also_ do this as an iterator, but let's see how this is for now.
        // optimizing for minimal sqlite transactions, this code can be optimised later
        for (n, record) in records.enumerate() {
            let bytes = record.serialize()?;

            let record = Record::builder()
                .host(Host::new(self.host_id))
                .version(RecordVersion::from(Version::LATEST.name()))
                .tag(RecordTag::History)
                .idx(idx + n as u64)
                .data(bytes)
                .build();

            let record = record.encrypt(&self.encryption_key);

            ret.push(record);
        }

        self.store.push_batch(ret.iter()).await?;

        Ok(())
    }

    pub async fn delete(&self, id: HistoryId) -> Result<(RecordId, RecordIdx)> {
        let record = HistoryRecord::Delete(id);

        self.push_record(record).await
    }

    /// Delete a batch of history entries via the record store.
    /// Returns the record IDs so the caller can run incremental_build when ready.
    pub async fn delete_entries(
        &self,
        entries: impl IntoIterator<Item = History>,
    ) -> Result<Vec<RecordId>> {
        let mut record_ids = Vec::new();
        for entry in entries {
            let (id, _) = self.delete(entry.id).await?;
            record_ids.push(id);
        }
        Ok(record_ids)
    }

    pub async fn push(&self, history: History) -> Result<(RecordId, RecordIdx)> {
        // TODO(ellie): move the history store to its own file
        // it's tiny rn so fine as is
        let record = HistoryRecord::Create(history);

        self.push_record(record).await
    }

    pub async fn history(&self) -> Result<Vec<HistoryRecord>> {
        // Atm this loads all history into memory
        // Not ideal as that is potentially quite a lot, although history will be small.
        let records = self.store.all_tagged(&RecordTag::History).await?;
        let mut ret = Vec::with_capacity(records.len());
        let mut skipped = 0;

        for record in records {
            let id = record.id;
            let version = record.version.clone();

            // A record we can't decrypt or decode must not block the rest of the store -
            // skip it, and load everything else.
            let hist = match Version::from_name(version.as_str()) {
                Some(_) => record.decrypt(&self.encryption_key).and_then(|decrypted| {
                    HistoryRecord::deserialize(&decrypted.data, version.as_str())
                }),
                None => Err(eyre!("unknown history version {version:?}")),
            };

            match hist {
                Ok(hist) => ret.push(hist),
                Err(e) => {
                    warn!("failed to decode history record {}, skipping: {e}", id.0);
                    skipped += 1;
                }
            }
        }

        if skipped > 0 {
            // library code that may run under the TUI or shell hooks, so no stderr here
            warn!(
                "skipped {skipped} history records that could not be decrypted or decoded. Run \
                 `atuin store verify` to check your store, and `atuin store purge` to remove \
                 broken records locally."
            );
        }

        Ok(ret)
    }

    pub async fn build(&self, database: &Sqlite) -> Result<()> {
        // I'd like to change how we rebuild and not couple this with the database, but need to
        // consider the structure more deeply. This will be easy to change.

        // TODO(ellie): page or iterate this
        let history = self.history().await?;

        // In theory we could flatten this here
        // The current issue is that the database may have history in it already, from the old sync
        // This didn't actually delete old history
        // If we're sure we have a DB only maintained by the new store, we can flatten
        // create/delete before we even get to sqlite
        let mut creates = Vec::new();
        let mut deletes = Vec::new();

        for i in history {
            match i {
                HistoryRecord::Create(h) => {
                    creates.push(h);
                }
                HistoryRecord::Delete(id) => {
                    deletes.push(id);
                }
            }
        }

        database.save_bulk(&creates).await?;
        database.delete_rows(deletes).await?;

        Ok(())
    }

    /// Apply records to the history database, yielding each batch of created `History`.
    pub fn incremental_build<'a>(
        &'a self,
        database: &'a Sqlite,
        ids: &'a [RecordId],
    ) -> impl Stream<Item = Result<Vec<History>>> + 'a {
        let records = stream::iter(ids)
            .map(move |id| async move { self.decode(*id).await })
            .buffered(DECODE_CONCURRENCY)
            .filter_map(future::ready)
            .boxed();

        // Group adjacent records of the same kind so each kind lands in its own bulk transaction,
        // while the database still sees them in record order.
        chunk_by_bounded(records, BUILD_BATCH_SIZE, |record| HistoryRecordKind::from(record)).then(
            move |(kind, chunk)| async move {
                // TODO(ATU-594): unwrapping the chunk into a typed `Vec` reallocates what
                // `chunk_by_bounded` already collected.
                match kind {
                    HistoryRecordKind::Create => {
                        let creates: Vec<_> = chunk
                            .into_iter()
                            .map(|record| match record {
                                HistoryRecord::Create(h) => h,
                                HistoryRecord::Delete(_) => unreachable!(),
                            })
                            .collect();

                        database.save_bulk(&creates).await?;

                        Ok(creates)
                    }
                    HistoryRecordKind::Delete => {
                        let deletes = chunk.into_iter().map(|record| match record {
                            HistoryRecord::Delete(id) => id,
                            HistoryRecord::Create(_) => unreachable!(),
                        });

                        database.delete_rows(deletes).await?;

                        Ok(Vec::new())
                    }
                }
            },
        )
    }

    /// Read a record and decode it, or `None` if it is missing, not history, or undecodable.
    async fn decode(&self, id: RecordId) -> Option<HistoryRecord> {
        let record = self.store.get(id).await.ok()?;

        if record.tag != RecordTag::History {
            return None;
        }

        let version = record.version.clone();

        // Skip records we can't decrypt or decode, rather than failing the entire build.
        let record = match Version::from_name(version.as_str()) {
            Some(_) => record.decrypt(&self.encryption_key).and_then(|decrypted| {
                HistoryRecord::deserialize(&decrypted.data, version.as_str())
            }),
            None => Err(eyre!("unknown history version {version:?}")),
        };

        match record {
            Ok(record) => Some(record),
            Err(e) => {
                warn!("failed to decode history record {}, skipping: {e}", id.0);
                None
            }
        }
    }

    /// Apply records to the history database, discarding the created `History` entries.
    ///
    /// Use this when you want the database writes but not the values. Callers that need
    /// the created entries should use [`HistoryStore::incremental_build`] directly.
    pub async fn build_all(&self, database: &Sqlite, ids: &[RecordId]) -> Result<()> {
        self.incremental_build(database, ids).try_for_each(|_| future::ready(Ok(()))).await
    }

    /// Get a list of history IDs that exist in the store
    /// Note: This currently involves loading all history into memory. This is not going to be a
    /// large amount in absolute terms, but do not all it in a hot loop.
    pub async fn history_ids(&self) -> Result<HashSet<HistoryId>> {
        let history = self.history().await?;

        let ret = HashSet::from_iter(history.iter().map(|h| match h {
            HistoryRecord::Create(h) => h.id.clone(),
            HistoryRecord::Delete(id) => id.clone(),
        }));

        Ok(ret)
    }

    pub async fn init_store(&self, db: &Sqlite) -> Result<()> {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("{spinner:.blue} {msg}")
                .unwrap()
                .with_key("eta", |state: &ProgressState, w: &mut dyn Write| {
                    write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap()
                })
                .progress_chars("#>-"),
        );
        pb.enable_steady_tick(Duration::from_millis(500));

        pb.set_message("Fetching history from old database");

        let context = current_context().await?;
        let history = db.list([], &context, None, false, true, None).await?;

        pb.set_message("Fetching history already in store");
        let store_ids = self.history_ids().await?;

        pb.set_message("Converting old history to new store");
        let mut records = Vec::new();

        for i in history {
            debug!("loaded {}", i.id);

            if store_ids.contains(&i.id) {
                debug!("skipping {} - already exists", i.id);
                continue;
            }

            if i.deleted_at.is_some() {
                records.push(HistoryRecord::Delete(i.id));
            } else {
                records.push(HistoryRecord::Create(i));
            }
        }

        pb.set_message("Writing to db");

        if !records.is_empty() {
            self.push_batch(records.into_iter()).await?;
        }

        pb.finish_with_message("Import complete");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use atuin_domain::record::{
        CmdOrigin, DecryptedData, Host, HostId, Record, RecordTag, RecordVersion,
    };
    use futures::TryStreamExt;
    use rstest::*;
    use time::Duration;
    use time::macros::datetime;

    use super::{BUILD_BATCH_SIZE, History};
    use crate::database::{Context, Sqlite};
    use crate::history::Version;
    use crate::history::store::{HistoryRecord, HistoryStore};
    use crate::record::sqlite_store::SqliteStore;
    use crate::settings::test_local_timeout;

    /// The identical `History` literal used by both async tests.
    #[fixture]
    fn sample_history() -> History {
        History {
            id: "018cd4fe81757cd2aee65cd7861f9c81".to_owned().into(),
            timestamp: datetime!(2024-01-04 00:00:00.000000 +00:00),
            duration: 100,
            exit: 0,
            command: "ls".to_owned(),
            cwd: "/".to_owned(),
            session: "018cd4fead897597852527a31c998059".to_owned(),
            cmd_origin: CmdOrigin::try_from("test:test").unwrap(),
            author: "test".to_owned(),
            intent: None,
            deleted_at: None,
            shell: None,
        }
    }

    /// A `:memory:` `SqliteStore`, its `HostId`, and the `HistoryStore` built on it.
    ///
    /// Separate `:memory:` `SqliteStore` instances are INDEPENDENT databases, so the store
    /// and the `HistoryStore` layered on it must originate from a single fixture.
    #[fixture]
    async fn stores() -> (SqliteStore, HostId, HistoryStore) {
        let store = SqliteStore::new(":memory:", test_local_timeout()).await.unwrap();
        let host_id = HostId(atuin_common::utils::uuid_v7());
        let history_store = HistoryStore::new(store.clone(), host_id, [0u8; 32].into());
        (store, host_id, history_store)
    }

    fn assert_record_roundtrip(record: &HistoryRecord, expected_bytes: &[u8]) {
        let serialized = record.serialize().expect("failed to serialize history");
        assert_eq!(serialized.0, expected_bytes);

        let deserialized = HistoryRecord::deserialize(&serialized, Version::LATEST.name())
            .expect("failed to deserialize HistoryRecord");
        assert_eq!(&deserialized, record);

        // check the snapshot too
        let deserialized = HistoryRecord::deserialize(
            &DecryptedData(Vec::from(expected_bytes)),
            Version::LATEST.name(),
        )
        .expect("failed to deserialize HistoryRecord");
        assert_eq!(&deserialized, record);
    }

    #[rstest]
    #[case::create(
        HistoryRecord::Create(History {
            id: "018cd4fe81757cd2aee65cd7861f9c81".to_owned().into(),
            timestamp: datetime!(2024-01-04 00:00:00.000000 +00:00),
            duration: 100,
            exit: 0,
            command: "ls".to_owned(),
            cwd: "/Users/ellie/src/github.com/atuinsh/atuin".to_owned(),
            session: "018cd4fead897597852527a31c998059".to_owned(),
            cmd_origin: CmdOrigin::try_from("boop:ellie").unwrap(),
            author: "ellie".to_owned(),
            intent: None,
            deleted_at: None,
            shell: Some("bash".to_owned()),
        }),
        vec![
            204, 0, 196, 153, 205, 0, 2, 156, 217, 32, 48, 49, 56, 99, 100, 52, 102, 101, 56, 49,
            55, 53, 55, 99, 100, 50, 97, 101, 101, 54, 53, 99, 100, 55, 56, 54, 49, 102, 57, 99,
            56, 49, 207, 23, 166, 251, 212, 181, 82, 0, 0, 100, 0, 162, 108, 115, 217, 41, 47, 85,
            115, 101, 114, 115, 47, 101, 108, 108, 105, 101, 47, 115, 114, 99, 47, 103, 105, 116,
            104, 117, 98, 46, 99, 111, 109, 47, 97, 116, 117, 105, 110, 115, 104, 47, 97, 116, 117,
            105, 110, 217, 32, 48, 49, 56, 99, 100, 52, 102, 101, 97, 100, 56, 57, 55, 53, 57, 55,
            56, 53, 50, 53, 50, 55, 97, 51, 49, 99, 57, 57, 56, 48, 53, 57, 170, 98, 111, 111, 112,
            58, 101, 108, 108, 105, 101, 192, 165, 101, 108, 108, 105, 101, 192, 164, 98, 97, 115,
            104,
        ]
    )]
    #[case::delete(
        HistoryRecord::Delete("018cd4fe81757cd2aee65cd7861f9c81".to_string().into()),
        vec![
            204, 1, 217, 32, 48, 49, 56, 99, 100, 52, 102, 101, 56, 49, 55, 53, 55, 99, 100, 50,
            97, 101, 101, 54, 53, 99, 100, 55, 56, 54, 49, 102, 57, 99, 56, 49,
        ]
    )]
    fn test_serialize_deserialize(#[case] record: HistoryRecord, #[case] expected_bytes: Vec<u8>) {
        assert_record_roundtrip(&record, &expected_bytes);
    }

    #[rstest]
    #[tokio::test]
    async fn test_history_skips_corrupt_records(
        #[future(awt)]
        #[from(stores)]
        parts: (SqliteStore, HostId, HistoryStore),
        #[from(sample_history)] history: History,
    ) {
        let (store, host_id, history_store) = parts;
        history_store.push(history.clone()).await.unwrap();

        // a record in the history tag encrypted with a different key - the store is corrupt,
        // or "mixed". it should be skipped, rather than breaking loading entirely.
        let corrupt = Record::builder()
            .host(Host::new(host_id))
            .version(RecordVersion::from(Version::LATEST.name()))
            .tag(RecordTag::History)
            .idx(1)
            .data(DecryptedData(vec![1, 2, 3]))
            .build();

        store.push(&corrupt.encrypt(&[1u8; 32].into())).await.unwrap();

        let records = history_store.history().await.unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0], HistoryRecord::Create(history));
    }

    #[rstest]
    #[tokio::test]
    async fn test_incremental_build_returns_created_histories(
        #[future(awt)]
        #[from(stores)]
        parts: (SqliteStore, HostId, HistoryStore),
        #[from(sample_history)] history: History,
    ) {
        let (_store, _host_id, history_store) = parts;
        // `push` returns the RECORD id (record-store id-space), distinct from
        // `history.id` (the HistoryId). This distinction is the whole bug.
        let (record_id, _) = history_store.push(history.clone()).await.unwrap();

        let db = memory_db().await;

        let created: Vec<History> =
            history_store.incremental_build(&db, &[record_id]).try_concat().await.unwrap();

        assert_eq!(created.len(), 1);
        assert_eq!(created[0], history);
    }

    async fn memory_db() -> Sqlite {
        Sqlite::new("sqlite::memory:", test_local_timeout()).await.unwrap()
    }

    fn history_n(n: usize) -> History {
        History {
            id: format!("{n:032x}").into(),
            timestamp: datetime!(2024-01-04 00:00:00.000000 +00:00) + Duration::seconds(n as i64),
            command: format!("command {n}"),
            ..sample_history()
        }
    }

    /// Filtering is `Global` in these tests, so `list` only needs a plausible context.
    fn context() -> Context {
        Context {
            session: "018cd4fead897597852527a31c998059".to_owned(),
            cwd: "/".to_owned(),
            cmd_origin: CmdOrigin::try_from("test:test").unwrap(),
            host_id: "test".to_owned(),
            git_root: None,
        }
    }

    /// Each yielded batch is one `save_bulk`, so batch sizes are the transaction shape.
    async fn batch_sizes(
        history_store: &HistoryStore,
        db: &Sqlite,
        ids: &[atuin_domain::record::RecordId],
    ) -> Vec<usize> {
        history_store
            .incremental_build(db, ids)
            .try_collect::<Vec<Vec<History>>>()
            .await
            .unwrap()
            .iter()
            .map(Vec::len)
            .collect()
    }

    #[rstest]
    #[tokio::test]
    async fn a_run_of_creates_is_one_bulk_write(
        #[future(awt)]
        #[from(stores)]
        parts: (SqliteStore, HostId, HistoryStore),
    ) {
        let (_store, _host_id, history_store) = parts;
        let db = memory_db().await;

        let mut ids = Vec::new();
        for n in 0..25 {
            let (id, _) = history_store.push(history_n(n)).await.unwrap();
            ids.push(id);
        }

        assert_eq!(batch_sizes(&history_store, &db, &ids).await, vec![25]);
        assert_eq!(db.list([], &context(), None, false, true, None).await.unwrap().len(), 25);
    }

    /// The create before a delete is flushed on its own rather than grouped with the create
    /// after it - which is what keeps the order intact.
    #[rstest]
    #[tokio::test]
    async fn interleaved_creates_and_deletes_keep_their_order(
        #[future(awt)]
        #[from(stores)]
        parts: (SqliteStore, HostId, HistoryStore),
    ) {
        let (_store, _host_id, history_store) = parts;
        let db = memory_db().await;

        let first = history_n(1);
        let (create_first, _) = history_store.push(first.clone()).await.unwrap();
        let (delete_first, _) = history_store.delete(first.id.clone()).await.unwrap();
        let (create_second, _) = history_store.push(history_n(2)).await.unwrap();

        // Three flushes: the create, the delete (which creates nothing), then the create.
        let ids = [create_first, delete_first, create_second];
        assert_eq!(batch_sizes(&history_store, &db, &ids).await, vec![1, 0, 1]);

        // Had the delete been applied before the create, `first` would still be present.
        let stored = db.list([], &context(), None, false, true, None).await.unwrap();
        assert_eq!(stored.iter().map(|h| h.command.as_str()).collect::<Vec<_>>(), vec![
            "command 2"
        ]);
    }

    #[rstest]
    #[tokio::test]
    async fn creates_are_split_into_bounded_batches(
        #[future(awt)]
        #[from(stores)]
        parts: (SqliteStore, HostId, HistoryStore),
    ) {
        let (store, _host_id, history_store) = parts;
        let db = memory_db().await;

        let total = BUILD_BATCH_SIZE.get() + 3;
        history_store
            .push_batch((0..total).map(|n| HistoryRecord::Create(history_n(n))))
            .await
            .unwrap();
        let ids: Vec<_> =
            store.all_tagged(&RecordTag::History).await.unwrap().iter().map(|r| r.id).collect();

        assert_eq!(batch_sizes(&history_store, &db, &ids).await, vec![BUILD_BATCH_SIZE.get(), 3]);
    }

    #[rstest]
    #[tokio::test]
    async fn missing_and_undecodable_ids_are_skipped(
        #[future(awt)]
        #[from(stores)]
        parts: (SqliteStore, HostId, HistoryStore),
        #[from(sample_history)] history: History,
    ) {
        let (store, host_id, history_store) = parts;
        let db = memory_db().await;

        let (good, _) = history_store.push(history.clone()).await.unwrap();

        // Encrypted with a different key: present, but undecodable.
        let corrupt = Record::builder()
            .host(Host::new(host_id))
            .version(RecordVersion::from(Version::LATEST.name()))
            .tag(RecordTag::History)
            .idx(1)
            .data(DecryptedData(vec![1, 2, 3]))
            .build();
        let corrupt_id = corrupt.id;
        store.push(&corrupt.encrypt(&[1u8; 32].into())).await.unwrap();

        let missing = atuin_domain::record::RecordId(atuin_common::utils::uuid_v7());

        let created: Vec<History> = history_store
            .incremental_build(&db, &[missing, good, corrupt_id])
            .try_concat()
            .await
            .unwrap();

        assert_eq!(created, vec![history]);
    }

    #[rstest]
    #[tokio::test]
    async fn a_database_error_aborts_the_build(
        #[future(awt)]
        #[from(stores)]
        parts: (SqliteStore, HostId, HistoryStore),
        #[from(sample_history)] history: History,
    ) {
        let (_store, _host_id, history_store) = parts;
        let (record_id, _) = history_store.push(history).await.unwrap();

        let db = memory_db().await;
        db.pool.close().await;

        assert!(history_store.build_all(&db, &[record_id]).await.is_err());
    }
}
