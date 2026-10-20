//! The backend used to manage the daemon service.
#[cfg(unix)]
mod external;
mod subproc;

use std::time::Duration;

use atuin_client::settings::Daemon as DaemonSettings;
use enum_dispatch::enum_dispatch;
#[cfg(unix)]
pub use external::External;
#[cfg(unix)]
pub use subproc::DaemonizeError;
pub use subproc::Subproc;

use crate::service::PidfileWaitError;
use crate::service::address::DaemonAddress;
use crate::service::connection::{Connection, FindError};
#[cfg(unix)]
use crate::service::pidfile::Pidfile;

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

/// How a service is started and stopped, and where it listens.
///
/// Waiting for one to exit or answer is the same whatever runs it, and is left to the caller.
#[enum_dispatch]
#[allow(async_fn_in_trait, reason = "only used within our code and we don't need it to be Send")]
pub trait Backend {
    /// Where the services this backend launches listen.
    fn address(&self) -> DaemonAddress;

    /// Start a service, without waiting for it to answer.
    async fn launch(&self) -> Result<(), BackendError>;

    /// Ask `service` to stop, without waiting for it to exit.
    async fn stop(&self, service: &Connection) -> Result<(), BackendError>;
}

#[enum_dispatch(Backend)]
pub enum AnyBackend {
    #[cfg(unix)]
    External(External),
    Subproc(Subproc),
}

impl AnyBackend {
    /// The backend that manages the service `daemon` describes: [`External`] while one that
    /// systemd started holds the pidfile, [`Subproc`] otherwise.
    ///
    /// Call it once the service has answered, if it does: systemd starts one when a client
    /// connects, and only then does it record itself in the pidfile.
    pub fn new(daemon: &DaemonSettings) -> Self {
        #[cfg(unix)]
        {
            let pidfile = Pidfile::new(&daemon.pidfile_path);
            if pidfile.is_live()
                && let Some(info) = pidfile.info()
                && info.started_by_systemd()
            {
                let path =
                    info.socket_path.unwrap_or_else(|| daemon.existing_socket_path().into_owned());
                return External::new(path.into()).into();
            }
        }

        Subproc::new(daemon).into()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use atuin_client::settings::Settings;
    use atuin_common::path::AbsPathBuf;
    use rstest::rstest;

    use super::*;
    use crate::service::pidfile::PidfileInfo;

    /// A client leaves the daemon to systemd only while one that systemd started runs, including
    /// one from before the pidfile said so, which recorded no socket path: neither a daemon that
    /// bound its own socket nor the record of one that has exited is a reason to.
    #[rstest]
    #[case::started_by_systemd(true, Some("atuin.sock"), true, true)]
    #[case::exited(true, Some("atuin.sock"), false, false)]
    #[case::bound_its_own_socket(false, Some("atuin.sock"), true, false)]
    #[case::started_by_systemd_before_it_said_so(false, None, true, true)]
    fn systemd_manages_only_a_running_daemon_it_started(
        #[case] systemd: bool,
        #[case] socket: Option<&str>,
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
            socket_path: socket.map(|name| dir.path().join(name)),
            systemd,
        };
        let guard = Pidfile::new(&settings.daemon.pidfile_path).acquire(&info).unwrap();
        // A daemon exits by dropping its guard.
        let _running = running.then_some(guard);

        assert_eq!(matches!(AnyBackend::new(&settings.daemon), AnyBackend::External(_)), external);
    }
}
