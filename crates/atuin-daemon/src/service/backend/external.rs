use atuin_client::settings::Daemon as DaemonSettings;

use crate::service::address::DaemonAddress;
use crate::service::backend::{Backend, BackendError};
use crate::service::connection::Connection;
use crate::service::pidfile::{Pidfile, PidfileInfo};
use crate::service::run_mode::{ExternalManager, RunMode};

/// Represents a backend for a service that something other than Atuin runs, through a unit or job
/// the user set up.
pub struct External {
    address: DaemonAddress,
    manager: ExternalManager,
}

impl External {
    #[must_use]
    pub fn new(daemon: &DaemonSettings, manager: ExternalManager) -> Self {
        Self {
            address: DaemonAddress::existing(daemon),
            manager,
        }
    }

    /// The backend for the service `daemon` describes, if something other than Atuin runs it: the
    /// systemd socket it is configured for, or what the daemon holding the pidfile says runs it.
    pub(super) fn for_client(daemon: &DaemonSettings) -> Option<Self> {
        if daemon.systemd_socket {
            return Some(Self::new(daemon, ExternalManager::SystemdSocket));
        }

        // It would only start again once stopped, and an agent of Atuin's would fight it for the
        // pidfile at every login.
        let pidfile = Pidfile::new(&daemon.pidfile_path);
        if !pidfile.is_live() {
            return None;
        }
        let PidfileInfo {
            run_mode: Some(RunMode::External(Some(manager))),
            ..
        } = pidfile.info()?
        else {
            return None;
        };
        Some(Self::new(daemon, manager))
    }
}

impl Backend for External {
    fn address(&self) -> DaemonAddress {
        self.address.clone()
    }

    async fn launch(&self) -> Result<(), BackendError> {
        Err(BackendError::new(self.manager.clone()))
    }

    async fn stop(&self, _service: &Connection) -> Result<(), BackendError> {
        Err(BackendError::new(self.manager.clone()))
    }

    async fn evict(&self) -> Result<(), BackendError> {
        Err(BackendError::new(self.manager.clone()))
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::settings::Settings;
    use atuin_common::path::AbsPathBuf;
    use rstest::rstest;

    use super::*;

    /// A client leaves the daemon to what runs it only while a daemon that says something else
    /// runs it holds the pidfile: not once it has exited, nor while one Atuin started, or one that
    /// couldn't tell, or one from before daemons said, does.
    #[rstest]
    #[case::launchd_job(
        Some(RunMode::External(Some(ExternalManager::LaunchdJob {
            label: "homebrew.mxcl.atuin".to_owned(),
        }))),
        true,
        true
    )]
    #[case::systemd_unit(
        Some(RunMode::External(Some(ExternalManager::SystemdUnit {
            name: "atuin-daemon.service".to_owned(),
        }))),
        true,
        true
    )]
    #[case::exited(
        Some(RunMode::External(Some(ExternalManager::LaunchdJob {
            label: "homebrew.mxcl.atuin".to_owned(),
        }))),
        false,
        false
    )]
    #[case::undetected(Some(RunMode::External(None)), true, false)]
    #[case::forked(Some(RunMode::DaemonizeForked), true, false)]
    #[case::agent(Some(RunMode::LaunchdAgent), true, false)]
    #[case::older_daemon(None, true, false)]
    fn external_only_while_something_else_runs_the_daemon(
        #[case] run_mode: Option<RunMode>,
        #[case] running: bool,
        #[case] external: bool,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::default();
        settings.daemon.pidfile_path =
            AbsPathBuf::new(dir.path().join("atuin-daemon.pid")).unwrap();
        let info = PidfileInfo {
            pid: std::process::id(),
            version: crate::VERSION.to_string(),
            socket_path: Some(dir.path().join("atuin.sock")),
            run_mode,
        };
        let guard = Pidfile::new(&settings.daemon.pidfile_path).acquire(&info).unwrap();
        // A daemon exits by dropping its guard.
        let _running = running.then_some(guard);

        assert_eq!(External::for_client(&settings.daemon).is_some(), external);
    }
}
