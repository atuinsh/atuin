use clap::Subcommand;
use eyre::{Context, Result};

use atuin_client::{
    encryption,
    kv::{Key, KvStore, PasetoSymmetricKey},
    record::store::Store,
    settings::Settings,
};

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
        let kv_store = KvStore::new();

        let encryption_key: [u8; 32] = encryption::load_key(settings)
            .context("could not load encryption key")?
            .into();
        let encryption_key = PasetoSymmetricKey::from(Key::from(encryption_key));

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
