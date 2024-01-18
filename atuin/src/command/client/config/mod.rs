use clap::{Args, Subcommand};
use eyre::Result;

use atuin_client::settings::Settings;

pub mod alias;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    Alias(alias::Cmd),
}

impl Cmd {
    pub async fn run(self, settings: &Settings) -> Result<()> {
        match self {
            Cmd::Alias(cmd) => cmd.run(&settings).await,
        }
    }
}
