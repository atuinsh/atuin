use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::encryption::paseto_v4;
use atuin_common::userio;
use clap::Args;
use eyre::{Context as _, Result};
use secrecy::{ExposeSecret, SecretString};

use crate::i18n::fl;

#[derive(Args, Debug)]
pub struct Rekey {
    #[arg(help = fl!("arg-store-rekey-key"))]
    key: Option<SecretString>,
}

impl Rekey {
    pub async fn run(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        let key: paseto_v4::Key = if let Some(key_str) = &self.key {
            userio::log::info!("store-rekey-specified-key");

            paseto_v4::Key::try_from_mnemonic(key_str.expose_secret())?
        } else {
            userio::log::info!("store-rekey-generated-key");
            paseto_v4::Key::generate()
        };

        let current_key = paseto_v4::Key::try_load_from_path(&settings.key_path)
            .context("could not load encryption key")?;

        store.re_encrypt(&current_key, &key).await?;
        super::invalidate_ai_sessions(settings).await;

        userio::log::info!("store-rekey-saving-key");
        key.overwrite_path(&settings.key_path)?;

        Ok(())
    }
}
