#![warn(clippy::pedantic, clippy::nursery)]
#![allow(clippy::use_self, clippy::missing_const_for_fn)] // not 100% reliable

use clap::Parser;
use eyre::Result;

use command::AtuinCmd;
mod command;
mod engines;
mod interactive;
mod utils;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const SHA: &str = env!("GIT_HASH");

static HELP_TEMPLATE: &str = "\
{before-help}{name} {version}
{author}
{about}

{usage-heading}
  {usage}

{all-args}{after-help}";

/// Magical shell history
#[derive(Parser)]
#[command(
    author = "Ellie Huxtable <e@elm.sh>",
    version = VERSION,
    help_template(HELP_TEMPLATE),
)]
struct Atuin {
    #[command(subcommand)]
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
