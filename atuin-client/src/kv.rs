use eyre::{bail, eyre, Result};

use crate::record::store::Store;
use crate::settings::Settings;

const KV_VERSION: &str = "v0";
const KV_TAG: &str = "kv";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvRecord {
    pub namespace: String,
    pub key: String,
    pub value: String,
}

impl KvRecord {
    pub fn serialize(&self) -> Result<Vec<u8>> {
        use rmp::encode;

        let mut output = vec![];

        // INFO: ensure this is updated when adding new fields
        encode::write_array_len(&mut output, 3)?;

        encode::write_str(&mut output, &self.namespace)?;
        encode::write_str(&mut output, &self.key)?;
        encode::write_str(&mut output, &self.value)?;

        Ok(output)
    }

    pub fn deserialize(data: &[u8]) -> Result<Self> {
        use rmp::decode;

        fn error_report<E: std::fmt::Debug>(err: E) -> eyre::Report {
            eyre!("{err:?}")
        }

        let mut bytes = decode::Bytes::new(data);

        let nfields = decode::read_array_len(&mut bytes).map_err(error_report)?;
        if !(3..=3).contains(&nfields) {
            bail!("malformed kv record")
        }

        let bytes = bytes.remaining_slice();

        let (namespace, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
        let (key, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
        let (value, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;

        if !bytes.is_empty() {
            bail!("trailing bytes in encoded kvrecord. malformed")
        }

        Ok(KvRecord {
            namespace: namespace.to_owned(),
            key: key.to_owned(),
            value: value.to_owned(),
        })
    }
}

pub struct KvStore;

impl Default for KvStore {
    fn default() -> Self {
        Self::new()
    }
}

impl KvStore {
    // will want to init the actual kv store when that is done
    pub fn new() -> KvStore {
        KvStore {}
    }

    pub async fn set(
        &self,
        store: &mut (impl Store + Send + Sync),
        namespace: &str,
        key: &str,
        value: &str,
    ) -> Result<()> {
        let host_id = Settings::host_id().expect("failed to get host_id");

        let record = KvRecord {
            namespace: namespace.to_string(),
            key: key.to_string(),
            value: value.to_string(),
        };

        let bytes = record.serialize()?;

        let parent = store
            .last(host_id.as_str(), KV_TAG)
            .await?
            .map(|entry| entry.id);

        let record = atuin_common::record::Record::builder()
            .host(host_id)
            .version(KV_VERSION.to_string())
            .tag(KV_TAG.to_string())
            .parent(parent)
            .data(bytes)
            .build();

        store.push(&record).await?;

        Ok(())
    }

    // TODO: setup an actual kv store, rebuild func, and do not pass the main store in here as
    // well.
    pub async fn get(
        &self,
        store: &impl Store,
        namespace: &str,
        key: &str,
    ) -> Result<Option<KvRecord>> {
        // TODO: don't load this from disk so much
        let host_id = Settings::host_id().expect("failed to get host_id");

        // Currently, this is O(n). When we have an actual KV store, it can be better
        // Just a poc for now!

        // iterate records to find the value we want
        // start at the end, so we get the most recent version
        let Some(mut record) = store.last(host_id.as_str(), KV_TAG).await? else {
            return Ok(None);
        };

        loop {
            let kv = KvRecord::deserialize(&record.data)?;
            if kv.key == key && kv.namespace == namespace {
                return Ok(Some(kv));
            }

            if let Some(parent) = record.parent {
                record = store.get(parent.as_str()).await?;
            } else {
                break;
            }
        }

        // if we get here, then... we didn't find the record with that key :(
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::KvRecord;

    #[test]
    fn encode_decode() {
        let kv = KvRecord {
            namespace: "foo".to_owned(),
            key: "bar".to_owned(),
            value: "baz".to_owned(),
        };
        let snapshot = [
            0x93, 0xa3, b'f', b'o', b'o', 0xa3, b'b', b'a', b'r', 0xa3, b'b', b'a', b'z',
        ];

        let encoded = kv.serialize().unwrap();
        let decoded = KvRecord::deserialize(&encoded).unwrap();

        assert_eq!(encoded, &snapshot);
        assert_eq!(decoded, kv);
    }
}
