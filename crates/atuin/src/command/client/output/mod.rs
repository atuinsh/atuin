use atuin_client::database::Sqlite;
use atuin_client::settings::Settings;
use atuin_client::theme::Theme;
use clap::Subcommand;
use eyre::Result;

mod search;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Full-text search over captured command output
    Search(search::Cmd),
}

impl Cmd {
    pub async fn run(self, db: &Sqlite, settings: &Settings, theme: &Theme) -> Result<()> {
        match self {
            Self::Search(cmd) => cmd.run(db, settings, theme).await.map_err(Into::into),
        }
    }
}
