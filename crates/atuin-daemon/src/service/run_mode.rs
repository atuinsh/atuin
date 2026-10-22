use atuin_client::settings::Daemon as DaemonSettings;
use serde::{Deserialize, Serialize};

use crate::service::backend::Launchd;
use crate::service::systemd::Systemd;

/// How `atuin daemon start` was run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    /// Forked off a client, with `--daemonize`, as clients start the daemon.
    Daemonize,
    /// In place, as launchd, a service manager or a user runs it.
    InPlace,
}

/// How the daemon runs, which it records in its pidfile: clients start and stop a daemon that
/// Atuin runs, and leave one that something else does to that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    /// A client forked it, with `daemon start --daemonize`.
    DaemonizeForked,
    /// Atuin's launchd agent, `sh.atuin.daemon`.
    LaunchdAgent,
    /// Something other than Atuin started it. [`None`] when the daemon couldn't tell what.
    External(Option<ExternalManager>),
}

/// What starts and stops the daemon in Atuin's place, and so the error clients refuse to start or
/// stop it with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum ExternalManager {
    /// A systemd socket unit, which hands the daemon its socket.
    #[error(
        "the daemon is managed by systemd (`daemon.systemd_socket = true`); use `systemctl \
         --user` to start, stop or restart it"
    )]
    SystemdSocket,
    /// A systemd user service, without socket activation.
    #[error(
        "the daemon is run by the systemd unit `{name}`; use `systemctl --user restart {name}` to \
         restart it, or `systemctl --user stop {name}` to stop it"
    )]
    SystemdUnit {
        name: String,
    },
    /// A launchd job other than Atuin's agent, such as `brew services`' `homebrew.mxcl.atuin`.
    #[error("{}", Self::launchd_job_refusal(label))]
    LaunchdJob {
        label: String,
    },
}

impl RunMode {
    /// How this process runs as the daemon `daemon` describes, `launch`ed so, when launchd didn't
    /// check it in as Atuin's agent.
    pub(crate) fn detect(daemon: &DaemonSettings, launch: Launch) -> Self {
        match launch {
            Launch::Daemonize => Self::DaemonizeForked,
            Launch::InPlace => Self::External(ExternalManager::detect(daemon)),
        }
    }
}

impl ExternalManager {
    /// What runs this process, which was started in place, as far as it can tell.
    fn detect(daemon: &DaemonSettings) -> Option<Self> {
        if daemon.systemd_socket {
            return Some(Self::SystemdSocket);
        }
        Launchd::external_job_label()
            .map(|label| Self::LaunchdJob { label })
            .or_else(|| Systemd::unit_of_this_process().map(|name| Self::SystemdUnit { name }))
    }

    /// The refusal for the launchd job `label`, with the command that restarts it.
    fn launchd_job_refusal(label: &str) -> String {
        match label.strip_prefix("homebrew.mxcl.") {
            Some(formula) => format!(
                "the daemon is managed by `brew services`; use `brew services restart {formula}` \
                 to restart it, or `brew services stop {formula}` to let Atuin manage it"
            ),
            None => format!(
                "the daemon is run by the launchd job `{label}`; use `launchctl kickstart -k \
                 gui/$(id -u)/{label}` to restart it"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::brew_services("homebrew.mxcl.atuin", "`brew services restart atuin`")]
    #[case::other_job(
        "org.nix-community.home.atuin-daemon",
        "`launchctl kickstart -k gui/$(id -u)/org.nix-community.home.atuin-daemon`"
    )]
    fn a_launchd_job_is_left_to_the_command_that_restarts_it(
        #[case] label: &str,
        #[case] command: &str,
    ) {
        let refusal = ExternalManager::LaunchdJob {
            label: label.to_owned(),
        }
        .to_string();
        assert!(refusal.contains(command), "{refusal}");
    }
}
