use clap::Args;
use eyre::Result;

use atuin_client::{
    encryption::{decode_key, generate_encoded_key, load_key},
    record::sqlite_store::SqliteStore,
    record::store::Store,
    settings::Settings,
};

#[derive(Args, Debug)]
pub struct Rekey {
    /// The new key to use for encryption. Omit for a randomly-generated key
    key: Option<String>,
}

impl Rekey {
    pub async fn run(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        let new_key = if let Some(key) = self.key.clone() {
            println!("Re-encrypting store with specified key");
            key
        } else {
            println!("Re-encrypting store with freshly-generated key");
            let (_, encoded) = generate_encoded_key()?;
            encoded
        };

        let current_key: [u8; 32] = load_key(settings)?.into();
        let new_key: [u8; 32] = decode_key(new_key)?.into();

        store.re_encrypt(&current_key, &new_key).await?;

        Ok(())
    }
}
