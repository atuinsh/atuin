use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::encryption::paseto_v4;
use atuin_dotfiles::store::AliasStore;
use clap::{Subcommand, ValueEnum};
use eyre::{Context, Result};

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
}

impl Cmd {
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
                aliases.sort_by_key(|a| a.name.to_lowercase());
            }
            SortBy::Value => {
                aliases.sort_by_key(|a| a.value.to_lowercase());
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

    pub async fn run(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        let Self::List {
            sort_by,
            reverse,
            name,
            value,
        } = self;

        let encryption_key = paseto_v4::Key::try_load_from_path(&settings.key_path)
            .context("could not load encryption key")?;
        let host_id = Settings::host_id().await?;

        let alias_store = AliasStore::new(store, host_id, encryption_key);
        self.list(&alias_store, *sort_by, *reverse, name.clone(), value.clone()).await
    }
}
