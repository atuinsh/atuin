use atuin_client::settings::{Settings, Tmux};
use clap::{Parser, ValueEnum};
use eyre::Result;
use tracing::instrument;

use crate::i18n::fl;

mod bash;
mod fish;
mod nu;
mod powershell;
mod xonsh;
mod zsh;

#[derive(Parser, Debug)]
pub struct Cmd {
    shell: Shell,

    #[clap(long, help = fl!("arg-init-disable-ctrl-r"))]
    disable_ctrl_r: bool,

    #[clap(long, help = fl!("arg-init-disable-up-arrow"))]
    disable_up_arrow: bool,

    #[clap(long, help = fl!("arg-init-disable-ai"))]
    disable_ai: bool,
}

#[derive(Clone, Copy, ValueEnum, Debug)]
#[value(rename_all = "lower")]
#[allow(clippy::enum_variant_names, clippy::doc_markdown)]
pub enum Shell {
    #[value(help = fl!("value-init-shell-zsh"))]
    Zsh,
    #[value(help = fl!("value-init-shell-bash"))]
    Bash,
    #[value(help = fl!("value-init-shell-fish"))]
    Fish,
    #[value(help = fl!("value-init-shell-nu"))]
    Nu,
    #[value(help = fl!("value-init-shell-xonsh"))]
    Xonsh,
    #[value(help = fl!("value-init-shell-powershell"))]
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

    fn to_options<'a>(&self, settings: &'a Settings) -> StaticInitOptions<'a> {
        StaticInitOptions {
            enable_up_arrow: !self.disable_up_arrow,
            enable_ctrl_r: !self.disable_ctrl_r,
            enable_ai: !self.disable_ai && settings.ai.enabled.unwrap_or(true),
            tmux: &settings.tmux,
        }
    }

    /// When `pty_proxy.enabled` is set, prepend the pty-proxy exec preamble to the init script so
    /// the shell re-execs itself inside `atuin pty-proxy` — no separate `atuin pty-proxy init` line
    /// needed in shell config. The preamble checks whether a PTY proxy has already been spawned and
    /// won't spawn another, so users who still have the standalone line keep working: whichever
    /// copy runs first wins and the other no-ops.
    #[cfg(all(feature = "pty-proxy", unix))]
    fn pty_proxy_init(&self, settings: &Settings) {
        if !settings.pty_proxy.enabled {
            return;
        }

        let shell = match self.shell {
            Shell::Zsh => atuin_pty_proxy::Shell::Zsh,
            Shell::Bash => atuin_pty_proxy::Shell::Bash,
            Shell::Fish => atuin_pty_proxy::Shell::Fish,
            Shell::Nu => atuin_pty_proxy::Shell::Nu,
            Shell::Xonsh | Shell::PowerShell => {
                eprintln!(
                    "atuin: pty_proxy.enabled is set, but atuin pty-proxy does not support this \
                     shell"
                );
                return;
            }
        };

        print!("{}", atuin_pty_proxy::init_script(shell));
    }

    #[cfg(not(all(feature = "pty-proxy", unix)))]
    fn pty_proxy_init(&self, settings: &Settings) {
        if settings.pty_proxy.enabled {
            eprintln!(
                "atuin: pty_proxy.enabled is set, but this build of atuin does not include \
                 pty-proxy support"
            );
        }
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(self, settings: &Settings) -> Result<()> {
        if !settings.paths_ok().await {
            eprintln!(
                "Atuin settings paths are broken. Disabling atuin shell hooks. Run `atuin doctor` \
                 to diagnose."
            );
            return Ok(());
        }

        self.pty_proxy_init(settings);

        self.static_init(settings);

        Ok(())
    }
}
