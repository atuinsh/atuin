use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use clap::Subcommand;
use eyre::Result;
use tracing::instrument;

mod alias;
mod var;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Manage shell aliases with Atuin
    #[command(subcommand)]
    Alias(alias::Cmd),

    /// Manage shell and environment variables with Atuin
    #[command(subcommand)]
    Var(var::Cmd),
}

impl Cmd {
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(self, settings: &Settings, store: SqliteStore) -> Result<()> {
        match self {
            Self::Alias(cmd) => cmd.run(settings, store).await,
            Self::Var(cmd) => cmd.run(settings, store).await,
        }
    }
}
