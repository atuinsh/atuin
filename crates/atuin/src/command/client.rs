use std::path::PathBuf;

use clap::Subcommand;
use eyre::{Result, WrapErr};

use atuin_client::{
    database::Sqlite, record::sqlite_store::SqliteStore, settings::Settings, theme,
};
use tracing_subscriber::{filter::EnvFilter, fmt, prelude::*};

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
mod search;
mod stats;
mod store;

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

    /// Print Atuin's shell init script
    #[command()]
    Init(init::Cmd),

    /// Information about dotfiles locations and ENV vars
    #[command()]
    Info,

    /// Run the doctor to check for common issues
    #[command()]
    Doctor,

    /// *Experimental* Start the background daemon
    #[cfg(feature = "daemon")]
    #[command()]
    Daemon,

    /// Print the default atuin configuration (config.toml)
    #[command()]
    DefaultConfig,
}

impl Cmd {
    pub fn run(self) -> Result<()> {
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

    async fn run_inner(
        self,
        mut settings: Settings,
        mut theme_manager: theme::ThemeManager,
    ) -> Result<()> {
        let filter =
            EnvFilter::from_env("ATUIN_LOG").add_directive("sqlx_sqlite::regexp=off".parse()?);

        tracing_subscriber::registry()
            .with(fmt::layer())
            .with(filter)
            .init();

        tracing::trace!(command = ?self, "client command");

        // Skip initializing any databases for history
        // This is a pretty hot path, as it runs before and after every single command the user
        // runs
        match self {
            Self::History(history) => return history.run(&settings).await,
            Self::Init(init) => return init.run(&settings).await,
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

            Self::Info => {
                info::run(&settings);
                Ok(())
            }

            Self::Doctor => doctor::run(&settings).await,

            Self::DefaultConfig => {
                default_config::run();
                Ok(())
            }

            #[cfg(feature = "daemon")]
            Self::Daemon => daemon::run(settings, sqlite_store, db).await,

            _ => unimplemented!(),
        }
    }
}
