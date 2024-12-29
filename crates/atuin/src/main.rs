#![warn(clippy::pedantic, clippy::nursery)]
#![allow(clippy::use_self, clippy::missing_const_for_fn)] // not 100% reliable
#[macro_use]
extern crate rust_i18n;

use clap::Parser;
use eyre::Result;
use sys_locale::get_locale;

use atuin_client;

use command::AtuinCmd;

mod command;

#[cfg(feature = "sync")]
mod sync;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const SHA: &str = env!("GIT_HASH");

static HELP_TEMPLATE: &str = "\
{before-help}{name} {version}
{author}
{about}

{usage-heading}
  {usage}

{all-args}{after-help}";

i18n!("locales", fallback = "en");

/// Magical shell history
#[derive(Parser)]
#[command(
    author = "Ellie Huxtable <ellie@atuin.sh>",
    version = VERSION,
    help_template(HELP_TEMPLATE),
)]
struct Atuin {
    #[command(subcommand)]
    atuin: AtuinCmd,
}

impl Atuin {
    fn run(self) -> Result<()> {
        let locale = get_locale().unwrap_or_else(|| String::from("en"));

        rust_i18n::set_locale(locale.as_str());
        atuin_client::set_locale(locale.as_str());

        self.atuin.run()
    }
}

fn main() -> Result<()> {
    Atuin::parse().run()
}
