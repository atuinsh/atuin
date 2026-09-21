use atuin_client::settings::{Settings, Tmux};
use atuin_common::shell::Shell as CommonShell;
use clap::{Parser, ValueEnum};
use eyre::Result;
use tracing::instrument;

mod bash;
mod fish;
mod nu;
mod powershell;
mod xonsh;
mod zsh;

#[derive(Parser, Debug)]
pub struct Cmd {
    /// Shell to generate init for, or "auto" to detect
    #[arg(default_value = "auto")]
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
    /// Auto-detect shell
    Auto,
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
    /// Resolve `Shell::Auto` to a concrete shell with the parent process name.
    fn resolve_shell(&self) -> Result<Shell> {
        match self.shell {
            Shell::Auto => match CommonShell::current() {
                CommonShell::Zsh => Ok(Shell::Zsh),
                CommonShell::Bash => Ok(Shell::Bash),
                CommonShell::Fish => Ok(Shell::Fish),
                CommonShell::Nu => Ok(Shell::Nu),
                CommonShell::Xonsh => Ok(Shell::Xonsh),
                CommonShell::Powershell => Ok(Shell::PowerShell),
                CommonShell::Sh | CommonShell::Unknown => Err(eyre::eyre!(
                    "could not detect shell. Supported shells: zsh, bash, fish, nu, xonsh, \
                     powershell"
                )),
            },
            other => Ok(other),
        }
    }

    fn static_init(&self, shell: Shell, settings: &Settings) {
        let options = self.to_options(settings);

        match shell {
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
            Shell::Auto => unreachable!("shell should be resolved before static_init"),
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
    fn pty_proxy_init(shell: Shell, settings: &Settings) {
        if !settings.pty_proxy.enabled {
            return;
        }

        let shell = match shell {
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
            Shell::Auto => unreachable!("shell should be resolved before pty_proxy_init"),
        };

        print!("{}", atuin_pty_proxy::init_script(shell));
    }

    #[cfg(not(all(feature = "pty-proxy", unix)))]
    fn pty_proxy_init(_shell: Shell, settings: &Settings) {
        if settings.pty_proxy.enabled {
            eprintln!(
                "atuin: pty_proxy.enabled is set, but this build of atuin does not include \
                 pty-proxy support"
            );
        }
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(self, settings: &Settings) -> Result<()> {
        if !settings.paths_ok() {
            eprintln!(
                "Atuin settings paths are broken. Disabling atuin shell hooks. Run `atuin doctor` \
                 to diagnose."
            );
            return Ok(());
        }

        let shell = self.resolve_shell()?;

        Self::pty_proxy_init(shell, settings);

        self.static_init(shell, settings);

        Ok(())
    }
}
