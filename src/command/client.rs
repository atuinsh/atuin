use std::path::PathBuf;

use clap::Subcommand;
use eyre::{Result, WrapErr};

use atuin_client::{database::Sqlite, settings::Settings};

#[cfg(feature = "sync")]
mod sync;

mod history;
mod import;
mod search;
mod stats;

#[derive(Subcommand)]
#[clap(infer_subcommands = true)]
pub enum Cmd {
    /// Manipulate shell history
    #[clap(subcommand)]
    History(history::Cmd),

    /// Import shell history from file
    #[clap(subcommand)]
    Import(import::Cmd),

    /// Calculate statistics for your history
    Stats(stats::Cmd),

    /// Interactive history search
    Search(search::Cmd),

    #[cfg(feature = "sync")]
    #[clap(flatten)]
    Sync(sync::Cmd),
}

impl Cmd {
    #[tokio::main(flavor = "current_thread")]
    pub async fn run(self) -> Result<()> {
        pretty_env_logger::init();

        let settings = Settings::new().wrap_err("could not load client settings")?;
        settings.needs_update().await;

        let db_path = PathBuf::from(settings.db_path.as_str());
        let mut db = Sqlite::new(db_path).await?;

        match self {
            Self::History(history) => history.run(&settings, &mut db).await,
            Self::Import(import) => import.run(&mut db).await,
            Self::Stats(stats) => stats.run(&mut db, &settings).await,
            Self::Search(search) => search.run(&mut db, &settings).await,
            #[cfg(feature = "sync")]
            Self::Sync(sync) => sync.run(settings, &mut db).await,
        }
    }
}
