use clap::Args;
use eyre::{Result, bail};
use tokio::{fs::File, io::AsyncWriteExt};

use atuin_client::{
    encryption::{Key, decode_key, encode_key, generate_encoded_key, load_key},
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
        let key = if let Some(key) = self.key.clone() {
            println!("Re-encrypting store with specified key");

            match bip39::Mnemonic::from_phrase(&key, bip39::Language::English) {
                Ok(mnemonic) => encode_key(Key::from_slice(mnemonic.entropy()))?,
                Err(err) => {
                    match err.downcast_ref::<bip39::ErrorKind>() {
                        Some(err) => {
                            match err {
                                // assume they copied in the base64 key
                                bip39::ErrorKind::InvalidWord => key,
                                bip39::ErrorKind::InvalidChecksum => {
                                    bail!("key mnemonic was not valid")
                                }
                                bip39::ErrorKind::InvalidKeysize(_)
                                | bip39::ErrorKind::InvalidWordLength(_)
                                | bip39::ErrorKind::InvalidEntropyLength(_, _) => {
                                    bail!("key was not the correct length")
                                }
                            }
                        }
                        _ => {
                            // unknown error. assume they copied the base64 key
                            key
                        }
                    }
                }
            }
        } else {
            println!("Re-encrypting store with freshly-generated key");
            let (_, encoded) = generate_encoded_key()?;
            encoded
        };

        let current_key: [u8; 32] = load_key(settings)?.into();
        let new_key: [u8; 32] = decode_key(key.clone())?.into();

        store.re_encrypt(&current_key, &new_key).await?;

        println!("Store rewritten. Saving new key");
        let mut file = File::create(settings.key_path.clone()).await?;
        file.write_all(key.as_bytes()).await?;

        Ok(())
    }
}
