use std::path::PathBuf;

use clap::Subcommand;
use eyre::{Result, WrapErr};

use atuin_client::{database::Sqlite, record::sqlite_store::SqliteStore, settings::Settings};
use env_logger::Builder;

#[cfg(feature = "sync")]
mod sync;

#[cfg(feature = "sync")]
mod account;

mod history;
mod import;
mod kv;
mod search;
mod stats;

#[derive(Subcommand)]
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
}

impl Cmd {
    #[tokio::main(flavor = "current_thread")]
    pub async fn run(self) -> Result<()> {
        Builder::new()
            .filter_level(log::LevelFilter::Off)
            .parse_env("ATUIN_LOG")
            .init();

        let mut settings = Settings::new().wrap_err("could not load client settings")?;

        let db_path = PathBuf::from(settings.db_path.as_str());
        let record_store_path = PathBuf::from(settings.record_store_path.as_str());

        let mut db = Sqlite::new(db_path).await?;
        let mut store = SqliteStore::new(record_store_path).await?;

        match self {
            Self::History(history) => history.run(&settings, &mut db).await,
            Self::Import(import) => import.run(&mut db).await,
            Self::Stats(stats) => stats.run(&mut db, &settings).await,
            Self::Search(search) => search.run(db, &mut settings).await,

            #[cfg(feature = "sync")]
            Self::Sync(sync) => sync.run(settings, &mut db).await,

            #[cfg(feature = "sync")]
            Self::Account(account) => account.run(settings).await,

            Self::Kv(kv) => kv.run(&settings, &mut store).await,
        }
    }
}
