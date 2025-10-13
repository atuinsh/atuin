use eyre::{Result, bail, eyre};
use rmp::decode::Bytes;

use crate::record::{encryption::PASETO_V4, sqlite_store::SqliteStore, store::Store};
use atuin_common::record::{DecryptedData, Host, HostId, Record, RecordId, RecordIdx};

pub const TAG_VERSION: &str = "v0";
pub const TAG_TAG: &str = "tag";

#[derive(Debug, Clone)]
pub struct TagStore {
    pub store: SqliteStore,
    pub host_id: HostId,
    pub encryption_key: [u8; 32],
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum TagRecord {
    Tag(String, String),   // Tag a command (command, tag)
    Untag(String, String), // Remove tag from a command (command, tag)
}

impl TagRecord {
    pub fn serialize(&self) -> Result<DecryptedData> {
        use rmp::encode;

        let mut output = vec![];

        match self {
            TagRecord::Tag(command, tag) => {
                // 0 -> tag a command
                encode::write_u8(&mut output, 0)?;
                encode::write_str(&mut output, command)?;
                encode::write_str(&mut output, tag)?;
            }
            TagRecord::Untag(command, tag) => {
                // 1 -> untag a command
                encode::write_u8(&mut output, 1)?;
                encode::write_str(&mut output, command)?;
                encode::write_str(&mut output, tag)?;
            }
        };

        Ok(DecryptedData(output))
    }

    pub fn deserialize(bytes: &DecryptedData, _version: &str) -> Result<Self> {
        use rmp::decode;

        fn error_report<E: std::fmt::Debug>(err: E) -> eyre::Report {
            eyre!("{err:?}")
        }

        let mut bytes = Bytes::new(&bytes.0);

        let record_type = decode::read_u8(&mut bytes).map_err(error_report)?;

        match record_type {
            // 0 -> TagRecord::Tag
            0 => {
                let bytes = bytes.remaining_slice();
                let (command, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
                let (tag, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;

                if !bytes.is_empty() {
                    bail!("trailing bytes decoding TagRecord::Tag - malformed? got {bytes:?}");
                }

                Ok(TagRecord::Tag(command.to_string(), tag.to_string()))
            }

            // 1 -> TagRecord::Untag
            1 => {
                let bytes = bytes.remaining_slice();
                let (command, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
                let (tag, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;

                if !bytes.is_empty() {
                    bail!("trailing bytes decoding TagRecord::Untag - malformed? got {bytes:?}");
                }

                Ok(TagRecord::Untag(command.to_string(), tag.to_string()))
            }

            n => {
                bail!("unknown TagRecord type {n}")
            }
        }
    }
}

impl TagStore {
    pub fn new(store: SqliteStore, host_id: HostId, encryption_key: [u8; 32]) -> Self {
        TagStore {
            store,
            host_id,
            encryption_key,
        }
    }

    async fn push_record(&self, record: TagRecord) -> Result<(RecordId, RecordIdx)> {
        let bytes = record.serialize()?;
        let idx = self
            .store
            .last(self.host_id, TAG_TAG)
            .await?
            .map_or(0, |p| p.idx + 1);

        let record = Record::builder()
            .host(Host::new(self.host_id))
            .version(TAG_VERSION.to_string())
            .tag(TAG_TAG.to_string())
            .idx(idx)
            .data(bytes)
            .build();

        let id = record.id;

        self.store
            .push(&record.encrypt::<PASETO_V4>(&self.encryption_key))
            .await?;

        Ok((id, idx))
    }

    pub async fn tag(&self, command: String, tag: String) -> Result<(RecordId, RecordIdx)> {
        let record = TagRecord::Tag(command, tag);
        self.push_record(record).await
    }

    pub async fn untag(&self, command: String, tag: String) -> Result<(RecordId, RecordIdx)> {
        let record = TagRecord::Untag(command, tag);
        self.push_record(record).await
    }

    pub async fn all(&self) -> Result<Vec<TagRecord>> {
        let records = self.store.all_tagged(TAG_TAG).await?;
        let mut ret = Vec::with_capacity(records.len());

        for record in records.into_iter() {
            let tag_record = match record.version.as_str() {
                TAG_VERSION => {
                    let decrypted = record.decrypt::<PASETO_V4>(&self.encryption_key)?;
                    TagRecord::deserialize(&decrypted.data, TAG_VERSION)
                }
                version => bail!("unknown tag version {version:?}"),
            }?;

            ret.push(tag_record);
        }

        Ok(ret)
    }

    /// Rebuild the local command_tags table from synced tag records
    /// This should be called after syncing to apply remote tag changes
    pub async fn build(&self, pool: &sqlx::SqlitePool) -> Result<()> {
        let records = self.all().await?;

        for record in records {
            match record {
                TagRecord::Tag(command, tag) => {
                    let now = time::OffsetDateTime::now_utc().unix_timestamp_nanos() as i64;
                    sqlx::query(
                        "INSERT OR IGNORE INTO command_tags (command, tag, created_at) VALUES (?1, ?2, ?3)"
                    )
                    .bind(&command)
                    .bind(&tag)
                    .bind(now)
                    .execute(pool)
                    .await?;
                }
                TagRecord::Untag(command, tag) => {
                    sqlx::query("DELETE FROM command_tags WHERE command = ?1 AND tag = ?2")
                        .bind(&command)
                        .bind(&tag)
                        .execute(pool)
                        .await?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atuin_common::record::DecryptedData;

    #[test]
    fn test_serialize_deserialize_tag() {
        let record = TagRecord::Tag("cargo build".to_string(), "work".to_string());

        let serialized = record.serialize().expect("failed to serialize TagRecord::Tag");
        let deserialized = TagRecord::deserialize(&serialized, TAG_VERSION)
            .expect("failed to deserialize TagRecord::Tag");

        assert_eq!(deserialized, record);
    }

    #[test]
    fn test_serialize_deserialize_untag() {
        let record = TagRecord::Untag("cargo build".to_string(), "work".to_string());

        let serialized = record.serialize().expect("failed to serialize TagRecord::Untag");
        let deserialized = TagRecord::deserialize(&serialized, TAG_VERSION)
            .expect("failed to deserialize TagRecord::Untag");

        assert_eq!(deserialized, record);
    }

    #[test]
    fn test_tag_untag_roundtrip() {
        // Test that we can serialize and deserialize both tag types correctly
        let tag_record = TagRecord::Tag("cargo build".to_string(), "work".to_string());
        let untag_record = TagRecord::Untag("cargo build".to_string(), "work".to_string());

        let tag_serialized = tag_record.serialize().expect("failed to serialize");
        let untag_serialized = untag_record.serialize().expect("failed to serialize");

        // Deserialize and verify
        let tag_deserialized = TagRecord::deserialize(&tag_serialized, TAG_VERSION)
            .expect("failed to deserialize tag");
        let untag_deserialized = TagRecord::deserialize(&untag_serialized, TAG_VERSION)
            .expect("failed to deserialize untag");

        assert_eq!(tag_deserialized, tag_record);
        assert_eq!(untag_deserialized, untag_record);
    }
}
