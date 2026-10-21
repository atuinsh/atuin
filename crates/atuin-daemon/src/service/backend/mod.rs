//! The backend used to manage the daemon service.
#[cfg(unix)]
mod external;
#[cfg(target_os = "macos")]
mod launchd;
mod subproc;

use std::time::Duration;

use atuin_client::settings::Daemon as DaemonSettings;
use enum_dispatch::enum_dispatch;
#[cfg(unix)]
pub use external::External;
#[cfg(target_os = "macos")]
pub use launchd::Launchd;
#[cfg(unix)]
pub use subproc::DaemonizeError;
pub use subproc::Subproc;

use crate::service::PidfileWaitError;
use crate::service::address::{DaemonAddress, DaemonListener};
use crate::service::connection::{Connection, FindError};
#[cfg(unix)]
use crate::service::pidfile::{Pidfile, PidfileInfo};
#[cfg(unix)]
use crate::service::run_mode::{ExternalManager, RunMode};
#[cfg(unix)]
use crate::service::socket_file::SocketFile;

/// An error from a [`Backend`], each of which fails in its own ways.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct BackendError(Box<dyn std::error::Error + Send + Sync>);

impl BackendError {
    fn new(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self(Box::new(err))
    }
}

/// Errors from spawning a service.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error(transparent)]
    Pidfile(#[from] PidfileWaitError),
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error("timed out waiting for daemon startup after {}ms", .0.as_millis())]
    NotReady(Duration),
    #[error(transparent)]
    Find(#[from] FindError),
}

/// Errors from stopping a service.
#[derive(Debug, thiserror::Error)]
pub enum StopError {
    #[error(transparent)]
    Pidfile(#[from] PidfileWaitError),
    #[error(transparent)]
    Backend(#[from] BackendError),
}

/// The socket a backend hands the service it runs.
pub struct Listening {
    pub listener: DaemonListener,
    /// The socket file the service listens on, kept fresh while it runs and, if it bound the file
    /// itself, removed when it exits.
    #[cfg(unix)]
    pub socket: Option<SocketFile>,
}

/// How a service is started and stopped, and where it listens.
///
/// Waiting for one to exit or answer is the same whatever runs it, and is left to the caller.
#[enum_dispatch]
#[allow(async_fn_in_trait, reason = "only used within our code and we don't need it to be Send")]
pub trait Backend {
    /// Where the services this backend launches listen.
    fn address(&self) -> DaemonAddress;

    /// Take up the socket to serve on, from within a service this backend runs.
    fn listen(&self) -> Result<Listening, BackendError>;

    /// Start a service, without waiting for it to answer.
    async fn launch(&self) -> Result<(), BackendError>;

    /// Ask `service` to stop, without waiting for it to exit.
    async fn stop(&self, service: &Connection) -> Result<(), BackendError>;

    /// Kill the running service, whether or not it answers, and remove what it left behind.
    async fn evict(&self) -> Result<(), BackendError>;
}

#[enum_dispatch(Backend)]
pub enum AnyBackend {
    #[cfg(unix)]
    External(External),
    #[cfg(target_os = "macos")]
    Launchd(Launchd),
    Subproc(Subproc),
}

impl AnyBackend {
    /// The backend that manages the service `daemon` describes: [`External`] while a daemon that
    /// something other than Atuin runs holds the pidfile.
    pub fn new(daemon: &DaemonSettings) -> Self {
        #[cfg(unix)]
        if daemon.systemd_socket {
            return External::new(daemon, ExternalManager::SystemdSocket).into();
        }

        // It would only start again once stopped, and an agent of Atuin's would fight it for the
        // pidfile at every login.
        #[cfg(unix)]
        {
            let pidfile = Pidfile::new(&daemon.pidfile_path);
            if pidfile.is_live()
                && let Some(PidfileInfo {
                    run_mode: Some(RunMode::External(Some(manager))),
                    ..
                }) = pidfile.info()
            {
                return External::new(daemon, manager).into();
            }
        }

        #[cfg(target_os = "macos")]
        if Launchd::serves_this_profile()
            && Launchd::is_available()
            // Without `autostart`, something else manages the daemon, such as home-manager. One
            // installed while it was on is still ours to stop, though.
            && (daemon.autostart || Launchd::is_installed())
        {
            return Launchd::new(daemon).into();
        }

        Subproc::new(daemon).into()
    }

    /// Where the running service `daemon` describes listens, whichever backend started it.
    ///
    /// Unlike [`Self::new`], it doesn't ask whether the launchd agent is installed, which costs
    /// every command a few lookups in the user database.
    pub fn locate(daemon: &DaemonSettings) -> Result<DaemonAddress, BackendError> {
        #[cfg(unix)]
        if daemon.systemd_socket {
            return Ok(External::new(daemon, ExternalManager::SystemdSocket).address());
        }

        // The launchd agent publishes its socket in the pidfile, as a forked daemon does.
        Subproc::new(daemon).locate()
    }
}

#[cfg(all(test, unix))]
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

        assert_eq!(matches!(AnyBackend::new(&settings.daemon), AnyBackend::External(_)), external);
    }
}
