use eyre::{Result, eyre};

use atuin_client::record::sqlite_store::SqliteStore;
use atuin_common::encryption::paseto_v4;
use atuin_domain::record::{Host, HostId, Record, RecordId, RecordIdx, RecordTag, RecordVersion};
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
    pub fn new(store: SqliteStore, host_id: HostId, encryption_key: paseto_v4::Key) -> Self {
        ScriptStore {
            store,
            host_id,
            encryption_key,
        }
    }

    async fn push_record(&self, record: ScriptRecord) -> Result<(RecordId, RecordIdx)> {
        let bytes = record.serialize()?;
        let idx = self
            .store
            .last(self.host_id, &RecordTag::Script)
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

        self.store
            .push(&record.encrypt(&self.encryption_key))
            .await?;

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

        for record in records.into_iter() {
            // Skip records we can't decrypt or decode, rather than failing the entire build.
            let script = match record.version {
                RecordVersion::V0 => record.decrypt(&self.encryption_key).and_then(|decrypted| {
                    ScriptRecord::deserialize(&decrypted.data, &RecordVersion::V0)
                }),
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
            // library code that may run under the TUI or shell hooks, so no stderr here
            tracing::warn!(
                "skipped {skipped} script records that could not be decrypted or decoded"
            );
        }

        Ok(ret)
    }

    pub async fn build(&self, database: Database) -> Result<()> {
        // Clear existing data before replaying all records from the store.
        // Without this, stale rows can cause unique constraint violations
        // when records are replayed (eg name conflicts from renamed scripts).
        database.clear().await?;

        // Get all the scripts from the store - they are already sorted by timestamp
        let scripts = self.scripts().await?;

        for script in scripts {
            match script {
                ScriptRecord::Create(script) => {
                    database.save(&script).await?;
                }
                ScriptRecord::Update(script) => database.update(&script).await?,
                ScriptRecord::Delete(id) => database.delete(&id.to_string()).await?,
            }
        }

        Ok(())
    }
}
