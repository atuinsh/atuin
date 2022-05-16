#![warn(clippy::pedantic, clippy::nursery)]
#![allow(clippy::use_self)] // not 100% reliable

use clap::{AppSettings, Parser};
use eyre::Result;

#[macro_use]
extern crate log;

use command::AtuinCmd;
mod command;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Magical shell history
#[derive(Parser)]
#[clap(
    author = "Ellie Huxtable <e@elm.sh>",
    version = VERSION,
    global_setting(AppSettings::DeriveDisplayOrder),
)]
struct Atuin {
    #[clap(subcommand)]
    atuin: AtuinCmd,
}

impl Atuin {
    fn run(self) -> Result<()> {
        self.atuin.run()
    }
}

fn main() -> Result<()> {
    Atuin::parse().run()
}
