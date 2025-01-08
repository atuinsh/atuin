use std::path::PathBuf;

use atuin_client::{encryption, record::sqlite_store::SqliteStore, settings::Settings};
use atuin_dotfiles::store::{var::VarStore, AliasStore};
use clap::{Parser, ValueEnum};
use eyre::{Result, WrapErr};

mod bash;
mod fish;
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

    /// Only import environment variables (useful for `~/.zsh_env` or `~/.bash_env`)
    #[clap(long)]
    only_env: bool,

    /// Skip importing environment variables (useful if already imported)
    #[clap(long)]
    skip_env: bool,
}

#[derive(Clone, Copy, ValueEnum, Debug)]
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
}

impl Cmd {
    fn init_nu(&self) {
        let full = include_str!("../../shell/atuin.nu");
        println!("{full}");

        if std::env::var("ATUIN_NOBIND").is_err() {
            const BIND_CTRL_R: &str = r"$env.config = (
    $env.config | upsert keybindings (
        $env.config.keybindings
        | append {
            name: atuin
            modifier: control
            keycode: char_r
            mode: [emacs, vi_normal, vi_insert]
            event: { send: executehostcommand cmd: (_atuin_search_cmd) }
        }
    )
)";
            const BIND_UP_ARROW: &str = r"
$env.config = (
    $env.config | upsert keybindings (
        $env.config.keybindings
        | append {
            name: atuin
            modifier: none
            keycode: up
            mode: [emacs, vi_normal, vi_insert]
            event: {
                until: [
                    {send: menuup}
                    {send: executehostcommand cmd: (_atuin_search_cmd '--shell-up-key-binding') }
                ]
            }
        }
    )
)
";
            if !self.disable_ctrl_r {
                println!("{BIND_CTRL_R}");
            }
            if !self.disable_up_arrow {
                println!("{BIND_UP_ARROW}");
            }
        }
    }

    fn static_init(&self) {
        match self.shell {
            Shell::Zsh => {
                zsh::init_static(self.disable_up_arrow, self.disable_ctrl_r);
            }
            Shell::Bash => {
                bash::init_static(self.disable_up_arrow, self.disable_ctrl_r);
            }
            Shell::Fish => {
                fish::init_static(self.disable_up_arrow, self.disable_ctrl_r);
            }
            Shell::Nu => {
                self.init_nu();
            }
            Shell::Xonsh => {
                xonsh::init_static(self.disable_up_arrow, self.disable_ctrl_r);
            }
        };
    }

    async fn dotfiles_init(&self, settings: &Settings) -> Result<()> {
        let record_store_path = PathBuf::from(settings.record_store_path.as_str());
        let sqlite_store = SqliteStore::new(record_store_path, settings.local_timeout).await?;

        let encryption_key: [u8; 32] = encryption::load_key(settings)
            .context("could not load encryption key")?
            .into();
        let host_id = Settings::host_id().expect("failed to get host_id");

        let alias_store = AliasStore::new(sqlite_store.clone(), host_id, encryption_key);
        let var_store = VarStore::new(sqlite_store.clone(), host_id, encryption_key);

        match self.shell {
            Shell::Zsh => {
                zsh::init(
                    alias_store,
                    var_store,
                    self.disable_up_arrow,
                    self.disable_ctrl_r,
                    self.only_env,
                    self.skip_env,
                )
                .await?;
            }
            Shell::Bash => {
                bash::init(
                    alias_store,
                    var_store,
                    self.disable_up_arrow,
                    self.disable_ctrl_r,
                    self.only_env,
                    self.skip_env,
                )
                .await?;
            }
            Shell::Fish => {
                fish::init(
                    alias_store,
                    var_store,
                    self.disable_up_arrow,
                    self.disable_ctrl_r,
                    self.only_env,
                    self.skip_env,
                )
                .await?;
            }
            Shell::Nu => self.init_nu(),
            Shell::Xonsh => {
                xonsh::init(
                    alias_store,
                    var_store,
                    self.disable_up_arrow,
                    self.disable_ctrl_r,
                    self.only_env,
                    self.skip_env,
                )
                .await?;
            }
        }

        Ok(())
    }

    pub async fn run(self, settings: &Settings) -> Result<()> {
        if settings.dotfiles.enabled {
            self.dotfiles_init(settings).await?;
        } else {
            self.static_init();
        }

        Ok(())
    }
}
