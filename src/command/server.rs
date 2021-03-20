use eyre::Result;
use structopt::StructOpt;

use crate::remote::server;
use crate::settings::Settings;

#[derive(StructOpt)]
pub enum Cmd {
    Start { host: Vec<String> },
}

#[allow(clippy::unused_self)] // I'll use it later
impl Cmd {
    pub fn run(&self, settings: &Settings) -> Result<()> {
        server::launch(settings);
        Ok(())
    }
}
