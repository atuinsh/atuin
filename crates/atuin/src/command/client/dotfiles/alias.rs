use clap::{Subcommand, ValueEnum};
use eyre::{Context, Result, eyre};

use atuin_client::{encryption, record::sqlite_store::SqliteStore, settings::Settings};

use atuin_dotfiles::{shell::Alias, store::AliasStore};

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum SortBy {
    /// Sort by alias name
    #[default]
    Name,
    /// Sort by alias value
    Value,
}

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Set an alias
    Set { name: String, value: String },

    /// Delete an alias
    Delete { name: String },

    /// List all aliases
    List {
        /// Sort results by field
        #[arg(long, value_enum, default_value_t = SortBy::Name)]
        sort_by: SortBy,

        /// Sort in reverse (descending) order
        #[arg(long, short)]
        reverse: bool,

        /// Filter aliases by name (substring match)
        #[arg(long, short)]
        name: Option<String>,

        /// Filter aliases by value (substring match)
        #[arg(long, short)]
        value: Option<String>,
    },

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

    async fn list(
        &self,
        store: &AliasStore,
        sort_by: SortBy,
        reverse: bool,
        name_filter: Option<String>,
        value_filter: Option<String>,
    ) -> Result<()> {
        let mut aliases = store.aliases().await?;

        // Apply filters
        if let Some(ref name_pattern) = name_filter {
            let pattern = name_pattern.to_lowercase();
            aliases.retain(|a| a.name.to_lowercase().contains(&pattern));
        }
        if let Some(ref value_pattern) = value_filter {
            let pattern = value_pattern.to_lowercase();
            aliases.retain(|a| a.value.to_lowercase().contains(&pattern));
        }

        // Apply sorting
        match sort_by {
            SortBy::Name => {
                aliases.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            }
            SortBy::Value => {
                aliases.sort_by(|a, b| a.value.to_lowercase().cmp(&b.value.to_lowercase()));
            }
        }

        // Apply reverse if requested
        if reverse {
            aliases.reverse();
        }

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
        }
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
            eprintln!(
                "Dotfiles are not enabled. Add\n\n[dotfiles]\nenabled = true\n\nto your configuration file to enable them.\n"
            );
            eprintln!("The default configuration file is located at ~/.config/atuin/config.toml.");
            return Ok(());
        }

        let encryption_key: [u8; 32] = encryption::load_key(settings)
            .context("could not load encryption key")?
            .into();
        let host_id = Settings::host_id().await?;

        let alias_store = AliasStore::new(store, host_id, encryption_key);

        match self {
            Self::Set { name, value } => self.set(&alias_store, name.clone(), value.clone()).await,
            Self::Delete { name } => self.delete(&alias_store, name.clone()).await,
            Self::List {
                sort_by,
                reverse,
                name,
                value,
            } => {
                self.list(
                    &alias_store,
                    *sort_by,
                    *reverse,
                    name.clone(),
                    value.clone(),
                )
                .await
            }
            Self::Clear => self.clear(&alias_store).await,
        }
    }
}
