use atuin_client::settings::{Settings, UpdateChannel};
use axoupdater::{AxoUpdater, UpdateRequest};
use clap::Parser;
use eyre::{Result, bail, eyre};
use tracing::instrument;

#[derive(Parser, Debug)]
pub struct Cmd {
    /// Check whether an update is available without installing it
    #[arg(long)]
    check: bool,

    /// Update (or roll back) to a specific version, e.g. "18.9.0" or
    /// "18.9.0-nightly.1", instead of the channel's latest release
    #[arg(long, conflicts_with = "check")]
    version: Option<String>,
}

impl Cmd {
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(self, settings: &Settings) -> Result<()> {
        let current = env!("CARGO_PKG_VERSION");

        let mut updater = AxoUpdater::new_for("atuin");
        updater.disable_installer_output();

        // The install receipt is written by the shell/powershell installers.
        // No receipt means atuin came from a package manager, which should
        // stay in charge of upgrades.
        if updater.load_receipt().is_err() {
            bail!(
                "`atuin update` is only available when atuin was installed via the standalone installer (https://setup.atuin.sh).\n\
                 If you installed atuin with a package manager (brew, nix, pacman, cargo, ...), please update it there instead."
            );
        }

        // The receipt records the version the installer wrote; trust the
        // binary itself in case it was replaced by other means.
        let _ = updater.set_current_version(current.parse()?);

        if !updater.check_receipt_is_for_this_executable()? {
            let current_exe = std::env::current_exe()?;
            let receipt_prefix = updater.install_prefix_root()?;
            bail!(
                "This atuin binary ({}) is not the one the standalone installer installed to {}. \
                 Are multiple copies of atuin installed?",
                current_exe.display(),
                receipt_prefix,
            );
        }

        let request = match (&self.version, settings.update_channel) {
            (Some(version), _) => {
                UpdateRequest::SpecificVersion(version.trim_start_matches('v').to_string())
            }
            (None, UpdateChannel::Stable) => UpdateRequest::Latest,
            (None, UpdateChannel::Nightly) => UpdateRequest::LatestMaybePrerelease,
        };
        updater.configure_version_specifier(request);

        let channel = match settings.update_channel {
            UpdateChannel::Stable => "stable",
            UpdateChannel::Nightly => "nightly",
        };

        if self.check {
            println!("Checking for updates on the {channel} channel...");
            return match updater.query_new_version().await? {
                Some(new) if new > &current.parse()? => {
                    println!("Update available: v{current} -> v{new}");
                    println!("Run `atuin update` to install it");
                    Ok(())
                }
                _ => {
                    println!("atuin v{current} is up to date");
                    Ok(())
                }
            };
        }

        match &self.version {
            Some(version) => println!("Updating to atuin v{version}..."),
            None => println!("Checking for updates on the {channel} channel..."),
        }

        match updater.run().await? {
            Some(result) => {
                println!(
                    "Updated atuin v{current} -> v{} ({})",
                    result.new_version, result.new_version_tag
                );
                println!("Restart your shell sessions to pick up the new version");
                Ok(())
            }
            None if self.version.is_some() => Err(eyre!("update did not run")),
            None => {
                println!("atuin v{current} is up to date");
                Ok(())
            }
        }
    }
}
