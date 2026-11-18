use atuin_client::settings::{Settings, UpdateChannel};
use atuin_common::userio;
use axoupdater::{AxoUpdater, UpdateRequest};
use clap::Parser;
use eyre::{Result, bail, eyre};
use tracing::instrument;

use crate::i18n::fl;

#[derive(Parser, Debug)]
pub struct Cmd {
    #[arg(long, help = fl!("arg-update-check"))]
    check: bool,

    #[arg(long, conflicts_with = "check", help = fl!("arg-update-version"))]
    version: Option<String>,

    #[arg(long, conflicts_with = "version", help = fl!("arg-update-prerelease"))]
    prerelease: bool,
}

impl Cmd {
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(self, settings: &Settings) -> Result<()> {
        let current = env!("CARGO_PKG_VERSION");

        let mut updater = AxoUpdater::new_for("atuin");
        updater.disable_installer_output();
        if let Ok(token) = atuin_common::env::var("ATUIN_GITHUB_TOKEN")
            && !token.is_empty()
        {
            updater.set_github_token(&token);
        }

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

        let update_channel = if self.prerelease {
            UpdateChannel::Nightly
        } else {
            settings.update_channel
        };

        let request = match (&self.version, update_channel) {
            (Some(version), _) => {
                UpdateRequest::SpecificVersion(version.trim_start_matches('v').to_string())
            }
            (None, UpdateChannel::Stable) => UpdateRequest::Latest,
            (None, UpdateChannel::Nightly) => UpdateRequest::LatestMaybePrerelease,
        };
        updater.configure_version_specifier(request);

        let channel = match update_channel {
            UpdateChannel::Stable => "stable",
            UpdateChannel::Nightly => "nightly",
        };

        if self.check {
            userio::log::info!("update-checking", channel = channel);
            return match updater.query_new_version().await? {
                Some(new) if new > &current.parse()? => {
                    println!("Update available: v{current} -> v{new}");
                    userio::log::info!("update-run-hint");
                    Ok(())
                }
                _ => {
                    println!("atuin v{current} is up to date");
                    Ok(())
                }
            };
        }

        if let Some(version) = &self.version {
            userio::log::info!("update-installing", version = version);
        } else {
            userio::log::info!("update-checking", channel = channel);
        }

        match updater.run().await? {
            Some(result) => {
                userio::log::info!(
                    "update-done",
                    current = current,
                    new_version = result.new_version.to_string(),
                    tag = result.new_version_tag
                );
                userio::log::info!("update-restart-shell");
                Ok(())
            }
            None if self.version.is_some() => Err(eyre!("update did not run")),
            None => {
                userio::log::info!("update-up-to-date", current = current);
                Ok(())
            }
        }
    }
}
