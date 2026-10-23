#![warn(clippy::pedantic, clippy::nursery)]
#![allow(clippy::use_self, clippy::missing_const_for_fn)] // not 100% reliable

use clap::Parser;
use clap::builder::Styles;
use clap::builder::styling::{AnsiColor, Effects};
use clap_couture::{Couture, CoutureParser};
use command::AtuinCmd;
use eyre::Result;

use crate::i18n::fl;

mod command;
mod i18n;
pub(crate) mod logs;
#[cfg(feature = "client")]
pub(crate) mod shell;

#[cfg(feature = "sync")]
mod print_error;
#[cfg(feature = "sync")]
mod sync;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const SHA: &str = env!("GIT_HASH");

const LONG_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("GIT_HASH"), ")");

const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Yellow.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Green.on_default());

#[derive(Parser, Couture)]
#[command(
    author = "Ellie Huxtable <ellie@atuin.sh>",
    version = VERSION,
    long_version = LONG_VERSION,
    styles = STYLES,
    about = fl!("cmd-atuin"),
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
    Atuin::couture_parse().run()
}
