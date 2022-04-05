#![warn(clippy::pedantic, clippy::nursery)]
#![allow(clippy::use_self)] // not 100% reliable

use clap::AppSettings;
use clap::Parser;
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
    async fn run(self) -> Result<()> {
        self.atuin.run().await
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    pretty_env_logger::init();

    Atuin::parse().run().await
}
