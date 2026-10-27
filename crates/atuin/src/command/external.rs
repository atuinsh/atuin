use std::process::Command;
use std::{io, process};

#[cfg(feature = "client")]
use atuin_client::plugin::{OfficialPluginRegistry, PluginContext};
use clap::CommandFactory;
use clap::error::ErrorKind;
use eyre::{Context as _, Result};

use crate::Atuin;
use crate::i18n::fl;

pub fn run(args: &[String]) -> Result<()> {
    let subcommand = &args[0];
    let bin = format!("atuin-{subcommand}");
    let mut cmd = Command::new(&bin);
    cmd.args(&args[1..]);

    #[cfg(feature = "client")]
    let context = PluginContext::new(subcommand);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let err = not_found(subcommand, &bin);
            // clap colours the error only on a terminal, and exits as its own usage errors do.
            let _ = err.print();

            #[cfg(feature = "client")]
            drop(context);

            process::exit(err.exit_code());
        }
        Err(e) => return Err(e).wrap_err_with(|| format!("failed to run {bin}")),
    };

    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        #[cfg(feature = "client")]
        drop(context);

        process::exit(status.code().unwrap_or(1));
    }
}

/// The error for a subcommand that is neither atuin's own nor an `atuin-<subcommand>` on `PATH`.
fn not_found(subcommand: &str, bin: &str) -> clap::Error {
    #[cfg(feature = "client")]
    if let Some(install_message) = OfficialPluginRegistry::new().get_install_message(subcommand) {
        return clap::Error::raw(
            ErrorKind::InvalidSubcommand,
            format!(
                "'{subcommand}' is an official atuin plugin, but it's not \
                 installed\n\n{install_message}\n"
            ),
        );
    }

    let message = fl!("unrecognized-subcommand", subcommand = subcommand, bin = bin);
    Atuin::command().error(ErrorKind::InvalidSubcommand, message)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::not_found;

    #[rstest]
    fn not_found_names_the_subcommand_and_its_binary() {
        let rendered = not_found("frobnicate", "atuin-frobnicate").to_string();
        assert!(
            rendered.starts_with(
                "error: unrecognized subcommand 'frobnicate' and no executable named \
                 'atuin-frobnicate' found in your PATH\n\n"
            ),
            "{rendered:?}"
        );
    }
}
