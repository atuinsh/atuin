use std::num::NonZeroU64;

use atuin_client::database::Sqlite;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::sync::{ClientSource, Operation, SyncEngine};
use atuin_client::settings::Settings;
use atuin_common::encryption::paseto_v4;
use atuin_domain::record::RecordTag;
use clap::Args;
use eyre::{Context as _, Result};

#[derive(Args, Debug)]
pub struct Pull {
    /// The tag to push (eg, 'history'). Defaults to all tags
    #[arg(long, short)]
    pub tag: Option<RecordTag>,

    /// Force push records
    ///
    /// This will first wipe the local store, and then download all records from the remote
    #[arg(long, default_value = "false")]
    pub force: bool,

    /// Page Size
    ///
    /// How many records to download at once. Defaults to 100
    #[arg(long, default_value = "100")]
    pub page: NonZeroU64,
}

impl Pull {
    pub async fn run(&self, settings: &Settings, store: SqliteStore, db: &Sqlite) -> Result<()> {
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
        let key = paseto_v4::Key::try_load_from_path(&settings.key_path)
            .context("could not load encryption key")?;
        let engine = SyncEngine::builder()
            .store(store.clone())
            .client_source(ClientSource::FromSettings {
                settings,
                caps: None,
            })
            .build()
            .connect()
            .await?
            .with_page_size(self.page);

        let keyed = engine.keyed(&key);
        let (diff, remote_index) = engine.diff().await?;

        // Skip on --force: local was already wiped above, mismatch is the user's call.
        if !self.force
            && let Some(err) = keyed.key_valid_against(&remote_index).await
        {
            return Err(crate::print_error::format_sync_error(err));
        }

        let operations = SyncEngine::operations(diff)?;

        let operations = operations
            .into_iter()
            .filter(|op| match op {
                // No noops or downloads thx
                Operation::Noop { .. } | Operation::Upload { .. } => false,

                // pull, so yes plz to downloads!
                Operation::Download { series, .. } => {
                    if self.force {
                        return true;
                    }

                    if let Some(t) = self.tag.clone()
                        && t != series.tag
                    {
                        return false;
                    }

                    true
                }
            })
            .collect();

        let (_, downloaded) = keyed.sync_remote(operations).await?;

        println!("Downloaded {} records", downloaded.len());

        crate::sync::build(settings, &store, db, Some(&downloaded)).await?;

        Ok(())
    }
}
