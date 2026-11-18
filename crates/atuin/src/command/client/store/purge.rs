use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::encryption::paseto_v4;
use atuin_common::userio;
use clap::Args;
use eyre::{Context as _, Result};

#[derive(Args, Debug)]
pub struct Purge {}

impl Purge {
    pub async fn run(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        userio::log::info!("store-purge-start");

        let key = paseto_v4::Key::try_load_from_path(&settings.key_path)
            .context("could not load encryption key")?;

        match store.purge(&key).await {
            Ok(()) => userio::log::info!("store-purge-done"),
            Err(e) => {
                userio::log::error!("store-purge-failed", error = format!("{e:#}"));
            }
        }
        // Even a failed purge may have deleted some records, which a replay would not take out
        // of the threads index.
        super::reset_ai_sessions_after(settings).await
    }
}
