use std::path::PathBuf;

use atuin_client::{encryption, record::sqlite_store::SqliteStore, settings::Settings};
use atuin_config::store::AliasStore;
use clap::{Parser, ValueEnum};
use eyre::{Result, WrapErr};

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
    fn init_bash(&self) {
        let base = include_str!("../../shell/atuin.bash");
        let (bind_ctrl_r, bind_up_arrow) = if std::env::var("ATUIN_NOBIND").is_ok() {
            (false, false)
        } else {
            (!self.disable_ctrl_r, !self.disable_up_arrow)
        };

        println!("__atuin_bind_ctrl_r={bind_ctrl_r}");
        println!("__atuin_bind_up_arrow={bind_up_arrow}");
        println!("{base}");
    }

    fn init_fish(&self) {
        let full = include_str!("../../shell/atuin.fish");
        println!("{full}");

        if std::env::var("ATUIN_NOBIND").is_err() {
            const BIND_CTRL_R: &str = r"bind \cr _atuin_search";
            const BIND_UP_ARROW: &str = r"bind -k up _atuin_bind_up
bind \eOA _atuin_bind_up
bind \e\[A _atuin_bind_up";
            const BIND_CTRL_R_INS: &str = r"bind -M insert \cr _atuin_search";
            const BIND_UP_ARROW_INS: &str = r"bind -M insert -k up _atuin_bind_up
bind -M insert \eOA _atuin_bind_up
bind -M insert \e\[A _atuin_bind_up";

            if !self.disable_ctrl_r {
                println!("{BIND_CTRL_R}");
            }
            if !self.disable_up_arrow {
                println!("{BIND_UP_ARROW}");
            }

            println!("if bind -M insert > /dev/null 2>&1");
            if !self.disable_ctrl_r {
                println!("{BIND_CTRL_R_INS}");
            }
            if !self.disable_up_arrow {
                println!("{BIND_UP_ARROW_INS}");
            }
            println!("end");
        }
    }

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
# The up arrow keybinding has surprising behavior in Nu, and is disabled by default.
# See https://github.com/atuinsh/atuin/issues/1025 for details
# $env.config = (
#     $env.config | upsert keybindings (
#         $env.config.keybindings
#         | append {
#             name: atuin
#             modifier: none
#             keycode: up
#             mode: [emacs, vi_normal, vi_insert]
#             event: { send: executehostcommand cmd: (_atuin_search_cmd '--shell-up-key-binding') }
#         }
#     )
# )
";
            if !self.disable_ctrl_r {
                println!("{BIND_CTRL_R}");
            }
            if !self.disable_up_arrow {
                println!("{BIND_UP_ARROW}");
            }
        }
    }

    fn init_xonsh(&self) {
        let base = include_str!("../../shell/atuin.xsh");
        let (bind_ctrl_r, bind_up_arrow) = if std::env::var("ATUIN_NOBIND").is_ok() {
            (false, false)
        } else {
            (!self.disable_ctrl_r, !self.disable_up_arrow)
        };
        println!(
            "_ATUIN_BIND_CTRL_R={}",
            if bind_ctrl_r { "True" } else { "False" }
        );
        println!(
            "_ATUIN_BIND_UP_ARROW={}",
            if bind_up_arrow { "True" } else { "False" }
        );
        println!("{base}");
    }

    pub async fn run(self, settings: &Settings) -> Result<()> {
        let record_store_path = PathBuf::from(settings.record_store_path.as_str());
        let sqlite_store = SqliteStore::new(record_store_path, settings.local_timeout).await?;

        let encryption_key: [u8; 32] = encryption::load_key(settings)
            .context("could not load encryption key")?
            .into();
        let host_id = Settings::host_id().expect("failed to get host_id");

        let alias_store = AliasStore::new(sqlite_store, host_id, encryption_key);

        match self.shell {
            Shell::Zsh => {
                zsh::init(alias_store, self.disable_up_arrow, self.disable_ctrl_r).await?;
            }
            Shell::Bash => self.init_bash(),
            Shell::Fish => self.init_fish(),
            Shell::Nu => self.init_nu(),
            Shell::Xonsh => self.init_xonsh(),
        }

        Ok(())
    }
}
