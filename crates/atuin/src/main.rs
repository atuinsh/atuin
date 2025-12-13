#![warn(clippy::pedantic, clippy::nursery)]
#![allow(clippy::use_self, clippy::missing_const_for_fn)] // not 100% reliable

use clap::Parser;
use clap::builder::Styles;
use clap::builder::styling::{AnsiColor, Effects};
use eyre::Result;

use command::AtuinCmd;

mod command;

#[cfg(feature = "sync")]
mod sync;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const SHA: &str = env!("GIT_HASH");

const LONG_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("GIT_HASH"), ")");

static HELP_TEMPLATE: &str = "\
{before-help}{name} {version}
{author}
{about}

{usage-heading}
  {usage}

{all-args}{after-help}";

const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Yellow.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Green.on_default());

/// Magical shell history
#[derive(Parser)]
#[command(
    author = "Ellie Huxtable <ellie@atuin.sh>",
    version = VERSION,
    long_version = LONG_VERSION,
    help_template(HELP_TEMPLATE),
    styles = STYLES,
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
