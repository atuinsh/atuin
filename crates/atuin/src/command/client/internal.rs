//! Internal subcommands, not for direct use by users.

use atuin_client::settings::Settings;

#[derive(clap::Subcommand, Debug)]
pub enum Cmd {
    PrepareSearchIndex,
}

impl Cmd {
    pub async fn run(self, settings: &Settings) -> eyre::Result<()> {
        match self {
            Self::PrepareSearchIndex => super::search::prepare_index(settings).await,
        }
    }
}
