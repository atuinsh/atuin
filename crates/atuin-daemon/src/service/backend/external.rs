use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use atuin_client::settings::Daemon as DaemonSettings;
use atuin_common::path::AbsPathBuf;

use crate::service::address::DaemonAddress;
use crate::service::backend::{Backend, BackendError, Launchd};
use crate::service::connection::Connection;
use crate::service::pidfile::{Pidfile, PidfileInfo};
use crate::service::run_mode::{ExternalManager, RunMode};
use crate::service::systemd::Systemd;

/// Represents a backend for a service that something other than Atuin runs, through a unit or job
/// the user set up.
pub struct External {
    address: DaemonAddress,
    manager: ExternalManager,
    /// The path the daemon was run as, which `manager` runs again to restart it.
    program: Option<PathBuf>,
}

/// Errors from [`External::restart`].
#[derive(Debug, thiserror::Error)]
pub enum ExternalRestartError {
    #[error(
        "what runs the daemon starts {}, not this atuin at {}, so restarting it wouldn't bring up \
         this build",
        .program.display(),
        .ours.display()
    )]
    OtherProgram {
        program: PathBuf,
        ours: PathBuf,
    },
    #[allow(dead_code, reason = "only launchd and systemd have a command to run")]
    #[error("could not run `{command}` to restart the daemon: {source}")]
    Spawn {
        command: String,
        source: io::Error,
    },
    #[allow(dead_code, reason = "only launchd and systemd have a command to run")]
    #[error("`{command}` could not restart the daemon: {status}")]
    Failed {
        command: String,
        status: ExitStatus,
    },
    #[error("{manager} isn't on this system to restart the daemon")]
    Missing {
        manager: &'static str,
    },
}

impl External {
    #[must_use]
    pub fn new(daemon: &DaemonSettings, manager: ExternalManager) -> Self {
        Self {
            address: DaemonAddress::existing(daemon),
            manager,
            program: None,
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
            program,
            ..
        } = pidfile.info()?
        else {
            return None;
        };
        Some(Self {
            program,
            ..Self::new(daemon, manager)
        })
    }

    /// Have what runs the service restart it, without waiting for the new one to answer.
    ///
    /// Only if that brings up this client's build: every client of this build would otherwise
    /// restart it again.
    pub(in crate::service) async fn restart(&self) -> Result<(), BackendError> {
        self.ensure_restarts_as(&atuin_common::os::process::current_exe()).await?;

        // A client that gives up waiting leaves the manager's command running, and the restart
        // finishes anyway.
        let restarted = match &self.manager {
            ExternalManager::LaunchdJob { label } => Launchd::restart_job(label).await,
            ExternalManager::SystemdUnit { name } => Systemd::restart_unit(name).await,
            // It doesn't say which unit the socket starts.
            ExternalManager::SystemdSocket => return Err(BackendError::new(self.manager.clone())),
        };
        restarted.map_err(BackendError::new)
    }

    /// Make sure that restarting the service runs `exe`, through whatever links lead there, such as
    /// `brew`'s `opt/atuin/bin/atuin` to the build it last installed.
    async fn ensure_restarts_as(&self, exe: &Path) -> Result<(), BackendError> {
        let Some(program) = &self.program else {
            return Err(BackendError::new(self.manager.clone()));
        };
        match (
            AbsPathBuf::canonicalize_async(program).await,
            AbsPathBuf::canonicalize_async(exe).await,
        ) {
            (Ok(theirs), Ok(ours)) if theirs == ours => Ok(()),
            _ => Err(BackendError::new(ExternalRestartError::OtherProgram {
                program: program.clone(),
                ours: exe.to_owned(),
            })),
        }
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
            program: None,
        };
        let guard = Pidfile::new(&settings.daemon.pidfile_path).acquire(&info).unwrap();
        // A daemon exits by dropping its guard.
        let _running = running.then_some(guard);

        // Windows locks a held pidfile against reading, so a client there can't tell what runs
        // the daemon. Nothing there runs it in Atuin's place, though.
        assert_eq!(External::for_client(&settings.daemon).is_some(), external && cfg!(unix));
    }

    /// A restart brings up this client's build only if what runs the daemon starts this client's
    /// program again, as through a link to it like `brew`'s `opt/atuin/bin/atuin`.
    #[cfg(unix)]
    #[rstest]
    #[case::this_program(Some("new"), true)]
    #[case::a_link_to_it(Some("link"), true)]
    #[case::another_program(Some("old"), false)]
    #[case::unrecorded(None, false)]
    #[tokio::test]
    async fn a_restart_starts_only_this_program(
        #[case] program: Option<&str>,
        #[case] restarts: bool,
    ) {
        let dir = tempfile::tempdir().unwrap();
        for build in ["new", "old"] {
            std::fs::write(dir.path().join(build), "").unwrap();
        }
        std::os::unix::fs::symlink(dir.path().join("new"), dir.path().join("link")).unwrap();
        let manager = ExternalManager::LaunchdJob {
            label: "homebrew.mxcl.atuin".to_owned(),
        };
        let external = External {
            program: program.map(|program| dir.path().join(program)),
            ..External::new(&Settings::default().daemon, manager)
        };

        let restart = external.ensure_restarts_as(&dir.path().join("new")).await;
        assert_eq!(restart.is_ok(), restarts);
    }
}
