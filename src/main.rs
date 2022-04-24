#![forbid(unsafe_code)]
#![warn(clippy::pedantic, clippy::nursery)]

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
    Atuin::parse().run().await
}
