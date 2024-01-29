use atuin_common::record::HostId;
use clap::Args;
use eyre::{bail, Context, Result};
use uuid::Uuid;

use atuin_client::{
    database::Database,
    encryption,
    history::store::HistoryStore,
    record::{sqlite_store::SqliteStore, sync},
    record::{store::Store, sync::Operation},
    settings::Settings,
};

#[derive(Args, Debug)]
pub struct Push {
    #[arg(long, short)]
    pub tag: Option<String>,

    #[arg(long)]
    pub host: Option<Uuid>,
}

impl Push {
    pub async fn run(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        // We can actually just use the existing diff/etc to push
        // 1. Diff
        // 2. Get operations
        // 3. Filter operations by
        //  a) are they an upload op?
        //  b) are they for the host/tag we are pushing here?
        let (diff, _) = sync::diff(settings, &store).await?;
        let operations = sync::operations(diff, &store).await?;

        let operations = operations
            .into_iter()
            .filter(|op| match op {
                // No noops thx
                Operation::Noop { .. } => false,

                // this is a push, so no downloads either
                Operation::Download { .. } => false,

                // push, so yes plz to uploads!
                Operation::Upload { host, tag, .. } => {
                    if let Some(h) = self.host {
                        if HostId(h) != *host {
                            return false;
                        }
                    }

                    if let Some(t) = self.tag.clone() {
                        if t != *tag {
                            return false;
                        }
                    }

                    true
                }
            })
            .collect();

        let (uploaded, _) = sync::sync_remote(operations, &store, settings).await?;

        println!("Uploaded {} records", uploaded);

        Ok(())
    }
}
