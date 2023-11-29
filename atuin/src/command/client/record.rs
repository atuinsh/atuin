use clap::Subcommand;
use eyre::Result;

use atuin_client::{record::store::Store, settings::Settings};

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    Status,
}

impl Cmd {
    pub async fn run(
        &self,
        settings: &Settings,
        store: &mut (impl Store + Send + Sync),
    ) -> Result<()> {
        let host_id = Settings::host_id().expect("failed to get host_id");
        Ok(())
    }
}
