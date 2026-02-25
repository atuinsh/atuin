use atuin_common::shell::Shell;
use clap::{Parser, Subcommand};
use tracing::Level;
use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[cfg(debug_assertions)]
pub mod debug_render;

pub mod init;
pub mod inline;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Custom API endpoint
    #[arg(long, global = true, env = "ATUIN_AI_API_ENDPOINT")]
    api_endpoint: Option<String>,

    /// Custom API token
    #[arg(long, global = true, env = "ATUIN_AI_API_TOKEN")]
    api_token: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize shell integration
    Init {
        /// Shell to generate integration for; defaults to "auto"
        #[arg(value_name = "SHELL", default_value = "auto")]
        shell: String,
    },

    /// Inline completion mode with small TUI overlay
    Inline {
        /// Current command line to complete
        #[arg(value_name = "COMMAND")]
        command: Option<String>,

        /// Start in natural language mode
        #[arg(long)]
        natural_language: bool,

        /// Keep TUI output visible after exit (default: erase)
        #[arg(long)]
        keep: bool,

        /// Log state changes to file for debugging (dev tool)
        #[arg(long, value_name = "FILE")]
        debug_state: Option<String>,
    },

    /// Debug render: output a single frame from JSON state (dev tool)
    #[cfg(debug_assertions)]
    DebugRender {
        /// Input file (reads from stdin if not provided)
        #[arg(short, long)]
        input: Option<String>,

        /// Output format: ansi (default), plain, json
        #[arg(short, long, default_value = "ansi")]
        format: String,
    },
}

pub async fn run() -> eyre::Result<()> {
    let cli = Cli::parse();

    init_tracing(cli.verbose);

    match cli.command {
        Commands::Init { shell } => init::run(shell).await,
        Commands::Inline {
            command,
            natural_language,
            keep,
            debug_state,
        } => {
            inline::run(
                command,
                natural_language,
                cli.api_endpoint,
                cli.api_token,
                keep,
                debug_state,
            )
            .await
        }
        #[cfg(debug_assertions)]
        Commands::DebugRender { input, format } => {
            let output_format = match format.as_str() {
                "plain" => debug_render::OutputFormat::Plain,
                "json" => debug_render::OutputFormat::Json,
                _ => debug_render::OutputFormat::Ansi,
            };
            debug_render::run(input, output_format).await
        }
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

pub fn detect_shell() -> Option<String> {
    Some(Shell::current().to_string())
}
