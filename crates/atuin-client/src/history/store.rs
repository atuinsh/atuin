use std::{collections::HashSet, fmt::Write, time::Duration};

use eyre::{Result, bail, eyre};
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use rmp::decode::Bytes;

use crate::{
    database::{Database, current_context},
    record::{encryption::PASETO_V4, sqlite_store::SqliteStore, store::Store},
};
use atuin_common::record::{DecryptedData, Host, HostId, Record, RecordId, RecordIdx};

use super::{HISTORY_TAG, HISTORY_VERSION, HISTORY_VERSION_V0, History, HistoryId};

#[derive(Debug, Clone)]
pub struct HistoryStore {
    pub store: SqliteStore,
    pub host_id: HostId,
    pub encryption_key: [u8; 32],
}

#[derive(Debug, Eq, PartialEq, Clone)]
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
        use rmp::encode;

        let mut output = vec![];

        match self {
            HistoryRecord::Create(history) => {
                // 0 -> a history create
                encode::write_u8(&mut output, 0)?;

                let bytes = history.serialize()?;

                encode::write_bin(&mut output, &bytes.0)?;
            }
            HistoryRecord::Delete(id) => {
                // 1 -> a history delete
                encode::write_u8(&mut output, 1)?;
                encode::write_str(&mut output, id.0.as_str())?;
            }
        };

        Ok(DecryptedData(output))
    }

    pub fn deserialize(bytes: &DecryptedData, version: &str) -> Result<Self> {
        use rmp::decode;

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

                Ok(HistoryRecord::Create(record))
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

                Ok(HistoryRecord::Delete(id.to_string().into()))
            }

            n => {
                bail!("unknown HistoryRecord type {n}")
            }
        }
    }
}

impl HistoryStore {
    pub fn new(store: SqliteStore, host_id: HostId, encryption_key: [u8; 32]) -> Self {
        HistoryStore {
            store,
            host_id,
            encryption_key,
        }
    }

    async fn push_record(&self, record: HistoryRecord) -> Result<(RecordId, RecordIdx)> {
        let bytes = record.serialize()?;
        let idx = self
            .store
            .last(self.host_id, HISTORY_TAG)
            .await?
            .map_or(0, |p| p.idx + 1);

        let record = Record::builder()
            .host(Host::new(self.host_id))
            .version(HISTORY_VERSION.to_string())
            .tag(HISTORY_TAG.to_string())
            .idx(idx)
            .data(bytes)
            .build();

        let id = record.id;

        self.store
            .push(&record.encrypt::<PASETO_V4>(&self.encryption_key))
            .await?;

        Ok((id, idx))
    }

    async fn push_batch(&self, records: impl Iterator<Item = HistoryRecord>) -> Result<()> {
        let mut ret = Vec::new();

        let idx = self
            .store
            .last(self.host_id, HISTORY_TAG)
            .await?
            .map_or(0, |p| p.idx + 1);

        // Could probably _also_ do this as an iterator, but let's see how this is for now.
        // optimizing for minimal sqlite transactions, this code can be optimised later
        for (n, record) in records.enumerate() {
            let bytes = record.serialize()?;

            let record = Record::builder()
                .host(Host::new(self.host_id))
                .version(HISTORY_VERSION.to_string())
                .tag(HISTORY_TAG.to_string())
                .idx(idx + n as u64)
                .data(bytes)
                .build();

            let record = record.encrypt::<PASETO_V4>(&self.encryption_key);

            ret.push(record);
        }

        self.store.push_batch(ret.iter()).await?;

        Ok(())
    }

    pub async fn delete(&self, id: HistoryId) -> Result<(RecordId, RecordIdx)> {
        let record = HistoryRecord::Delete(id);

        self.push_record(record).await
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
        let records = self.store.all_tagged(HISTORY_TAG).await?;
        let mut ret = Vec::with_capacity(records.len());

        for record in records.into_iter() {
            let hist = match record.version.as_str() {
                HISTORY_VERSION_V0 | HISTORY_VERSION => {
                    let version = record.version.clone();
                    let decrypted = record.decrypt::<PASETO_V4>(&self.encryption_key)?;

                    HistoryRecord::deserialize(&decrypted.data, version.as_str())
                }
                version => bail!("unknown history version {version:?}"),
            }?;

            ret.push(hist);
        }

        Ok(ret)
    }

    pub async fn build(&self, database: &dyn Database) -> Result<()> {
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
        database.delete_rows(&deletes).await?;

        Ok(())
    }

    pub async fn incremental_build(&self, database: &dyn Database, ids: &[RecordId]) -> Result<()> {
        for id in ids {
            let record = self.store.get(*id).await;

            let record = match record {
                Ok(record) => record,
                _ => {
                    continue;
                }
            };

            if record.tag != HISTORY_TAG {
                continue;
            }

            let version = record.version.clone();
            let decrypted = record.decrypt::<PASETO_V4>(&self.encryption_key)?;
            let record = match version.as_str() {
                HISTORY_VERSION_V0 | HISTORY_VERSION => {
                    HistoryRecord::deserialize(&decrypted.data, version.as_str())?
                }
                version => bail!("unknown history version {version:?}"),
            };

            match record {
                HistoryRecord::Create(h) => {
                    // TODO: benchmark CPU time/memory tradeoff of batch commit vs one at a time
                    database.save(&h).await?;
                }
                HistoryRecord::Delete(id) => {
                    database.delete_rows(&[id]).await?;
                }
            }
        }

        Ok(())
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

    pub async fn init_store(&self, db: &impl Database) -> Result<()> {
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
        let history = db.list(&[], &context, None, false, true).await?;

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
    use atuin_common::record::DecryptedData;
    use time::macros::datetime;

    use crate::history::{HISTORY_VERSION, store::HistoryRecord};

    use super::History;

    #[test]
    fn test_serialize_deserialize_create() {
        let bytes = [
            204, 0, 196, 147, 205, 0, 1, 154, 217, 32, 48, 49, 56, 99, 100, 52, 102, 101, 56, 49,
            55, 53, 55, 99, 100, 50, 97, 101, 101, 54, 53, 99, 100, 55, 56, 54, 49, 102, 57, 99,
            56, 49, 207, 23, 166, 251, 212, 181, 82, 0, 0, 100, 0, 162, 108, 115, 217, 41, 47, 85,
            115, 101, 114, 115, 47, 101, 108, 108, 105, 101, 47, 115, 114, 99, 47, 103, 105, 116,
            104, 117, 98, 46, 99, 111, 109, 47, 97, 116, 117, 105, 110, 115, 104, 47, 97, 116, 117,
            105, 110, 217, 32, 48, 49, 56, 99, 100, 52, 102, 101, 97, 100, 56, 57, 55, 53, 57, 55,
            56, 53, 50, 53, 50, 55, 97, 51, 49, 99, 57, 57, 56, 48, 53, 57, 170, 98, 111, 111, 112,
            58, 101, 108, 108, 105, 101, 192, 165, 101, 108, 108, 105, 101,
        ];

        let history = History {
            id: "018cd4fe81757cd2aee65cd7861f9c81".to_owned().into(),
            timestamp: datetime!(2024-01-04 00:00:00.000000 +00:00),
            duration: 100,
            exit: 0,
            command: "ls".to_owned(),
            cwd: "/Users/ellie/src/github.com/atuinsh/atuin".to_owned(),
            session: "018cd4fead897597852527a31c998059".to_owned(),
            hostname: "boop:ellie".to_owned(),
            author: "ellie".to_owned(),
            intent: None,
            deleted_at: None,
        };

        let record = HistoryRecord::Create(history);

        let serialized = record.serialize().expect("failed to serialize history");
        assert_eq!(serialized.0, bytes);

        let deserialized = HistoryRecord::deserialize(&serialized, HISTORY_VERSION)
            .expect("failed to deserialize HistoryRecord");
        assert_eq!(deserialized, record);

        // check the snapshot too
        let deserialized =
            HistoryRecord::deserialize(&DecryptedData(Vec::from(bytes)), HISTORY_VERSION)
                .expect("failed to deserialize HistoryRecord");
        assert_eq!(deserialized, record);
    }

    #[test]
    fn test_serialize_deserialize_delete() {
        let bytes = [
            204, 1, 217, 32, 48, 49, 56, 99, 100, 52, 102, 101, 56, 49, 55, 53, 55, 99, 100, 50,
            97, 101, 101, 54, 53, 99, 100, 55, 56, 54, 49, 102, 57, 99, 56, 49,
        ];
        let record = HistoryRecord::Delete("018cd4fe81757cd2aee65cd7861f9c81".to_string().into());

        let serialized = record.serialize().expect("failed to serialize history");
        assert_eq!(serialized.0, bytes);

        let deserialized = HistoryRecord::deserialize(&serialized, HISTORY_VERSION)
            .expect("failed to deserialize HistoryRecord");
        assert_eq!(deserialized, record);

        let deserialized =
            HistoryRecord::deserialize(&DecryptedData(Vec::from(bytes)), HISTORY_VERSION)
                .expect("failed to deserialize HistoryRecord");
        assert_eq!(deserialized, record);
    }
}
