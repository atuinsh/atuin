use atuin_client::record::sqlite_store::SqliteStore;
use atuin_common::encryption::paseto_v4;
use atuin_common::userio;
use atuin_domain::record::{
    Host, HostId, Record, RecordId, RecordIdx, RecordSeriesKey, RecordTag, RecordVersion,
};
use eyre::{Result, eyre};
use record::ScriptRecord;
use script::Script;

use crate::database::Database;

pub mod record;
pub mod script;

#[derive(Debug, Clone)]
pub struct ScriptStore {
    pub store: SqliteStore,
    pub host_id: HostId,
    pub encryption_key: paseto_v4::Key,
}

impl ScriptStore {
    #[must_use]
    pub fn new(store: SqliteStore, host_id: HostId, encryption_key: paseto_v4::Key) -> Self {
        Self {
            store,
            host_id,
            encryption_key,
        }
    }

    async fn push_record(&self, record: ScriptRecord) -> Result<(RecordId, RecordIdx)> {
        let bytes = record.serialize()?;
        let idx = self
            .store
            .last(&RecordSeriesKey::new(self.host_id, RecordTag::Script))
            .await?
            .map_or(0, |p| p.idx + 1);

        let record = Record::builder()
            .host(Host::new(self.host_id))
            .version(RecordVersion::V0)
            .tag(RecordTag::Script)
            .idx(idx)
            .data(bytes)
            .build();

        let id = record.id;

        self.store.push(&record.encrypt(&self.encryption_key)).await?;

        Ok((id, idx))
    }

    pub async fn create(&self, script: Script) -> Result<()> {
        let record = ScriptRecord::Create(script);
        self.push_record(record).await?;
        Ok(())
    }

    pub async fn update(&self, script: Script) -> Result<()> {
        let record = ScriptRecord::Update(script);
        self.push_record(record).await?;
        Ok(())
    }

    pub async fn delete(&self, script_id: uuid::Uuid) -> Result<()> {
        let record = ScriptRecord::Delete(script_id);
        self.push_record(record).await?;
        Ok(())
    }

    pub async fn scripts(&self) -> Result<Vec<ScriptRecord>> {
        let records = self.store.all_tagged(&RecordTag::Script).await?;
        let mut ret = Vec::with_capacity(records.len());
        let mut skipped = 0;

        for record in records {
            // Skip records we can't decrypt or decode, rather than failing the entire build.
            let script = match record.version {
                RecordVersion::V0 => {
                    record.decrypt(&self.encryption_key).map_err(Into::into).and_then(|decrypted| {
                        ScriptRecord::deserialize(&decrypted.data, &RecordVersion::V0)
                    })
                }
                ref version => Err(eyre!("unknown script version {version:?}")),
            };

            match script {
                Ok(script) => ret.push(script),
                Err(e) => {
                    tracing::warn!("failed to decode script record, skipping: {e}");
                    skipped += 1;
                }
            }
        }

        if skipped > 0 {
            userio::log::warn!("scripts-records-skipped", skipped = skipped);
        }

        Ok(ret)
    }

    /// Rebuild `database` from every script record in the store.
    pub async fn build(&self, database: &Database) -> Result<()> {
        // Already sorted by timestamp.
        let scripts = self.scripts().await?;
        database.replay(&scripts).await?;

        Ok(())
    }
}
