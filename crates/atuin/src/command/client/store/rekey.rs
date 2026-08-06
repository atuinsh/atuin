use clap::Args;
use eyre::{Result, bail};
use tokio::{fs::File, io::AsyncWriteExt};

use atuin_client::{
    encryption::{load_key, paseto_v4},
    record::sqlite_store::SqliteStore,
    settings::Settings,
};

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

        let current_key = load_key(settings)?;

        store.re_encrypt(&current_key, &key).await?;

        println!("Store rewritten. Saving new key");
        let mut file = File::create(settings.key_path.clone()).await?;
        file.write_all(key.encode().dangerously_leak_secret().as_bytes())
            .await?;

        Ok(())
    }
}
