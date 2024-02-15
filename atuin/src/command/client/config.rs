use clap::Subcommand;
use eyre::Result;

use atuin_client::{record::sqlite_store::SqliteStore, settings::Settings};

mod alias;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    #[command(subcommand)]
    Alias(alias::Cmd),
}

impl Cmd {
    pub async fn run(self, settings: &Settings, store: SqliteStore) -> Result<()> {
        match self {
            Self::Alias(cmd) => cmd.run(settings, store).await,
        }
    }
}
