use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::encryption::paseto_v4;
use clap::Args;
use eyre::{Context as _, Result};

#[derive(Args, Debug)]
pub struct Purge {}

impl Purge {
    pub async fn run(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        println!("Purging local records that cannot be decrypted");

        let key = paseto_v4::Key::try_load_from_path(&settings.key_path)
            .context("could not load encryption key")?;

        match store.purge(&key).await {
            Ok(()) => println!("Local store purge completed OK"),
            Err(e) => println!("Failed to purge local store: {e:?}"),
        }

        Ok(())
    }
}
