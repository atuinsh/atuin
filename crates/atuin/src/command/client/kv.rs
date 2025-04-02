use clap::Subcommand;
use eyre::{Context, Result};

use atuin_client::{encryption, kv::KvStore, record::store::Store, settings::Settings};

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    // atuin kv set foo bar bar
    Set {
        #[arg(long, short)]
        key: String,

        #[arg(long, short, default_value = "default")]
        namespace: String,

        value: String,
    },

    #[command(alias = "rm")]
    Delete {
        key: String,

        #[arg(long, short, default_value = "default")]
        namespace: String,
    },

    // atuin kv get foo => bar baz
    Get {
        key: String,

        #[arg(long, short, default_value = "default")]
        namespace: String,
    },

    List {
        #[arg(long, short, default_value = "default")]
        namespace: String,

        #[arg(long, short)]
        all_namespaces: bool,
    },
}

impl Cmd {
    pub async fn run(&self, settings: &Settings, store: &(impl Store + Send + Sync)) -> Result<()> {
        let kv_store = KvStore::new();

        let encryption_key: [u8; 32] = encryption::load_key(settings)
            .context("could not load encryption key")?
            .into();

        let host_id = Settings::host_id().expect("failed to get host_id");

        match self {
            Self::Set {
                key,
                value,
                namespace,
            } => {
                kv_store
                    .set(store, &encryption_key, host_id, namespace, key, Some(value))
                    .await
            }

            Self::Delete { key, namespace } => {
                kv_store
                    .set(store, &encryption_key, host_id, namespace, key, None)
                    .await
            }

            Self::Get { key, namespace } => {
                let val = kv_store.get(store, &encryption_key, namespace, key).await?;

                if let Some(kv) = val {
                    // a `None` for kv.value means the key was deleted
                    if let Some(value) = kv.value {
                        println!("{value}");
                    }
                }

                Ok(())
            }

            Self::List {
                namespace,
                all_namespaces,
            } => {
                // TODO: don't rebuild this every time lol
                let map = kv_store.build_kv(store, &encryption_key).await?;

                // slower, but sorting is probably useful
                if *all_namespaces {
                    for (ns, kv) in &map {
                        for (k, v) in kv {
                            if v.value.is_some() {
                                println!("{ns}.{k}");
                            }
                        }
                    }
                } else {
                    let ns = map.get(namespace);

                    if let Some(ns) = ns {
                        for (k, v) in ns {
                            if v.value.is_some() {
                                println!("{k}");
                            }
                        }
                    }
                }

                Ok(())
            }
        }
    }
}
