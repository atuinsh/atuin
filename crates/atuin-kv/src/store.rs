use std::collections::HashSet;

use eyre::{Result, bail};

use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::{encryption::PASETO_V4, store::Store};
use atuin_common::record::{Host, HostId, Record, RecordId, RecordIdx};
use entry::KvEntry;
use record::{KV_TAG, KV_VERSION, KvRecord};

use crate::database::Database;

pub mod entry;
pub mod record;

#[derive(Debug, Clone)]
pub struct KvStore {
    pub record_store: SqliteStore,
    pub kv_db: Database,
    pub host_id: HostId,
    pub encryption_key: [u8; 32],
}

impl KvStore {
    pub fn new(
        record_store: SqliteStore,
        kv_db: Database,
        host_id: HostId,
        encryption_key: [u8; 32],
    ) -> Self {
        KvStore {
            record_store,
            kv_db,
            host_id,
            encryption_key,
        }
    }

    pub async fn set(&self, namespace: &str, key: &str, value: &str) -> Result<()> {
        let kv_record = KvRecord::builder()
            .namespace(namespace.to_string())
            .key(key.to_string())
            .value(Some(value.to_string()))
            .build();

        self.push_record(kv_record).await?;

        let kv = KvEntry::builder()
            .namespace(namespace.to_string())
            .key(key.to_string())
            .value(value.to_string())
            .build();

        self.kv_db.save(&kv).await?;

        Ok(())
    }

    pub async fn get(&self, namespace: &str, key: &str) -> Result<Option<String>> {
        let kv = self.kv_db.load(namespace, key).await?;
        Ok(kv.map(|kv| kv.value))
    }

    pub async fn delete(&self, namespace: &str, keys: &[String]) -> Result<()> {
        for key in keys {
            let record = KvRecord::builder()
                .namespace(namespace.to_string())
                .key(key.to_string())
                .value(None)
                .build();

            self.push_record(record).await?;
            self.kv_db.delete(namespace, key).await?;
        }

        Ok(())
    }

    pub async fn list(&self, namespace: Option<&str>) -> Result<Vec<KvEntry>> {
        let entries = self.kv_db.list(namespace).await?;

        Ok(entries)
    }

    async fn push_record(&self, record: KvRecord) -> Result<(RecordId, RecordIdx)> {
        let bytes = record.serialize()?;
        let idx = self
            .record_store
            .last(self.host_id, KV_TAG)
            .await?
            .map_or(0, |p| p.idx + 1);

        let record = Record::builder()
            .host(Host::new(self.host_id))
            .version(KV_VERSION.to_string())
            .tag(KV_TAG.to_string())
            .idx(idx)
            .data(bytes)
            .build();

        let id = record.id;

        self.record_store
            .push(&record.encrypt::<PASETO_V4>(&self.encryption_key))
            .await?;

        Ok((id, idx))
    }

    pub async fn build(&self) -> Result<()> {
        let mut tagged = self.record_store.all_tagged(KV_TAG).await?;
        tagged.reverse();

        let cached = self.kv_db.list(None).await?;

        let mut visited = HashSet::new();

        // Iterate through all KV records from newest to oldest;
        // only visit each KV once, inserting or deleting based on the first time we see it
        for record in tagged {
            let decrypted = match record.version.as_str() {
                "v0" | KV_VERSION => record.decrypt::<PASETO_V4>(&self.encryption_key)?,
                version => bail!("unknown version {version:?}"),
            };

            let kv = KvRecord::deserialize(&decrypted.data, &decrypted.version)?;
            let uniq_id = format!("{}.{}", kv.namespace, kv.key);

            if visited.insert(uniq_id) {
                match kv.value {
                    Some(value) => {
                        self.kv_db
                            .save(
                                &KvEntry::builder()
                                    .namespace(kv.namespace.clone())
                                    .key(kv.key.clone())
                                    .value(value)
                                    .build(),
                            )
                            .await?;
                    }
                    None => {
                        self.kv_db
                            .delete(kv.namespace.as_str(), kv.key.as_str())
                            .await?;
                    }
                }
            }
        }

        // Any KVs that were in the cache but not in the tagged list should be deleted;
        // this should never happen in practice since the cache is always built from the tagged list,
        // but just in case because ** S O F T W A R E **
        for kv in cached {
            if !visited.contains(&format!("{}.{}", kv.namespace, kv.key)) {
                self.kv_db
                    .delete(kv.namespace.as_str(), kv.key.as_str())
                    .await?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> Result<KvStore> {
        let record_store = SqliteStore::new("sqlite::memory:", 1.0).await.unwrap();
        let kv_db = Database::new("sqlite::memory:", 1.0).await.unwrap();
        let host_id = atuin_common::record::HostId(atuin_common::utils::uuid_v7());
        let encryption_key = [0; 32];
        Ok(KvStore::new(record_store, kv_db, host_id, encryption_key))
    }

    #[tokio::test]
    async fn test_kv_store() -> Result<()> {
        let store = setup().await?;

        store.set("test", "key", "value").await.unwrap();
        let value = store.get("test", "key").await.unwrap();
        assert_eq!(value, Some("value".to_string()));

        let records = store.record_store.all_tagged(KV_TAG).await?;
        assert_eq!(records.len(), 1);

        let list = store.list(Some("test")).await.unwrap();
        let expected = vec![
            KvEntry::builder()
                .namespace("test".to_string())
                .key("key".to_string())
                .value("value".to_string())
                .build(),
        ];
        assert_eq!(list, expected);

        let ns_list = store.list(None).await.unwrap();
        assert_eq!(ns_list, expected);

        store.delete("test", &["key".to_string()]).await.unwrap();
        let value = store.get("test", "key").await.unwrap();
        assert_eq!(value, None);

        let records = store.record_store.all_tagged(KV_TAG).await?;
        assert_eq!(records.len(), 2);

        Ok(())
    }
}
