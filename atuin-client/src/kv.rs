use atuin_common::record::DecryptedData;
use eyre::{bail, ensure, eyre, Result};

use crate::record::encryption::PASETO_V4;
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
    pub fn serialize(&self) -> Result<DecryptedData> {
        use rmp::encode;

        let mut output = vec![];

        // INFO: ensure this is updated when adding new fields
        encode::write_array_len(&mut output, 3)?;

        encode::write_str(&mut output, &self.namespace)?;
        encode::write_str(&mut output, &self.key)?;
        encode::write_str(&mut output, &self.value)?;

        Ok(DecryptedData(output))
    }

    pub fn deserialize(data: &DecryptedData, version: &str) -> Result<Self> {
        use rmp::decode;

        fn error_report<E: std::fmt::Debug>(err: E) -> eyre::Report {
            eyre!("{err:?}")
        }

        match version {
            KV_VERSION => {
                let mut bytes = decode::Bytes::new(&data.0);

                let nfields = decode::read_array_len(&mut bytes).map_err(error_report)?;
                ensure!(nfields == 3, "too many entries in v0 kv record");

                let bytes = bytes.remaining_slice();

                let (namespace, bytes) =
                    decode::read_str_from_slice(bytes).map_err(error_report)?;
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
            _ => {
                bail!("unknown version {version:?}")
            }
        }
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
        encryption_key: &[u8; 32],
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

        store
            .push(&record.encrypt::<PASETO_V4>(encryption_key))
            .await?;

        Ok(())
    }

    // TODO: setup an actual kv store, rebuild func, and do not pass the main store in here as
    // well.
    pub async fn get(
        &self,
        store: &impl Store,
        encryption_key: &[u8; 32],
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
            let decrypted = match record.version.as_str() {
                KV_VERSION => record.decrypt::<PASETO_V4>(encryption_key)?,
                version => bail!("unknown version {version:?}"),
            };

            let kv = KvRecord::deserialize(&decrypted.data, &decrypted.version)?;
            if kv.key == key && kv.namespace == namespace {
                return Ok(Some(kv));
            }

            if let Some(parent) = decrypted.parent {
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
    use super::{KvRecord, KV_VERSION};

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
        let decoded = KvRecord::deserialize(&encoded, KV_VERSION).unwrap();

        assert_eq!(encoded.0, &snapshot);
        assert_eq!(decoded, kv);
    }
}
