use std::{
    fs,
    path::{Path, PathBuf},
};

use atuin_common::shell::Shell;
use clap::{Parser, Subcommand};
use eyre::Result;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
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

    let settings = atuin_client::settings::Settings::new()?;

    if settings.logs.ai_enabled() {
        init_logging(&settings, cli.verbose)?;
    }

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
                &settings,
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

pub fn detect_shell() -> Option<String> {
    Some(Shell::current().to_string())
}

/// Initializes logging for the AI commands.
fn init_logging(settings: &atuin_client::settings::Settings, verbose: bool) -> Result<()> {
    // ATUIN_LOG env var overrides config file level settings
    let env_log_set = std::env::var("ATUIN_LOG").is_ok();

    // Base filter from env var (or empty if not set)
    let base_filter =
        EnvFilter::from_env("ATUIN_LOG").add_directive("sqlx_sqlite::regexp=off".parse()?);

    // Use config level unless ATUIN_LOG is set
    let filter = if env_log_set {
        base_filter
    } else {
        EnvFilter::default()
            .add_directive(settings.logs.ai_level().as_directive().parse()?)
            .add_directive("sqlx_sqlite::regexp=off".parse()?)
    };

    let log_dir = PathBuf::from(&settings.logs.dir);
    fs::create_dir_all(&log_dir)?;

    let filename = settings.logs.ai.file.clone();

    // Clean up old log files
    cleanup_old_logs(&log_dir, &filename, settings.logs.ai_retention());

    let console_layer = if verbose {
        Some(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(true)
                .with_target(false)
                .with_filter(filter.clone()),
        )
    } else {
        None
    };

    let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, &filename);

    let base = tracing_subscriber::registry().with(
        fmt::layer()
            .with_writer(file_appender)
            .with_ansi(false)
            .with_filter(filter),
    );

    if let Some(console_layer) = console_layer {
        base.with(console_layer).init();
    } else {
        base.init();
    };

    Ok(())
}

fn cleanup_old_logs(log_dir: &Path, prefix: &str, retention_days: u64) {
    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs(retention_days * 24 * 60 * 60);

    let Ok(entries) = fs::read_dir(log_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        // Match files like "search.log.2024-02-23" or "daemon.log.2024-02-23"
        if !name.starts_with(prefix) || name == prefix {
            continue;
        }

        if let Ok(metadata) = entry.metadata()
            && let Ok(modified) = metadata.modified()
            && modified < cutoff
        {
            let _ = fs::remove_file(&path);
        }
    }
}
