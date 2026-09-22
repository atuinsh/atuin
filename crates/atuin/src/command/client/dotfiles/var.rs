use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::encryption::paseto_v4;
use atuin_dotfiles::store::var::VarStore;
use clap::{Subcommand, ValueEnum};
use eyre::{Context, Result};

use crate::i18n::fl;

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum SortBy {
    #[value(help = fl!("value-dotfiles-var-list-sort-by-name"))]
    #[default]
    Name,
    #[value(help = fl!("value-dotfiles-var-list-sort-by-value"))]
    Value,
}

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    #[command(about = fl!("cmd-dotfiles-var-list"))]
    List {
        #[arg(
            long,
            value_enum,
            default_value_t = SortBy::Name,
            help = fl!("arg-dotfiles-var-list-sort-by")
        )]
        sort_by: SortBy,

        #[arg(long, short, help = fl!("arg-dotfiles-var-list-reverse"))]
        reverse: bool,

        #[arg(long, short, help = fl!("arg-dotfiles-var-list-name"))]
        name: Option<String>,

        #[arg(long, short, help = fl!("arg-dotfiles-var-list-value"))]
        value: Option<String>,

        #[arg(
            long,
            conflicts_with = "shell_only",
            help = fl!("arg-dotfiles-var-list-exports-only")
        )]
        exports_only: bool,

        #[arg(
            long,
            conflicts_with = "exports_only",
            help = fl!("arg-dotfiles-var-list-shell-only")
        )]
        shell_only: bool,
    },
}

impl Cmd {
    #[allow(clippy::too_many_arguments)]
    async fn list(
        &self,
        store: VarStore,
        sort_by: SortBy,
        reverse: bool,
        name_filter: Option<String>,
        value_filter: Option<String>,
        exports_only: bool,
        shell_only: bool,
    ) -> Result<()> {
        let mut vars = store.vars().await?;

        // Apply export/shell filters
        if exports_only {
            vars.retain(|v| v.export);
        }
        if shell_only {
            vars.retain(|v| !v.export);
        }

        // Apply name/value filters
        if let Some(ref name_pattern) = name_filter {
            let pattern = name_pattern.to_lowercase();
            vars.retain(|v| v.name.to_lowercase().contains(&pattern));
        }
        if let Some(ref value_pattern) = value_filter {
            let pattern = value_pattern.to_lowercase();
            vars.retain(|v| v.value.to_lowercase().contains(&pattern));
        }

        // Apply sorting
        match sort_by {
            SortBy::Name => {
                vars.sort_by_key(|a| a.name.to_lowercase());
            }
            SortBy::Value => {
                vars.sort_by_key(|a| a.value.to_lowercase());
            }
        }

        // Apply reverse if requested
        if reverse {
            vars.reverse();
        }

        for i in vars {
            if i.export {
                println!("export {}={}", i.name, i.value);
            } else {
                println!("{}={}", i.name, i.value);
            }
        }

        Ok(())
    }

    pub async fn run(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        let Self::List {
            sort_by,
            reverse,
            name,
            value,
            exports_only,
            shell_only,
        } = self;

        let encryption_key = paseto_v4::Key::try_load_from_path(&settings.key_path)
            .context("could not load encryption key")?;
        let host_id = Settings::host_id().await?;

        let var_store = VarStore::new(store, host_id, encryption_key);
        self.list(
            var_store,
            *sort_by,
            *reverse,
            name.clone(),
            value.clone(),
            *exports_only,
            *shell_only,
        )
        .await
    }
}
