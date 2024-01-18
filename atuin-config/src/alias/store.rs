use eyre::{bail, eyre, Result};

use atuin_client::record::{encryption::PASETO_V4, sqlite_store::SqliteStore, store::Store};
use atuin_common::record::{Host, HostId, Record, RecordId, RecordIdx};

use super::record::{AliasId, AliasRecord, CONFIG_ALIAS_TAG, CONFIG_ALIAS_VERSION};

#[derive(Debug)]
pub struct AliasStore {
    pub store: SqliteStore,
    pub host_id: HostId,
    pub encryption_key: [u8; 32],
}

impl AliasStore {
    pub fn new(store: SqliteStore, host_id: HostId, encryption_key: [u8; 32]) -> Self {
        AliasStore {
            store,
            host_id,
            encryption_key,
        }
    }

    async fn push_record(&self, record: AliasRecord) -> Result<(RecordId, RecordIdx)> {
        let bytes = record.serialize()?;
        let idx = self
            .store
            .last(self.host_id, CONFIG_ALIAS_TAG)
            .await?
            .map_or(0, |p| p.idx + 1);

        let record = Record::builder()
            .host(Host::new(self.host_id))
            .version(CONFIG_ALIAS_VERSION.to_string())
            .tag(CONFIG_ALIAS_TAG.to_string())
            .idx(idx)
            .data(bytes)
            .build();

        let id = record.id;

        self.store
            .push(&record.encrypt::<PASETO_V4>(&self.encryption_key))
            .await?;

        Ok((id, idx))
    }
}
