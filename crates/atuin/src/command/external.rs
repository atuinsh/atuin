use std::fmt::Write as _;
use std::process::Command;
use std::{io, process};

#[cfg(feature = "client")]
use atuin_client::plugin::{OfficialPluginRegistry, PluginContext};
use clap::CommandFactory;
use clap::builder::{StyledStr, Styles};
use eyre::Result;

use crate::Atuin;
use crate::i18n::writet;

pub fn run(args: &[String]) -> Result<()> {
    let subcommand = &args[0];
    let bin = format!("atuin-{subcommand}");
    let mut cmd = Command::new(&bin);
    cmd.args(&args[1..]);

    #[cfg(feature = "client")]
    let context = PluginContext::new(subcommand);

    let spawn_result = match cmd.spawn() {
        Ok(child) => Ok(child),
        Err(e) => match e.kind() {
            io::ErrorKind::NotFound => {
                let output = render_not_found(subcommand, &bin);
                Err(output)
            }
            _ => Err(e.to_string().into()),
        },
    };

    match spawn_result {
        Ok(mut child) => {
            let status = child.wait()?;
            if status.success() {
                Ok(())
            } else {
                #[cfg(feature = "client")]
                drop(context);

                process::exit(status.code().unwrap_or(1));
            }
        }
        Err(e) => {
            eprintln!("{}", e.ansi());

            #[cfg(feature = "client")]
            drop(context);

            process::exit(1);
        }
    }
}

fn render_not_found(subcommand: &str, bin: &str) -> StyledStr {
    let mut output = StyledStr::new();
    let styles = Styles::styled();

    let error = styles.get_error();
    let invalid = styles.get_invalid();
    let literal = styles.get_literal();

    #[cfg(feature = "client")]
    {
        let registry = OfficialPluginRegistry::new();

        // Check if this is an official plugin
        if let Some(install_message) = registry.get_install_message(subcommand) {
            let _ = write!(output, "{error}error:{error:#} ");
            let _ = write!(
                output,
                "'{invalid}{subcommand}{invalid:#}' is an official atuin plugin, but it's not \
                 installed"
            );
            let _ = write!(output, "\n\n");
            let _ = write!(output, "{install_message}");
            return output;
        }
    }

    let mut atuin_cmd = Atuin::command();
    let usage = atuin_cmd.render_usage();

    let _ = write!(output, "{error}error:{error:#} ");
    let _ = writet!(
        output,
        "unrecognized-subcommand",
        subcommand = format!("{invalid}{subcommand}{invalid:#}"),
        bin = format!("{invalid}{bin}{invalid:#}"),
    );
    let _ = write!(output, "\n\n");
    let _ = write!(output, "{usage}");
    let _ = write!(output, "\n\n");
    let _ = write!(output, "For more information, try '{literal}--help{literal:#}'.");

    output
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::render_not_found;

    #[rstest]
    fn not_found_names_the_subcommand_and_its_binary() {
        let rendered = render_not_found("frobnicate", "atuin-frobnicate").to_string();
        assert!(
            rendered.starts_with(
                "error: unrecognized subcommand 'frobnicate' and no executable named \
                 'atuin-frobnicate' found in your PATH\n\n"
            ),
            "{rendered:?}"
        );
    }
}
