use eyre::Result;
use structopt::StructOpt;

use crate::server::server;

#[derive(StructOpt)]
pub enum ServerCmd {
    Start { command: Vec<String> },
}

impl ServerCmd {
    pub fn run(&self) -> Result<()> {
        server::launch();
        Ok(())
    }
}
