use clap::Args;
use eyre::Result;

use atuin_client::{
    database::Database,
    record::store::Store,
    record::sync::Operation,
    record::{sqlite_store::SqliteStore, sync},
    settings::Settings,
};

#[derive(Args, Debug)]
pub struct Pull {
    /// The tag to push (eg, 'history'). Defaults to all tags
    #[arg(long, short)]
    pub tag: Option<String>,

    /// Force push records
    /// This will first wipe the local store, and then download all records from the remote
    #[arg(long, default_value = "false")]
    pub force: bool,
}

impl Pull {
    pub async fn run(
        &self,
        settings: &Settings,
        store: SqliteStore,
        db: &dyn Database,
    ) -> Result<()> {
        if self.force {
            println!("Forcing local overwrite!");
            println!("Clearing local store");

            store.delete_all().await?;
        }

        // We can actually just use the existing diff/etc to push
        // 1. Diff
        // 2. Get operations
        // 3. Filter operations by
        //  a) are they a download op?
        //  b) are they for the host/tag we are pushing here?
        let (diff, _) = sync::diff(settings, &store).await?;
        let operations = sync::operations(diff, &store).await?;

        let operations = operations
            .into_iter()
            .filter(|op| match op {
                // No noops or downloads thx
                Operation::Noop { .. } | Operation::Upload { .. } => false,

                // pull, so yes plz to downloads!
                Operation::Download { tag, .. } => {
                    if self.force {
                        return true;
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

        let (_, downloaded) = sync::sync_remote(operations, &store, settings).await?;

        println!("Downloaded {} records", downloaded.len());

        crate::sync::build(settings, &store, db, Some(&downloaded)).await?;

        Ok(())
    }
}
