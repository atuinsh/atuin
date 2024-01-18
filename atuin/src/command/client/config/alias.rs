use clap::Parser;
use eyre::Result;

use atuin_client::settings::Settings;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    Create,
}

impl Cmd {
    pub async fn run(&self, settings: &Settings) -> Result<()> {
        println!("omg an alias");
        Ok(())
    }
}
