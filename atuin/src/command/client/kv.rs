use clap::Subcommand;
use eyre::Result;

use atuin_client::{kv::KvStore, record::store::Store, settings::Settings};

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
        _settings: &Settings,
        store: &mut (impl Store + Send + Sync),
    ) -> Result<()> {
        let kv_store = KvStore::new();

        match self {
            Self::Set {
                key,
                value,
                namespace,
            } => kv_store.set(store, namespace, key, value).await,

            Self::Get { key, namespace } => {
                let val = kv_store.get(store, namespace, key).await?;

                if let Some(kv) = val {
                    println!("{}", kv.value);
                }

                Ok(())
            }
        }
    }
}
