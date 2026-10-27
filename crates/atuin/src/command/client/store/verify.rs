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
        userio::log::info!("Verifying local store can be decrypted with the current key");

        let key = paseto_v4::Key::try_load_from_path(&settings.key_path)
            .context("could not load encryption key")?;

        match store.verify(&key).await {
            Ok(()) => userio::log::info!("Local store encryption verified OK"),
            Err(e) => userio::log::error!("failed to verify local store encryption: {e:#}"),
        }

        Ok(())
    }
}
