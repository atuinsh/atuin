use std::{
    fs,
    path::{Path, PathBuf},
};

use atuin_common::shell::Shell;
use clap::{Args, Subcommand};
use eyre::Result;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};
pub mod init;
pub mod inline;

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
pub enum Commands {
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

pub async fn run(
    command: Commands,
    settings: &atuin_client::settings::Settings,
) -> eyre::Result<()> {
    match command {
        Commands::Init { shell } => init::run(shell).await,
        Commands::Inline {
            command,
            hook,
            args,
            ..
        } => {
            if settings.logs.ai_enabled() {
                init_logging(settings, args.verbose)?;
            }

            inline::run(command, args.api_endpoint, args.api_token, settings, hook).await
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
    let ai_log_filename = settings.logs.ai.file.clone();

    // Clean up old log files
    cleanup_old_logs(&log_dir, &ai_log_filename, settings.logs.ai_retention());

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

    let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, &ai_log_filename);

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
