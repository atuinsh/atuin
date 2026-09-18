use atuin_client::logs::FromSettings;
use atuin_client::settings::Settings;
use atuin_common::logs::{FileConfig, LogConfig, StderrConfig};
use atuin_common::shell::Shell;
use clap::{Args, Subcommand};
pub(crate) mod inline;
pub(crate) mod search;

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
    /// Fuzzy-find an ingested agent session and resume it, in its own agent or another
    Search {
        /// Initial query
        #[arg(value_name = "QUERY")]
        query: Option<String>,
    },
    /// Scoop up local AI coding-agent sessions (Claude Code, Codex, OpenCode, Cursor) into Atuin
    Ingest {
        /// Which agent to ingest; every supported agent when omitted
        #[arg(value_name = "AGENT")]
        agent: Option<atuin_ai_session::Agent>,
    },
    #[command(hide = true)]
    /// This command is no longer necessary. If you have it in your shell init file, feel free to
    /// remove it.
    Init {
        #[arg(hide = true)]
        _shell: Option<std::ffi::OsString>,
    },
}

impl Command {
    pub fn log_config(&self, settings: &Settings) -> Option<LogConfig> {
        match self {
            Self::Inline { args, .. } => Some(LogConfig {
                file: FileConfig::from_settings(&settings.logs, &settings.logs.ai),
                stderr: args.verbose.then(StderrConfig::default),
            }),
            Self::Search { .. } | Self::Ingest { .. } | Self::Init { .. } => None,
        }
    }
}

pub async fn run(command: Command, settings: &Settings) -> eyre::Result<()> {
    match command {
        Command::Inline {
            command,
            hook,
            args,
            ..
        } => inline::run(command, args.api_endpoint, args.api_token, settings, hook).await,
        Command::Search { query } => search::run(query, settings).await,
        Command::Ingest { agent } => {
            let store = atuin_ai_session::store::Store::open(
                &Settings::effective_data_dir().join("agent_sessions.db"),
                settings.local_timeout,
            )
            .await?;
            let agents = agent.map_or_else(|| atuin_ai_session::Agent::ALL.to_vec(), |a| vec![a]);
            for agent in agents {
                let started = std::time::Instant::now();
                match atuin_ai_session::ingest::ingest(&store, agent).await {
                    Ok(stats) => println!(
                        "{agent}: {} new messages of {} read from {} sources in {:.1?}",
                        stats.inserted,
                        stats.read,
                        stats.sources,
                        started.elapsed()
                    ),
                    Err(e) => eprintln!("{agent}: {e}"),
                }
            }
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
    }
}

pub(crate) fn detect_shell() -> String {
    Shell::current().to_string()
}
