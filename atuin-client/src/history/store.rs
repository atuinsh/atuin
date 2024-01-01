use eyre::Result;

use crate::record::sqlite_store::SqliteStore;
use atuin_common::record::HostId;

use super::History;

#[derive(Debug)]
pub struct HistoryStore {
    pub store: SqliteStore,
    pub host_id: HostId,
    pub encryption_key: [u8; 32],
}

impl HistoryStore {
    pub fn new(store: SqliteStore, host_id: HostId, encryption_key: [u8; 32]) -> Self {
        HistoryStore {
            store,
            host_id,
            encryption_key,
        }
    }

    pub async fn push(&self, _history: &History) -> Result<()> {
        Ok(())
        /*
        * will continue this in another PR
        *
        *
        let bytes = history.serialize()?;
        let id = self
            .store
            .last(self.host_id, HISTORY_TAG)
            .await?
            .map_or(0, |p| p.idx + 1);

        let record = Record::builder()
            .host(Host::new(self.host_id))
            .version(HISTORY_VERSION.to_string())
            .tag(HISTORY_TAG.to_string())
            .idx(id)
            .data(bytes)
            .build();

        self.store
            .push(&record.encrypt::<PASETO_V4>(&self.encryption_key))
            .await?;

        Ok(())
            */
    }
}
