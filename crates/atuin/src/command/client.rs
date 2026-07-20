use clap::Subcommand;
use eyre::{Result, WrapErr};

use atuin_client::logs::FromSettings;
use atuin_client::{
    database::Sqlite, record::sqlite_store::SqliteStore, settings::Settings, theme,
};
use atuin_common::logs::{self, LogConfig};

#[cfg(feature = "sync")]
mod sync;

#[cfg(feature = "sync")]
mod account;

#[cfg(feature = "daemon")]
mod daemon;

mod config;
mod default_config;
mod doctor;
mod dotfiles;
mod history;
mod hook;
mod import;
mod info;
mod init;
mod kv;
mod scripts;
mod search;
mod setup;
mod stats;
mod store;
mod wrapped;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Setup Atuin features
    #[command()]
    Setup,

    /// Manipulate shell history
    #[command(subcommand)]
    History(history::Cmd),

    /// Manage AI-agent shell hooks
    Hook(hook::Cmd),

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

    #[command(subcommand)]
    Config(config::Cmd),

    /// Run the AI assistant
    #[cfg(feature = "ai")]
    #[command(subcommand)]
    Ai(atuin_ai::commands::Command),

    /// Start an MCP server exposing history search to AI tools (stdio)
    #[cfg(feature = "ai")]
    #[command()]
    Mcp,
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

        #[cfg(feature = "ai")]
        let mut runtime = if matches!(&self, Self::Ai(_)) {
            tokio::runtime::Builder::new_multi_thread()
        } else {
            tokio::runtime::Builder::new_current_thread()
        };

        #[cfg(not(feature = "ai"))]
        let mut runtime = tokio::runtime::Builder::new_current_thread();

        let runtime = runtime.enable_all().build().unwrap();

        // For non-history commands, we want to initialize logging and the theme manager before
        // doing anything else. History commands are performance-sensitive and run before and after
        // every shell command, so we want to skip any unnecessary initialization for them.
        let settings = Settings::new().wrap_err("could not load client settings")?;
        self.init_logging(&settings);
        let theme_manager = theme::ThemeManager::new(settings.theme.debug, None);
        let res = runtime.block_on(self.run_inner(settings, theme_manager));

        runtime.shutdown_timeout(std::time::Duration::from_millis(50));

        res
    }

    #[allow(clippy::too_many_lines, clippy::future_not_send)]
    async fn run_inner(
        self,
        mut settings: Settings,
        mut theme_manager: theme::ThemeManager,
    ) -> Result<()> {
        tracing::trace!(command = ?self, "client command");

        // Skip initializing any databases for history
        // This is a pretty hot path, as it runs before and after every single command the user
        // runs
        match self {
            Self::History(history) => return history.run(&settings).await,
            Self::Hook(hook) => return hook.run(&settings).await,
            Self::Init(init) => return init.run(&settings).await,
            Self::Doctor => return doctor::run(&settings).await,
            Self::Config(config) => return config.run(&settings).await,
            _ => {}
        }

        let db_path = &settings.db_path;
        let record_store_path = &settings.record_store_path;

        let db = Sqlite::new(db_path, settings.local_timeout).await?;
        let sqlite_store = SqliteStore::new(record_store_path, settings.local_timeout).await?;

        let theme_name = settings.theme.name.clone();
        let theme = theme_manager.load_theme(theme_name.as_str(), settings.theme.max_depth);

        match self {
            Self::Setup => setup::run(&settings).await,
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

            Self::History(_) | Self::Hook(_) | Self::Init(_) | Self::Doctor | Self::Config(_) => {
                unreachable!()
            }

            #[cfg(feature = "ai")]
            Self::Ai(cli) => atuin_ai::commands::run(cli, &settings).await,

            #[cfg(feature = "ai")]
            Self::Mcp => atuin_ai::mcp::run(&db).await,
        }
    }

    fn log_config(&self, settings: &Settings) -> Option<LogConfig> {
        match self {
            Self::History(cmd) => cmd.log_config(),

            Self::Search(cmd) if cmd.is_interactive() => Some(LogConfig::from_settings(
                &settings.logs,
                &settings.logs.search,
            )),

            #[cfg(feature = "daemon")]
            Self::Daemon(cmd) => Some(LogConfig {
                file: logs::FileConfig::from_settings(&settings.logs, &settings.logs.daemon),
                stderr: cmd.show_logs().then(logs::StderrConfig::verbose),
            }),

            #[cfg(feature = "ai")]
            Self::Ai(cmd) => Some(cmd.log_config(settings)),

            _ => Some(LogConfig::stderr_only()),
        }
    }

    fn init_logging(&self, settings: &Settings) {
        if let Some(config) = self.log_config(settings) {
            crate::logs::init_logging(&config);
        }
    }
}
