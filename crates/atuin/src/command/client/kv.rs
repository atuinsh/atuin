use std::io::{self, IsTerminal, Read};

use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::encryption::paseto_v4;
use atuin_kv::store::KvStore;
use clap::Subcommand;
use eyre::{Context, Result, eyre};
use tracing::instrument;

use crate::i18n::fl;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    #[command(about = fl!("cmd-kv-set"))]
    Set {
        #[arg(long, short, help = fl!("arg-kv-set-key"))]
        key: String,

        #[arg(help = fl!("arg-kv-set-value"))]
        value: Option<String>,

        #[arg(long, short, default_value = "default", help = fl!("arg-kv-namespace"))]
        namespace: String,
    },

    #[command(alias = "rm", about = fl!("cmd-kv-delete"))]
    Delete {
        #[arg(required = true, help = fl!("arg-kv-delete-keys"))]
        keys: Vec<String>,

        #[arg(long, short, default_value = "default", help = fl!("arg-kv-namespace"))]
        namespace: String,
    },

    #[command(about = fl!("cmd-kv-get"))]
    Get {
        #[arg(help = fl!("arg-kv-get-key"))]
        key: String,

        #[arg(long, short, default_value = "default", help = fl!("arg-kv-namespace"))]
        namespace: String,
    },

    #[command(alias = "ls", about = fl!("cmd-kv-list"))]
    List {
        #[arg(long, short, default_value = "default", help = fl!("arg-kv-list-namespace"))]
        namespace: String,

        #[arg(long, short, alias = "all", help = fl!("arg-kv-list-all-namespaces"))]
        all_namespaces: bool,
    },

    #[command(about = fl!("cmd-kv-rebuild"))]
    Rebuild,
}

impl Cmd {
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(&self, settings: &Settings, store: &SqliteStore) -> Result<()> {
        let encryption_key = paseto_v4::Key::try_load_or_generate(&settings.key_path)
            .context("could not load or generate encryption key")?;

        let host_id = Settings::host_id().await?;

        let kv_db = atuin_kv::database::Database::new(
            settings.kv.db_path.clone(),
            std::time::Duration::from_secs(1),
        )
        .await?;
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

                let value = if let Some(v) = value {
                    v.clone()
                } else if !io::stdin().is_terminal() {
                    let mut buf = String::new();
                    io::stdin()
                        .read_to_string(&mut buf)
                        .context("failed to read value from stdin")?;
                    buf
                } else {
                    return Err(eyre!("no value provided. Pass as an argument or pipe via stdin"));
                };

                kv_store.set(namespace, key, &value).await
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
