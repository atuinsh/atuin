//! Internal subcommands, not for direct use by users.

use atuin_client::settings::Settings;
use atuin_common::logs::LogConfig;

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

    pub fn log_config(&self) -> Option<LogConfig> {
        match self {
            // This command is called from the shell hooks with no stdout/stderr; there's no point
            // in initializing logging.
            Self::PrepareSearchIndex => None,
        }
    }
}
