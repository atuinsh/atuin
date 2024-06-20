use clap::Subcommand;
use eyre::{Context, Result};

use atuin_client::{encryption, record::sqlite_store::SqliteStore, settings::Settings};

use atuin_dotfiles::{shell::Var, store::var::VarStore};

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
    List,
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
                "Overwriting alias '{show_export}{name}={}' with '{name}={value}'.",
                found[0].value
            );
        }

        store.set(&name, &value, export).await?;

        Ok(())
    }

    async fn list(&self, store: VarStore) -> Result<()> {
        let vars = store.vars().await?;

        for i in vars.iter().filter(|v| !v.export) {
            println!("{}={}", i.name, i.value);
        }

        for i in vars.iter().filter(|v| v.export) {
            println!("export {}={}", i.name, i.value);
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
        };

        Ok(())
    }

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
            Self::List => self.list(var_store).await,
        }
    }
}
