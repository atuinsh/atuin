use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use clap::Subcommand;
use eyre::{Result, WrapErr};

use atuin_client::{
    database::Sqlite, record::sqlite_store::SqliteStore, settings::Settings, theme,
};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{
    Layer, filter::EnvFilter, filter::LevelFilter, fmt, fmt::format::FmtSpan, prelude::*,
};

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

#[cfg(feature = "sync")]
mod sync;

#[cfg(feature = "sync")]
mod account;

#[cfg(feature = "daemon")]
mod daemon;

mod default_config;
mod doctor;
mod dotfiles;
mod history;
mod import;
mod info;
mod init;
mod kv;
mod scripts;
mod search;
mod stats;
mod store;
mod wrapped;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Manipulate shell history
    #[command(subcommand)]
    History(history::Cmd),

    /// Import shell history from file
    #[command(subcommand)]
    Import(import::Cmd),

    /// Calculate statistics for your history
    Stats(stats::Cmd),

    /// Interactive history search
    Search(search::Cmd),

    #[cfg(feature = "sync")]
    #[command(flatten)]
    Sync(sync::Cmd),

    /// Manage your sync account
    #[cfg(feature = "sync")]
    Account(account::Cmd),

    /// Get or set small key-value pairs
    #[command(subcommand)]
    Kv(kv::Cmd),

    /// Manage the atuin data store
    #[command(subcommand)]
    Store(store::Cmd),

    /// Manage your dotfiles with Atuin
    #[command(subcommand)]
    Dotfiles(dotfiles::Cmd),

    /// Manage your scripts with Atuin
    #[command(subcommand)]
    Scripts(scripts::Cmd),

    /// Print Atuin's shell init script
    #[command()]
    Init(init::Cmd),

    /// Information about dotfiles locations and ENV vars
    #[command()]
    Info,

    /// Run the doctor to check for common issues
    #[command()]
    Doctor,

    #[command()]
    Wrapped { year: Option<i32> },

    /// *Experimental* Manage the background daemon
    #[cfg(feature = "daemon")]
    #[command()]
    Daemon(daemon::Cmd),

    /// Print the default atuin configuration (config.toml)
    #[command()]
    DefaultConfig,
}

impl Cmd {
    pub fn run(self) -> Result<()> {
        // Daemonize before creating the async runtime – fork() inside a live
        // tokio runtime corrupts its internal state.
        #[cfg(all(unix, feature = "daemon"))]
        if let Self::Daemon(ref cmd) = self
            && cmd.should_daemonize()
        {
            daemon::daemonize_current_process()?;
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let settings = Settings::new().wrap_err("could not load client settings")?;
        let theme_manager = theme::ThemeManager::new(settings.theme.debug, None);
        let res = runtime.block_on(self.run_inner(settings, theme_manager));

        runtime.shutdown_timeout(std::time::Duration::from_millis(50));

        res
    }

    #[allow(clippy::too_many_lines)]
    async fn run_inner(
        self,
        mut settings: Settings,
        mut theme_manager: theme::ThemeManager,
    ) -> Result<()> {
        // ATUIN_LOG env var overrides config file level settings
        let env_log_set = std::env::var("ATUIN_LOG").is_ok();

        // Base filter from env var (or empty if not set)
        let base_filter =
            EnvFilter::from_env("ATUIN_LOG").add_directive("sqlx_sqlite::regexp=off".parse()?);

        let is_interactive_search = matches!(&self, Self::Search(cmd) if cmd.is_interactive());
        // Use file-based logging for interactive search (TUI mode)
        let use_search_logging = is_interactive_search && settings.logs.search_enabled();

        // Use file-based logging for daemon
        #[cfg(feature = "daemon")]
        let use_daemon_logging = matches!(&self, Self::Daemon(_)) && settings.logs.daemon_enabled();

        #[cfg(not(feature = "daemon"))]
        let use_daemon_logging = false;

        // Check if daemon should also log to console
        #[cfg(feature = "daemon")]
        let daemon_show_logs = matches!(&self, Self::Daemon(cmd) if cmd.show_logs());

        #[cfg(not(feature = "daemon"))]
        let daemon_show_logs = false;

        // Set up span timing JSON logs if ATUIN_SPAN is set
        let span_path = std::env::var("ATUIN_SPAN").ok().map(|p| {
            if p.is_empty() {
                "atuin-spans.json".to_string()
            } else {
                p
            }
        });

        // Helper to create span timing layer
        macro_rules! make_span_layer {
            ($path:expr) => {{
                let span_file = OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .write(true)
                    .open($path)?;
                Some(
                    fmt::layer()
                        .json()
                        .with_writer(span_file)
                        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
                        .with_filter(LevelFilter::TRACE),
                )
            }};
        }

        // Build the subscriber with all configured layers
        if use_search_logging {
            let search_filename = settings.logs.search.file.clone();
            let log_dir = PathBuf::from(&settings.logs.dir);
            fs::create_dir_all(&log_dir)?;

            // Clean up old log files
            cleanup_old_logs(&log_dir, &search_filename, settings.logs.search_retention());

            let file_appender =
                RollingFileAppender::new(Rotation::DAILY, &log_dir, &search_filename);

            // Use config level unless ATUIN_LOG is set
            let filter = if env_log_set {
                base_filter
            } else {
                EnvFilter::default()
                    .add_directive(settings.logs.search_level().as_directive().parse()?)
                    .add_directive("sqlx_sqlite::regexp=off".parse()?)
            };

            let base = tracing_subscriber::registry().with(
                fmt::layer()
                    .with_writer(file_appender)
                    .with_ansi(false)
                    .with_filter(filter),
            );

            match &span_path {
                Some(sp) => {
                    base.with(make_span_layer!(sp)).init();
                }
                None => {
                    base.init();
                }
            }
        } else if use_daemon_logging {
            let daemon_filename = settings.logs.daemon.file.clone();
            let log_dir = PathBuf::from(&settings.logs.dir);
            fs::create_dir_all(&log_dir)?;

            // Clean up old log files
            cleanup_old_logs(&log_dir, &daemon_filename, settings.logs.daemon_retention());

            let file_appender =
                RollingFileAppender::new(Rotation::DAILY, &log_dir, &daemon_filename);

            // Use config level unless ATUIN_LOG is set
            let file_filter = if env_log_set {
                base_filter
            } else {
                EnvFilter::default()
                    .add_directive(settings.logs.daemon_level().as_directive().parse()?)
                    .add_directive("sqlx_sqlite::regexp=off".parse()?)
            };

            let file_layer = fmt::layer()
                .with_writer(file_appender)
                .with_ansi(false)
                .with_filter(file_filter);

            // Optionally add console layer for --show-logs
            if daemon_show_logs {
                let console_filter = EnvFilter::from_env("ATUIN_LOG")
                    .add_directive("sqlx_sqlite::regexp=off".parse()?);

                let console_layer = fmt::layer().with_filter(console_filter);

                let base = tracing_subscriber::registry()
                    .with(file_layer)
                    .with(console_layer);

                match &span_path {
                    Some(sp) => {
                        base.with(make_span_layer!(sp)).init();
                    }
                    None => {
                        base.init();
                    }
                }
            } else {
                let base = tracing_subscriber::registry().with(file_layer);

                match &span_path {
                    Some(sp) => {
                        base.with(make_span_layer!(sp)).init();
                    }
                    None => {
                        base.init();
                    }
                }
            }
        }

        tracing::trace!(command = ?self, "client command");

        // Skip initializing any databases for history
        // This is a pretty hot path, as it runs before and after every single command the user
        // runs
        match self {
            Self::History(history) => return history.run(&settings).await,
            Self::Init(init) => return init.run(&settings).await,
            Self::Doctor => return doctor::run(&settings).await,
            _ => {}
        }

        let db_path = PathBuf::from(settings.db_path.as_str());
        let record_store_path = PathBuf::from(settings.record_store_path.as_str());

        let db = Sqlite::new(db_path, settings.local_timeout).await?;
        let sqlite_store = SqliteStore::new(record_store_path, settings.local_timeout).await?;

        let theme_name = settings.theme.name.clone();
        let theme = theme_manager.load_theme(theme_name.as_str(), settings.theme.max_depth);

        match self {
            Self::Import(import) => import.run(&db).await,
            Self::Stats(stats) => stats.run(&db, &settings, theme).await,
            Self::Search(search) => search.run(db, &mut settings, sqlite_store, theme).await,

            #[cfg(feature = "sync")]
            Self::Sync(sync) => sync.run(settings, &db, sqlite_store).await,

            #[cfg(feature = "sync")]
            Self::Account(account) => account.run(settings, sqlite_store).await,

            Self::Kv(kv) => kv.run(&settings, &sqlite_store).await,

            Self::Store(store) => store.run(&settings, &db, sqlite_store).await,

            Self::Dotfiles(dotfiles) => dotfiles.run(&settings, sqlite_store).await,

            Self::Scripts(scripts) => scripts.run(&settings, sqlite_store, &db).await,

            Self::Info => {
                info::run(&settings);
                Ok(())
            }

            Self::DefaultConfig => {
                default_config::run();
                Ok(())
            }

            Self::Wrapped { year } => wrapped::run(year, &db, &settings, sqlite_store, theme).await,

            #[cfg(feature = "daemon")]
            Self::Daemon(cmd) => cmd.run(settings, sqlite_store, db).await,

            Self::History(_) | Self::Init(_) | Self::Doctor => unreachable!(),
        }
    }
}
