use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::encryption::paseto_v4;
use atuin_dotfiles::store::AliasStore;
use clap::{Subcommand, ValueEnum};
use eyre::{Context, Result};

use crate::i18n::fl;

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum SortBy {
    #[value(help = fl!("value-dotfiles-alias-list-sort-by-name"))]
    #[default]
    Name,
    #[value(help = fl!("value-dotfiles-alias-list-sort-by-value"))]
    Value,
}

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    #[command(about = fl!("cmd-dotfiles-alias-list"))]
    List {
        #[arg(
            long,
            value_enum,
            default_value_t = SortBy::Name,
            help = fl!("arg-dotfiles-list-sort-by")
        )]
        sort_by: SortBy,

        #[arg(long, short, help = fl!("arg-dotfiles-list-reverse"))]
        reverse: bool,

        #[arg(long, short, help = fl!("arg-dotfiles-alias-list-name"))]
        name: Option<String>,

        #[arg(long, short, help = fl!("arg-dotfiles-alias-list-value"))]
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
            .await
            .context("could not load encryption key")?;
        let host_id = Settings::host_id().await?;

        let alias_store = AliasStore::new(store, host_id, encryption_key);
        self.list(&alias_store, *sort_by, *reverse, name.clone(), value.clone()).await
    }
}
