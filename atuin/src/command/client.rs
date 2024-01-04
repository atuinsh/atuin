use std::path::PathBuf;

use clap::Subcommand;
use eyre::{Result, WrapErr};

use atuin_client::{database::Sqlite, record::sqlite_store::SqliteStore, settings::Settings};
use env_logger::Builder;

#[cfg(feature = "sync")]
mod sync;

#[cfg(feature = "sync")]
mod account;

mod config;
mod history;
mod import;
mod kv;
mod record;
mod search;
mod stats;

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

    #[cfg(feature = "sync")]
    Account(account::Cmd),

    #[command(subcommand)]
    Kv(kv::Cmd),

    #[command(subcommand)]
    Record(record::Cmd),

    /// Print example configuration
    #[command()]
    DefaultConfig,
}

impl Cmd {
    pub fn run(self) -> Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let res = runtime.block_on(self.run_inner());

        runtime.shutdown_timeout(std::time::Duration::from_millis(50));

        res
    }

    async fn run_inner(self) -> Result<()> {
        Builder::new()
            .filter_level(log::LevelFilter::Off)
            .parse_env("ATUIN_LOG")
            .init();

        tracing::trace!(command = ?self, "client command");

        let mut settings = Settings::new().wrap_err("could not load client settings")?;

        let db_path = PathBuf::from(settings.db_path.as_str());
        let record_store_path = PathBuf::from(settings.record_store_path.as_str());

        let db = Sqlite::new(db_path).await?;
        let store = SqliteStore::new(record_store_path).await?;

        match self {
            Self::History(history) => history.run(&settings, &db, store).await,
            Self::Import(import) => import.run(&db).await,
            Self::Stats(stats) => stats.run(&db, &settings).await,
            Self::Search(search) => search.run(db, &mut settings).await,

            #[cfg(feature = "sync")]
            Self::Sync(sync) => sync.run(settings, &db, &store).await,

            #[cfg(feature = "sync")]
            Self::Account(account) => account.run(settings).await,

            Self::Kv(kv) => kv.run(&settings, &store).await,

            Self::Record(record) => record.run(&settings, &store).await,

            Self::DefaultConfig => {
                config::run();
                Ok(())
            }
        }
    }
}
