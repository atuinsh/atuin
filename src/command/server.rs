use eyre::Result;
use structopt::StructOpt;

use crate::remote::server;

#[derive(StructOpt)]
pub enum Cmd {
    Start { host: Vec<String> },
}

#[allow(clippy::unused_self)] // I'll use it later
impl Cmd {
    pub fn run(&self) -> Result<()> {
        server::launch();
        Ok(())
    }
}
