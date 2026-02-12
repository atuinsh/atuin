use clap::{Parser, Subcommand};
use tracing::Level;
use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};

pub mod init;
pub mod inline;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Custom API endpoint (e.g., https://openrouter.ai/api/v1)
    #[arg(long, global = true, env = "ATUIN_AI_API_ENDPOINT")]
    api_endpoint: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize shell integration
    Init,

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
}

pub async fn run() -> eyre::Result<()> {
    let cli = Cli::parse();

    init_tracing(cli.verbose);

    match cli.command {
        Commands::Init => init::run().await,
        Commands::Inline {
            command,
            natural_language,
        } => inline::run(command, natural_language, cli.api_endpoint).await,
        Commands::Complete { command } => inline::run(command, false, cli.api_endpoint).await,
        Commands::Interactive => Err(eyre::eyre!("interactive mode not implemented yet")),
    }
}

fn init_tracing(verbose: bool) {
    let level = if verbose { Level::DEBUG } else { Level::INFO };

    // Create ~/.atuin/logs directory if it doesn't exist
    let dirs = directories::UserDirs::new().expect("Could not find home directory");
    let log_dir = dirs.home_dir().join(".atuin").join("logs");
    std::fs::create_dir_all(&log_dir).expect("Could not create log directory");

    // Set up file appender with daily rotation
    let file_appender = tracing_appender::rolling::daily(&log_dir, "ai.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Create env filter
    let env_filter = EnvFilter::from_default_env().add_directive(
        format!("atuin_ai={}", level.as_str().to_lowercase())
            .parse()
            .unwrap(),
    );

    // Create file layer (always logs INFO and above to file)
    let file_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .with_filter(EnvFilter::new("atuin_ai=info"));

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

    // Initialize subscriber with both layers
    let subscriber = tracing_subscriber::registry().with(file_layer);

    if let Some(console) = console_layer {
        subscriber.with(console).init();
    } else {
        subscriber.init();
    }

    // Keep the guard alive for the duration of the program
    std::mem::forget(_guard);
}
