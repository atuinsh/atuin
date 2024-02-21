use clap::Subcommand;
use eyre::{Context, Result};

use atuin_client::{encryption, record::sqlite_store::SqliteStore, settings::Settings};

use atuin_config::{shell::Alias, store::AliasStore};

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    Set { name: String, value: String },
    Delete { name: String },
    List,
}

impl Cmd {
    async fn set(&self, store: AliasStore, name: String, value: String) -> Result<()> {
        let aliases = store.aliases().await?;
        let found: Vec<Alias> = aliases.into_iter().filter(|a| a.name == name).collect();

        if found.is_empty() {
            println!("Aliasing {name}={value}");
        } else {
            println!(
                "Overwriting alias {name}={} with {name}={value}",
                found[0].value
            );
        }

        store.set(&name, &value).await?;

        Ok(())
    }

    async fn list(&self, store: AliasStore) -> Result<()> {
        let aliases = store.aliases().await?;

        for i in aliases {
            println!("{}={}", i.name, i.value);
        }

        Ok(())
    }

    async fn delete(&self, store: AliasStore, name: String) -> Result<()> {
        let aliases = store.aliases().await?;
        let found = aliases.into_iter().any(|a| a.name == name);

        if !found {
            eprintln!("Alias not found - \"{name}\" - could not delete");
            return Ok(());
        }

        store.delete(&name).await?;

        Ok(())
    }

    pub async fn run(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        let encryption_key: [u8; 32] = encryption::load_key(settings)
            .context("could not load encryption key")?
            .into();
        let host_id = Settings::host_id().expect("failed to get host_id");

        let alias_store = AliasStore::new(store, host_id, encryption_key);

        match self {
            Self::Set { name, value } => self.set(alias_store, name.clone(), value.clone()).await,
            Self::Delete { name } => self.delete(alias_store, name.clone()).await,
            Self::List => self.list(alias_store).await,
        }
    }
}
