use eyre::{bail, eyre, Result};
use rmp::decode::Bytes;

use crate::record::{encryption::PASETO_V4, sqlite_store::SqliteStore, store::Store};
use atuin_common::record::{DecryptedData, Host, HostId, Record, RecordIdx};

use super::{History, HISTORY_TAG, HISTORY_VERSION};

#[derive(Debug)]
pub struct HistoryStore {
    pub store: SqliteStore,
    pub host_id: HostId,
    pub encryption_key: [u8; 32],
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum HistoryRecord {
    Create(History), // Create a history record
    Delete(String),  // Delete a history record, identified by ID
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
                encode::write_str(&mut output, id)?;
            }
        };

        Ok(DecryptedData(output))
    }

    pub fn deserialize(bytes: &[u8], version: &str) -> Result<Self> {
        use rmp::decode;

        fn error_report<E: std::fmt::Debug>(err: E) -> eyre::Report {
            eyre!("{err:?}")
        }

        let mut bytes = Bytes::new(bytes);

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

                Ok(HistoryRecord::Delete(id.to_string()))
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

    async fn push_record(&self, record: HistoryRecord) -> Result<RecordIdx> {
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

        self.store
            .push(&record.encrypt::<PASETO_V4>(&self.encryption_key))
            .await?;

        Ok(idx)
    }

    pub async fn delete(&self, id: String) -> Result<RecordIdx> {
        let record = HistoryRecord::Delete(id);

        self.push_record(record).await
    }

    pub async fn push(&self, history: History) -> Result<RecordIdx> {
        // TODO(ellie): move the history store to its own file
        // it's tiny rn so fine as is
        let record = HistoryRecord::Create(history);

        self.push_record(record).await
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use crate::history::{store::HistoryRecord, HISTORY_VERSION};

    use super::History;

    #[test]
    fn test_serialize_deserialize_create() {
        let bytes = [
            204, 0, 196, 141, 205, 0, 0, 153, 217, 32, 48, 49, 56, 99, 100, 52, 102, 101, 56, 49,
            55, 53, 55, 99, 100, 50, 97, 101, 101, 54, 53, 99, 100, 55, 56, 54, 49, 102, 57, 99,
            56, 49, 207, 23, 166, 251, 212, 181, 82, 0, 0, 100, 0, 162, 108, 115, 217, 41, 47, 85,
            115, 101, 114, 115, 47, 101, 108, 108, 105, 101, 47, 115, 114, 99, 47, 103, 105, 116,
            104, 117, 98, 46, 99, 111, 109, 47, 97, 116, 117, 105, 110, 115, 104, 47, 97, 116, 117,
            105, 110, 217, 32, 48, 49, 56, 99, 100, 52, 102, 101, 97, 100, 56, 57, 55, 53, 57, 55,
            56, 53, 50, 53, 50, 55, 97, 51, 49, 99, 57, 57, 56, 48, 53, 57, 170, 98, 111, 111, 112,
            58, 101, 108, 108, 105, 101, 192,
        ];

        let history = History {
            id: "018cd4fe81757cd2aee65cd7861f9c81".to_owned(),
            timestamp: datetime!(2024-01-04 00:00:00.000000 +00:00),
            duration: 100,
            exit: 0,
            command: "ls".to_owned(),
            cwd: "/Users/ellie/src/github.com/atuinsh/atuin".to_owned(),
            session: "018cd4fead897597852527a31c998059".to_owned(),
            hostname: "boop:ellie".to_owned(),
            deleted_at: None,
        };

        let record = HistoryRecord::Create(history);

        let serialized = record.serialize().expect("failed to serialize history");
        assert_eq!(serialized.0, bytes);

        let deserialized = HistoryRecord::deserialize(&serialized.0, HISTORY_VERSION)
            .expect("failed to deserialize HistoryRecord");
        assert_eq!(deserialized, record);

        // check the snapshot too
        let deserialized = HistoryRecord::deserialize(&bytes, HISTORY_VERSION)
            .expect("failed to deserialize HistoryRecord");
        assert_eq!(deserialized, record);
    }

    #[test]
    fn test_serialize_deserialize_delete() {
        let bytes = [
            204, 1, 217, 32, 48, 49, 56, 99, 100, 52, 102, 101, 56, 49, 55, 53, 55, 99, 100, 50,
            97, 101, 101, 54, 53, 99, 100, 55, 56, 54, 49, 102, 57, 99, 56, 49,
        ];
        let record = HistoryRecord::Delete("018cd4fe81757cd2aee65cd7861f9c81".to_string());

        let serialized = record.serialize().expect("failed to serialize history");
        assert_eq!(serialized.0, bytes);

        let deserialized = HistoryRecord::deserialize(&serialized.0, HISTORY_VERSION)
            .expect("failed to deserialize HistoryRecord");
        assert_eq!(deserialized, record);

        let deserialized = HistoryRecord::deserialize(&bytes, HISTORY_VERSION)
            .expect("failed to deserialize HistoryRecord");
        assert_eq!(deserialized, record);
    }
}
