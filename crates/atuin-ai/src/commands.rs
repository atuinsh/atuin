use std::sync::Arc;

use atuin_client::settings::Settings;
use atuin_common::logs::LogMode;
use atuin_common::out::outln;
use atuin_common::shell::Shell;
use clap::{Args, Subcommand};
use secrecy::SecretString;
pub(crate) mod inline;
pub(crate) mod resume;
pub(crate) mod session;

pub use resume::Cmd as ResumeCmd;
pub use session::{Cmd as ThreadsCmd, import_all};

#[derive(Args, Debug)]
pub struct AiArgs {
    #[arg(short, long, global = true, hide = true)]
    #[allow(
        dead_code,
        reason = "accepted so existing invocations keep working; logging is set by ATUIN_LOG"
    )]
    verbose: bool,

    /// Custom API endpoint; defaults to reading from the `ai.endpoint` setting.
    #[arg(long, global = true)]
    api_endpoint: Option<String>,

    /// Custom API token; defaults to reading from the `ai.api_token` setting.
    #[arg(long, global = true)]
    api_token: Option<SecretString>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Inline completion mode with small TUI overlay
    Inline {
        #[command(flatten)]
        args: AiArgs,

        /// Current command line to complete
        #[arg(value_name = "COMMAND")]
        command: Option<String>,

        /// Use the hook mode
        #[arg(long, hide = true)]
        hook: bool,
    },
    #[command(hide = true)]
    /// This command is no longer necessary. If you have it in your shell init file, feel free to
    /// remove it.
    Init {
        #[arg(hide = true)]
        _shell: Option<std::ffi::OsString>,
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
    #[must_use]
    pub const fn log_mode(&self) -> Option<LogMode> {
        match self {
            // Their TUIs own the terminal, and the shell widget reads resume's result from stderr.
            Self::Inline { .. } | Self::Resume(_) => Some(LogMode::File),
            Self::Threads(cmd) => Some(cmd.log_mode()),
            Self::Init { .. } => Some(LogMode::Stderr),
        }
    }
}

pub async fn run(command: Command, settings: Settings) -> eyre::Result<()> {
    match command {
        Command::Inline {
            command,
            hook,
            args,
            ..
        } => {
            let settings = Arc::new(settings);
            inline::run(command, args.api_endpoint, args.api_token, &settings, hook).await
        }
        Command::Init { .. } => {
            // This is valid comment syntax in all the shells we support and thus a no-op: bash,
            // zsh, fish, nushell, xonsh, and powershell.
            outln!(
                "# This command is no longer necessary. If you have it in your shell init file, \
                 feel free to remove it."
            );
            Ok(())
        }
        Command::Threads(cmd) => session::run(cmd, settings).await,
        Command::Resume(cmd) => resume::run(cmd, settings).await,
    }
}

pub(crate) fn detect_shell() -> String {
    Shell::current().to_string()
}
