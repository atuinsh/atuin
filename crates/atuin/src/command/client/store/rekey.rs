use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::encryption::paseto_v4;
use clap::Args;
use eyre::{Context as _, Result};

use crate::i18n::fl;

#[derive(Args, Debug)]
pub struct Rekey {
    #[arg(help = fl!("arg-store-rekey-key"))]
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

        let current_key = paseto_v4::Key::try_load_from_path(&settings.key_path)
            .await
            .context("could not load encryption key")?;

        store.re_encrypt(&current_key, &key).await?;

        println!("Store rewritten. Saving new key");
        key.overwrite_path(&settings.key_path).await?;

        Ok(())
    }
}
