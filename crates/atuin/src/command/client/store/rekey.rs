use clap::Args;
use eyre::Result;

use atuin_client::{encryption::paseto_v4, record::sqlite_store::SqliteStore, settings::Settings};

#[derive(Args, Debug)]
pub struct Rekey {
    /// The new key to use for encryption. Omit for a randomly-generated key
    key: Option<String>,
}

impl Rekey {
    pub async fn run(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        let key: paseto_v4::Key = if let Some(key_str) = &self.key {
            println!("Re-encrypting store with specified key");

            paseto_v4::Key::try_from_mnemonic(key_str)?
        } else {
            println!("Re-encrypting store with freshly-generated key");
            paseto_v4::Key::generate()
        };

        let current_key = paseto_v4::Key::try_load_from_path(&settings.key_path)?;

        store.re_encrypt(&current_key, &key).await?;

        println!("Store rewritten. Saving new key");
        key.try_write_path(&settings.key_path)?;

        Ok(())
    }
}
