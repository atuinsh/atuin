use eyre::{Result, bail};

use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::{encryption::PASETO_V4, store::Store};
use atuin_common::record::{Host, HostId, Record, RecordId, RecordIdx};
use record::ScriptRecord;
use script::{SCRIPT_TAG, SCRIPT_VERSION, Script};

use crate::database::Database;

pub mod record;
pub mod script;

#[derive(Debug, Clone)]
pub struct ScriptStore {
    pub store: SqliteStore,
    pub host_id: HostId,
    pub encryption_key: [u8; 32],
}

impl ScriptStore {
    pub fn new(store: SqliteStore, host_id: HostId, encryption_key: [u8; 32]) -> Self {
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
            .last(self.host_id, SCRIPT_TAG)
            .await?
            .map_or(0, |p| p.idx + 1);

        let record = Record::builder()
            .host(Host::new(self.host_id))
            .version(SCRIPT_VERSION.to_string())
            .tag(SCRIPT_TAG.to_string())
            .idx(idx)
            .data(bytes)
            .build();

        let id = record.id;

        self.store
            .push(&record.encrypt::<PASETO_V4>(&self.encryption_key))
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
        let records = self.store.all_tagged(SCRIPT_TAG).await?;
        let mut ret = Vec::with_capacity(records.len());

        for record in records.into_iter() {
            let script = match record.version.as_str() {
                SCRIPT_VERSION => {
                    let decrypted = record.decrypt::<PASETO_V4>(&self.encryption_key)?;

                    ScriptRecord::deserialize(&decrypted.data, SCRIPT_VERSION)
                }
                version => bail!("unknown history version {version:?}"),
            }?;

            ret.push(script);
        }

        Ok(ret)
    }

    pub async fn build(&self, database: Database) -> Result<()> {
        // Get all the scripts from the database - they are already sorted by timestamp
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
