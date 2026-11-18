use atuin_client::logs::FromSettings;
use atuin_client::settings::Settings;
use atuin_common::logs::{FileConfig, LogConfig};
use clap::Subcommand;

pub(crate) mod resume;
pub(crate) mod session;

pub use resume::Cmd as ResumeCmd;
pub use session::{Cmd as ThreadsCmd, Recent, import_all, one_line, recent};

#[derive(Subcommand, Debug)]
pub enum Command {
    #[command(hide = true)]
    /// This command is no longer necessary. If you have it in your shell init file, feel free to
    /// remove it.
    Init {
        #[arg(hide = true)]
        _shell: Option<std::ffi::OsString>,
    },

    /// The `?` widget from a shell started before Atuin AI was removed.
    #[command(hide = true)]
    Inline {
        #[arg(hide = true, trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<std::ffi::OsString>,
    },

    /// `atuin threads`, as it was called before.
    #[command(subcommand, name = "session", hide = true)]
    Threads(session::Cmd),

    /// `atuin threads resume`, as it was called before.
    #[command(hide = true)]
    Resume(resume::Cmd),
}

/// `atuin threads resume` is `atuin ai resume`, so it is set up and run in one place.
impl From<session::Cmd> for Command {
    fn from(cmd: session::Cmd) -> Self {
        match cmd {
            session::Cmd::Resume(cmd) => Self::Resume(cmd),
            cmd => Self::Threads(cmd),
        }
    }
}

impl Command {
    /// Whether the command is a widget, which draws on the terminal and answers the shell on
    /// stderr.
    #[must_use]
    pub const fn is_widget(&self) -> bool {
        matches!(self, Self::Resume(_))
    }

    #[must_use]
    pub fn log_config(&self, settings: &Settings) -> Option<LogConfig> {
        match self {
            // File only: the shell widget reads the result from stderr.
            Self::Resume(_) => Some(LogConfig {
                file: FileConfig::from_settings(&settings.logs, &settings.logs.ai),
                stderr: None,
            }),
            Self::Inline { .. } | Self::Init { .. } | Self::Threads(_) => None,
        }
    }
}

/// Tells the user that Atuin AI is gone. Runs before the settings load, so that nothing else
/// reaches the old widget, which reads its answer from stderr.
pub fn inline(args: &[std::ffi::OsString]) {
    const DEPRECATED: &str = "Atuin AI has been deprecated. The ? key binding will be gone in a \
                              new shell, or if you saved what `atuin init` prints, once you save \
                              it again.";
    if args.iter().any(|arg| arg == "--hook") {
        eprintln!("__atuin_ai_print__:{DEPRECATED}");
    } else {
        eprintln!("{DEPRECATED}");
    }
}

pub async fn run(command: Command, settings: Settings) -> eyre::Result<()> {
    match command {
        Command::Inline { args } => {
            inline(&args);
            Ok(())
        }
        Command::Init { .. } => {
            // This is valid comment syntax in all the shells we support and thus a no-op: bash,
            // zsh, fish, nushell, xonsh, and powershell.
            println!(
                "# This command is no longer necessary. If you have it in your shell init file, \
                 feel free to remove it."
            );
            Ok(())
        }
        Command::Threads(cmd) => session::run(cmd, settings).await,
        Command::Resume(cmd) => resume::run(cmd, settings).await,
    }
}
