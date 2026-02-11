use clap::Subcommand;
use eyre::{Context, Result, eyre};

use atuin_client::{encryption, record::sqlite_store::SqliteStore, settings::Settings};
use atuin_kv::store::KvStore;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Set a key-value pair
    Set {
        /// Key to set
        #[arg(long, short)]
        key: String,

        /// Value to store
        value: String,

        /// Namespace for the key-value pair
        #[arg(long, short, default_value = "default")]
        namespace: String,
    },

    /// Delete one or more key-value pairs
    #[command(alias = "rm")]
    Delete {
        /// Keys to delete
        #[arg(required = true)]
        keys: Vec<String>,

        /// Namespace for the key-value pair
        #[arg(long, short, default_value = "default")]
        namespace: String,
    },

    /// Retrieve a saved value
    Get {
        /// Key to retrieve
        key: String,

        /// Namespace for the key-value pair
        #[arg(long, short, default_value = "default")]
        namespace: String,
    },

    /// List all keys in a namespace, or in all namespaces
    #[command(alias = "ls")]
    List {
        /// Namespace to list keys from
        #[arg(long, short, default_value = "default")]
        namespace: String,

        /// List all keys in all namespaces
        #[arg(long, short, alias = "all")]
        all_namespaces: bool,
    },

    /// Rebuild the KV store
    Rebuild,
}

impl Cmd {
    pub async fn run(&self, settings: &Settings, store: &SqliteStore) -> Result<()> {
        let encryption_key: [u8; 32] = encryption::load_key(settings)
            .context("could not load encryption key")?
            .into();

        let host_id = Settings::host_id().await?;

        let kv_db = atuin_kv::database::Database::new(settings.kv.db_path.clone(), 1.0).await?;
        let kv_store = KvStore::new(store.clone(), kv_db, host_id, encryption_key);

        match self {
            Self::Set {
                key,
                value,
                namespace,
            } => {
                if namespace.is_empty() {
                    return Err(eyre!("namespace cannot be empty"));
                }

                kv_store.set(namespace, key, value).await
            }

            Self::Delete { keys, namespace } => kv_store.delete(namespace, keys).await,

            Self::Get { key, namespace } => {
                let kv = kv_store.get(namespace, key).await?;

                if let Some(val) = kv {
                    println!("{val}");
                }

                Ok(())
            }

            Self::List {
                namespace,
                all_namespaces,
            } => {
                let entries = if *all_namespaces {
                    kv_store.list(None).await?
                } else {
                    kv_store.list(Some(namespace)).await?
                };

                for entry in entries {
                    if *all_namespaces {
                        println!("{}.{}", entry.namespace, entry.key);
                    } else {
                        println!("{}", entry.key);
                    }
                }

                Ok(())
            }

            Self::Rebuild {} => kv_store.build().await,
        }
    }
}
