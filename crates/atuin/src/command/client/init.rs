use std::path::PathBuf;

use atuin_client::{
    encryption,
    record::sqlite_store::SqliteStore,
    settings::{Settings, Tmux},
};
use atuin_dotfiles::store::{AliasStore, var::VarStore};
use clap::{Parser, ValueEnum};
use eyre::{Result, WrapErr};

mod bash;
mod fish;
mod nu;
mod powershell;
mod xonsh;
mod zsh;

#[derive(Parser, Debug)]
pub struct Cmd {
    shell: Shell,

    /// Disable the binding of CTRL-R to atuin
    #[clap(long)]
    disable_ctrl_r: bool,

    /// Disable the binding of the Up Arrow key to atuin
    #[clap(long)]
    disable_up_arrow: bool,

    /// Disable the binding of ? to Atuin AI
    #[clap(long)]
    disable_ai: bool,
}

#[derive(Clone, Copy, ValueEnum, Debug)]
#[value(rename_all = "lower")]
#[allow(clippy::enum_variant_names, clippy::doc_markdown)]
pub enum Shell {
    /// Zsh setup
    Zsh,
    /// Bash setup
    Bash,
    /// Fish setup
    Fish,
    /// Nu setup
    Nu,
    /// Xonsh setup
    Xonsh,
    /// PowerShell setup
    PowerShell,
}

struct StaticInitOptions<'a> {
    pub enable_up_arrow: bool,
    pub enable_ctrl_r: bool,
    #[cfg_attr(not(feature = "ai"), allow(dead_code))]
    pub enable_ai: bool,
    pub tmux: &'a Tmux,
}

impl Cmd {
    fn static_init(&self, settings: &Settings) {
        let options = self.to_options(settings);

        match self.shell {
            Shell::Zsh => {
                zsh::init_static(&options);
            }
            Shell::Bash => {
                bash::init_static(&options);
            }
            Shell::Fish => {
                fish::init_static(&options);
            }
            Shell::Nu => {
                nu::init_static(&options);
            }
            Shell::Xonsh => {
                xonsh::init_static(&options);
            }
            Shell::PowerShell => {
                powershell::init_static(&options);
            }
        }
    }

    async fn dotfiles_init(&self, settings: &Settings) -> Result<()> {
        let record_store_path = PathBuf::from(settings.record_store_path.as_str());
        let sqlite_store = SqliteStore::new(record_store_path, settings.local_timeout).await?;

        let encryption_key: [u8; 32] = encryption::load_key(settings)
            .context("could not load encryption key")?
            .into();
        let host_id = Settings::host_id().await?;

        let alias_store = AliasStore::new(sqlite_store.clone(), host_id, encryption_key);
        let var_store = VarStore::new(sqlite_store.clone(), host_id, encryption_key);

        let options = self.to_options(settings);

        match self.shell {
            Shell::Zsh => {
                zsh::init(alias_store, var_store, &options).await?;
            }
            Shell::Bash => {
                bash::init(alias_store, var_store, &options).await?;
            }
            Shell::Fish => {
                fish::init(alias_store, var_store, &options).await?;
            }
            Shell::Nu => nu::init_static(&options),
            Shell::Xonsh => {
                xonsh::init(alias_store, var_store, &options).await?;
            }
            Shell::PowerShell => {
                powershell::init(alias_store, var_store, &options).await?;
            }
        }

        Ok(())
    }

    fn to_options<'a>(&self, settings: &'a Settings) -> StaticInitOptions<'a> {
        StaticInitOptions {
            enable_up_arrow: !self.disable_up_arrow,
            enable_ctrl_r: !self.disable_ctrl_r,
            enable_ai: !self.disable_ai && settings.ai.enabled.unwrap_or(true),
            tmux: &settings.tmux,
        }
    }

    pub async fn run(self, settings: &Settings) -> Result<()> {
        if !settings.paths_ok() {
            eprintln!(
                "Atuin settings paths are broken. Disabling atuin shell hooks. Run `atuin doctor` to diagnose."
            );
            return Ok(());
        }

        if settings.dotfiles.enabled {
            self.dotfiles_init(settings).await?;
        } else {
            self.static_init(settings);
        }

        Ok(())
    }
}
