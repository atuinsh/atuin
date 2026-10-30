use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::encryption::paseto_v4;
use atuin_common::userio;
use clap::Args;
use eyre::{Context as _, Result};

#[derive(Args, Debug)]
pub struct Verify {}

impl Verify {
    pub async fn run(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        userio::log::info!("store-verify-start");

        let key = paseto_v4::Key::try_load_from_path(&settings.key_path)
            .context("could not load encryption key")?;

        match store.verify(&key).await {
            Ok(()) => userio::log::info!("store-verify-done"),
            Err(e) => userio::log::error!("store-verify-failed", error = format!("{e:#}")),
        }

        Ok(())
    }
}
