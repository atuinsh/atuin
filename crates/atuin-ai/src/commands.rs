use clap::{Parser, Subcommand, ValueEnum};
use tracing::Level;
use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};

pub mod init;
pub mod inline;
pub mod setup;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Custom API endpoint
    #[arg(long, global = true, env = "ATUIN_AI_API_ENDPOINT")]
    api_endpoint: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize shell integration
    Init {
        /// Shell integration to generate
        #[arg(long, value_enum)]
        shell: Option<Shell>,
    },

    /// Complete current command line
    Complete {
        /// Current command line to complete
        #[arg(value_name = "COMMAND")]
        command: Option<String>,
    },

    /// Inline completion mode with small TUI overlay
    Inline {
        /// Current command line to complete
        #[arg(value_name = "COMMAND")]
        command: Option<String>,

        /// Start in natural language mode
        #[arg(long)]
        natural_language: bool,
    },

    /// Interactive mode with TUI
    Interactive,

    /// Authenticate with Atuin Hub
    Setup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
    Nu,
    Xonsh,
    PowerShell,
}

pub async fn run() -> eyre::Result<()> {
    let cli = Cli::parse();

    init_tracing(cli.verbose);

    match cli.command {
        Commands::Init { shell } => init::run(shell.unwrap_or(Shell::Zsh)).await,
        Commands::Inline {
            command,
            natural_language,
        } => inline::run(command, natural_language, cli.api_endpoint).await,
        Commands::Complete { command } => inline::run(command, false, cli.api_endpoint).await,
        Commands::Interactive => Err(eyre::eyre!("interactive mode not implemented yet")),
        Commands::Setup => setup::run(cli.api_endpoint).await,
    }
}

fn init_tracing(verbose: bool) {
    let level = if verbose { Level::DEBUG } else { Level::INFO };

    // Create env filter
    let env_filter = EnvFilter::from_default_env().add_directive(
        format!("atuin_ai={}", level.as_str().to_lowercase())
            .parse()
            .unwrap(),
    );

    // Create console layer (only for verbose mode)
    let console_layer = if verbose {
        Some(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(true)
                .with_target(false)
                .with_filter(env_filter),
        )
    } else {
        None
    };

    // Initialize subscriber
    let subscriber = tracing_subscriber::registry();

    if let Some(console) = console_layer {
        subscriber.with(console).init();
    } else {
        subscriber.init();
    }
}
