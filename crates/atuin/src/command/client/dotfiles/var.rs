use clap::{Subcommand, ValueEnum};
use eyre::{Context, Result};

use atuin_client::{encryption, record::sqlite_store::SqliteStore, settings::Settings};

use atuin_dotfiles::{shell::Var, store::var::VarStore};

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum SortBy {
    /// Sort by variable name
    #[default]
    Name,
    /// Sort by variable value
    Value,
}

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Set a variable
    Set {
        name: String,
        value: String,

        #[clap(long, short, action)]
        no_export: bool,
    },

    /// Delete a variable
    Delete { name: String },

    /// List all variables
    List {
        /// Sort results by field
        #[arg(long, value_enum, default_value_t = SortBy::Name)]
        sort_by: SortBy,

        /// Sort in reverse (descending) order
        #[arg(long, short)]
        reverse: bool,

        /// Filter variables by name (substring match)
        #[arg(long, short)]
        name: Option<String>,

        /// Filter variables by value (substring match)
        #[arg(long, short)]
        value: Option<String>,

        /// Show only exported variables
        #[arg(long, conflicts_with = "shell_only")]
        exports_only: bool,

        /// Show only non-exported (shell) variables
        #[arg(long, conflicts_with = "exports_only")]
        shell_only: bool,
    },
}

impl Cmd {
    async fn set(&self, store: VarStore, name: String, value: String, export: bool) -> Result<()> {
        let vars = store.vars().await?;
        let found: Vec<Var> = vars.into_iter().filter(|a| a.name == name).collect();
        let show_export = if export { "export " } else { "" };

        if found.is_empty() {
            println!("Setting '{show_export}{name}={value}'.");
        } else {
            println!(
                "Overwriting var '{show_export}{name}={}' with '{name}={value}'.",
                found[0].value
            );
        }

        store.set(&name, &value, export).await?;

        Ok(())
    }

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
                vars.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            }
            SortBy::Value => {
                vars.sort_by(|a, b| a.value.to_lowercase().cmp(&b.value.to_lowercase()));
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

    async fn delete(&self, store: VarStore, name: String) -> Result<()> {
        let mut vars = store.vars().await?.into_iter();

        if let Some(var) = vars.find(|var| var.name == name) {
            println!("Deleting '{name}={}'.", var.value);
            store.delete(&name).await?;
        } else {
            eprintln!("Cannot delete '{name}': Var not set.");
        }

        Ok(())
    }

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

        let var_store = VarStore::new(store, host_id, encryption_key);

        match self {
            Self::Set {
                name,
                value,
                no_export,
            } => {
                self.set(var_store, name.clone(), value.clone(), !no_export)
                    .await
            }
            Self::Delete { name } => self.delete(var_store, name.clone()).await,
            Self::List {
                sort_by,
                reverse,
                name,
                value,
                exports_only,
                shell_only,
            } => {
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
    }
}
