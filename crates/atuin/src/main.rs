#![warn(clippy::pedantic, clippy::nursery)]
#![allow(clippy::use_self, clippy::missing_const_for_fn)] // not 100% reliable

use std::ffi::OsString;

use clap::Parser;
use clap::builder::Styles;
use clap::builder::styling::{AnsiColor, Effects};
use eyre::Result;

use command::AtuinCmd;

mod command;
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

/// Whether this is `atuin hook <agent>`, the per-tool-call event an agent runs
/// itself — as opposed to `atuin hook install <agent>`, which a user runs and
/// should see the failures of.
fn is_agent_hook_event(args: &[OsString]) -> bool {
    let arg = |index: usize| args.get(index).and_then(|arg| arg.to_str());

    arg(0) == Some("hook")
        && arg(1).is_some_and(|agent| agent != "install" && !agent.starts_with('-'))
}

fn main() -> Result<()> {
    let args: Vec<OsString> = std::env::args_os().collect();

    // An agent reads the exit code of the hook it ran as a verdict on the
    // command that hook wrapped: Claude Code denies a Bash call outright when a
    // `PreToolUse` hook exits 2, which is also what clap exits on a bad
    // invocation. Recording history is best effort and never worth blocking
    // someone's command over, so this path always reports success — including
    // when it failed before reaching the hook itself.
    if is_agent_hook_event(&args[1..]) {
        if let Err(err) = Atuin::try_parse_from(&args)
            .map_err(eyre::Report::from)
            .and_then(Atuin::run)
        {
            eprintln!("atuin: hook failed: {err:#}");
        }

        return Ok(());
    }

    Atuin::parse_from(args).run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::event(&["hook", "claude-code"], true)]
    #[case::alias(&["hook", "claude"], true)]
    #[case::install(&["hook", "install", "claude-code"], false)]
    // Swallowing these would print nothing for `atuin hook --help`.
    #[case::help(&["hook", "--help"], false)]
    #[case::no_agent(&["hook"], false)]
    #[case::another_command(&["history", "start"], false)]
    fn recognizes_the_agent_hook_event(#[case] args: &[&str], #[case] expected: bool) {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();

        assert_eq!(is_agent_hook_event(&args), expected);
    }
}
