use atuin_client::logs::FromSettings;
use atuin_client::settings::Settings;
use atuin_common::logs::{FileConfig, LogConfig, StderrConfig};
use atuin_common::shell::Shell;
use clap::{Args, Subcommand};
pub mod init;
pub(crate) mod inline;

#[derive(Args, Debug)]
pub struct AiArgs {
    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Custom API endpoint; defaults to reading from the `ai.endpoint` setting.
    #[arg(long, global = true)]
    api_endpoint: Option<String>,

    /// Custom API token; defaults to reading from the `ai.api_token` setting.
    #[arg(long, global = true)]
    api_token: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Initialize shell integration
    Init {
        /// Shell to generate integration for; defaults to "auto"
        #[arg(value_name = "SHELL", default_value = "auto")]
        shell: String,
    },

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
}

impl Command {
    pub fn log_config(&self, settings: &Settings) -> LogConfig {
        match self {
            Self::Inline { args, .. } => LogConfig {
                file: FileConfig::from_settings(&settings.logs, &settings.logs.ai),
                stderr: args.verbose.then(StderrConfig::default),
            },
            _ => LogConfig::stderr_only(),
        }
    }
}

pub async fn run(command: Command, settings: &Settings) -> eyre::Result<()> {
    match command {
        Command::Init { shell } => init::run(shell).await,
        Command::Inline {
            command,
            hook,
            args,
            ..
        } => inline::run(command, args.api_endpoint, args.api_token, settings, hook).await,
    }
}

pub(crate) fn detect_shell() -> Option<String> {
    Some(Shell::current().to_string())
}
