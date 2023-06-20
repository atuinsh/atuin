use clap::Subcommand;
use eyre::{bail, Result};

use atuin_client::record::{
    encodings::{
        key::{EncryptionKey, KeyStore},
        kv::KvStore,
    },
    store::Store,
};
use atuin_client::settings::Settings;

#[derive(Subcommand)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    // atuin kv set foo bar bar
    Set {
        #[arg(long, short)]
        key: String,

        #[arg(long, short, default_value = "global")]
        namespace: String,

        value: String,
    },

    // atuin kv get foo => bar baz
    Get {
        key: String,

        #[arg(long, short, default_value = "global")]
        namespace: String,
    },
}

impl Cmd {
    pub async fn run(
        &self,
        settings: &Settings,
        store: &mut (impl Store + Send + Sync),
    ) -> Result<()> {
        let key_store = KeyStore::new();
        // ensure this encryption key is the latest registered key before encrypting anything new.
        let encryption_key = match key_store.validate_encryption_key(store, settings).await? {
            EncryptionKey::Valid { encryption_key } => encryption_key,
            EncryptionKey::Invalid { kid, host_id } => {
                bail!("A new encryption key [id:{kid}] has been set by [host:{host_id}]. You must update to this encryption key to continue")
            }
        };

        let kv_store = KvStore::new();

        match self {
            Self::Set {
                key,
                value,
                namespace,
            } => {
                kv_store
                    .set(store, &encryption_key, namespace, key, value)
                    .await
            }

            Self::Get { key, namespace } => {
                let val = kv_store.get(store, &encryption_key, namespace, key).await?;

                if let Some(kv) = val {
                    println!("{}", kv.value);
                }

                Ok(())
            }
        }
    }
}
