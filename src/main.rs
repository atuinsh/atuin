#![warn(clippy::pedantic, clippy::nursery)]
#![allow(clippy::use_self)] // not 100% reliable

use eyre::Result;
use structopt::{clap::AppSettings, StructOpt};

#[macro_use]
extern crate log;

use command::AtuinCmd;

mod command;

#[derive(StructOpt)]
#[structopt(
    author = "Ellie Huxtable <e@elm.sh>",
    version = "0.5.0",
    about = "Magical shell history",
    global_settings(&[AppSettings::ColoredHelp, AppSettings::DeriveDisplayOrder])
)]
struct Atuin {
    #[structopt(subcommand)]
    atuin: AtuinCmd,
}

impl Atuin {
    async fn run(self) -> Result<()> {
        self.atuin.run().await
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    pretty_env_logger::init();

    Atuin::from_args().run().await
}
