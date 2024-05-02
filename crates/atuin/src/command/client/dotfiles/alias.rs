use clap::Subcommand;
use eyre::{eyre, Context, Result};

use atuin_client::{encryption, record::sqlite_store::SqliteStore, settings::Settings};

use atuin_dotfiles::{shell::Alias, store::AliasStore};

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Set an alias
    Set { name: String, value: String },

    /// Delete an alias
    Delete { name: String },

    /// List all aliases
    List,

    /// Delete all aliases
    Clear,
    // There are too many edge cases to parse at the moment. Disable for now.
    // Import,
}

impl Cmd {
    async fn set(&self, store: &AliasStore, name: String, value: String) -> Result<()> {
        let illegal_char = regex::Regex::new("[ \t\n&();<>|\\\"'`$/]").unwrap();
        if illegal_char.is_match(name.as_str()) {
            return Err(eyre!("Illegal character in alias name"));
        }

        let aliases = store.aliases().await?;
        let found: Vec<Alias> = aliases.into_iter().filter(|a| a.name == name).collect();

        if found.is_empty() {
            println!("Aliasing '{name}={value}'.");
        } else {
            println!(
                "Overwriting alias '{name}={}' with '{name}={value}'.",
                found[0].value
            );
        }

        store.set(&name, &value).await?;

        Ok(())
    }

    async fn list(&self, store: &AliasStore) -> Result<()> {
        let aliases = store.aliases().await?;

        for i in aliases {
            println!("{}={}", i.name, i.value);
        }

        Ok(())
    }

    async fn clear(&self, store: &AliasStore) -> Result<()> {
        let aliases = store.aliases().await?;

        for i in aliases {
            self.delete(store, i.name).await?;
        }

        Ok(())
    }

    async fn delete(&self, store: &AliasStore, name: String) -> Result<()> {
        let mut aliases = store.aliases().await?.into_iter();
        if let Some(alias) = aliases.find(|alias| alias.name == name) {
            println!("Deleting '{name}={}'.", alias.value);
            store.delete(&name).await?;
        } else {
            eprintln!("Cannot delete '{name}': Alias not set.");
        };
        Ok(())
    }

    /*
    async fn import(&self, store: &AliasStore) -> Result<()> {
        let aliases = atuin_dotfiles::shell::import_aliases(store).await?;

        for i in aliases {
            println!("Importing {}={}", i.name, i.value);
        }

        Ok(())
    }
    */

    pub async fn run(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        if !settings.dotfiles.enabled {
            eprintln!("Dotfiles are not enabled. Add\n\n[dotfiles]\nenabled = true\n\nto your configuration file to enable them.\n");
            eprintln!("The default configuration file is located at ~/.config/atuin/config.toml.");
            return Ok(());
        }

        let encryption_key: [u8; 32] = encryption::load_key(settings)
            .context("could not load encryption key")?
            .into();
        let host_id = Settings::host_id().expect("failed to get host_id");

        let alias_store = AliasStore::new(store, host_id, encryption_key);

        match self {
            Self::Set { name, value } => self.set(&alias_store, name.clone(), value.clone()).await,
            Self::Delete { name } => self.delete(&alias_store, name.clone()).await,
            Self::List => self.list(&alias_store).await,
            Self::Clear => self.clear(&alias_store).await,
        }
    }
}
