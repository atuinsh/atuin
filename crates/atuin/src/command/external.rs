use std::fmt::Write as _;
use std::process::Command;
use std::{io, process};

#[cfg(feature = "client")]
use atuin_client::plugin::OfficialPluginRegistry;
use clap::CommandFactory;
use clap::builder::{StyledStr, Styles};
use eyre::Result;

use crate::Atuin;

pub fn run(args: &[String]) -> Result<()> {
    let subcommand = &args[0];
    let bin = format!("atuin-{subcommand}");
    let mut cmd = Command::new(&bin);
    cmd.args(&args[1..]);

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
                process::exit(status.code().unwrap_or(1));
            }
        }
        Err(e) => {
            eprintln!("{}", e.ansi());
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
                "'{invalid}{subcommand}{invalid:#}' is an official atuin plugin, but it's not installed"
            );
            let _ = write!(output, "\n\n");
            let _ = write!(output, "{install_message}");
            return output;
        }
    }

    let mut atuin_cmd = Atuin::command();
    let usage = atuin_cmd.render_usage();

    let _ = write!(output, "{error}error:{error:#} ");
    let _ = write!(
        output,
        "unrecognized subcommand '{invalid}{subcommand}{invalid:#}' "
    );
    let _ = write!(
        output,
        "and no executable named '{invalid}{bin}{invalid:#}' found in your PATH"
    );
    let _ = write!(output, "\n\n");
    let _ = write!(output, "{usage}");
    let _ = write!(output, "\n\n");
    let _ = write!(
        output,
        "For more information, try '{literal}--help{literal:#}'."
    );

    output
}
