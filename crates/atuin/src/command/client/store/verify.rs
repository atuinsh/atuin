use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::encryption::paseto_v4;
use clap::Args;
use eyre::Result;

#[derive(Args, Debug)]
pub struct Verify {}

impl Verify {
    pub async fn run(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        println!("Verifying local store can be decrypted with the current key");

        let key = paseto_v4::Key::try_load_from_path(&settings.key_path)?;

        match store.verify(&key).await {
            Ok(()) => println!("Local store encryption verified OK"),
            Err(e) => println!("Failed to verify local store encryption: {e:?}"),
        }

        Ok(())
    }
}
