//! The backend used to manage the daemon service.
mod external;
mod launchd;
mod subproc;

use std::time::Duration;

use atuin_client::settings::Daemon as DaemonSettings;
use enum_dispatch::enum_dispatch;
pub use external::{External, ExternalRestartError};
pub use launchd::Launchd;
pub use subproc::{DaemonizeError, Subproc};

use crate::grpc::versioning::VersionMatchError;
use crate::service::PidfileWaitError;
use crate::service::address::DaemonAddress;
use crate::service::connection::{Connection, FindError};
use crate::service::run_mode::ExternalManager;

/// An error from a [`Backend`], each of which fails in its own ways.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct BackendError(Box<dyn std::error::Error + Send + Sync>);

impl BackendError {
    fn new(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self(Box::new(err))
    }

    /// The refusal to replace a daemon that another program runs, when that is this error.
    #[must_use]
    pub fn other_program(&self) -> Option<&ExternalRestartError> {
        self.0.downcast_ref().filter(|err| matches!(err, ExternalRestartError::OtherProgram { .. }))
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
    /// What answered by the timeout was still another build.
    #[error(transparent)]
    Mismatch(VersionMatchError),
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

    /// Kill the running service, whether or not it answers, and remove what it left behind.
    async fn evict(&self) -> Result<(), BackendError>;
}

#[enum_dispatch(Backend)]
pub enum AnyBackend {
    External(External),
    Launchd(Launchd),
    Subproc(Subproc),
}

impl AnyBackend {
    /// The backend that manages the service `daemon` describes: [`External`] while a daemon that
    /// something other than Atuin runs holds the pidfile.
    pub fn new(daemon: &DaemonSettings) -> Self {
        if let Some(external) = External::for_client(daemon) {
            return external.into();
        }
        if let Some(launchd) = Launchd::for_client(daemon) {
            return launchd.into();
        }
        Subproc::new(daemon).into()
    }

    /// Where the running service `daemon` describes listens, whichever backend started it.
    ///
    /// Unlike [`Self::new`], it asks neither the pidfile what runs the service nor whether the
    /// launchd agent is installed, which would cost every command a few lookups in the user
    /// database.
    pub fn locate(daemon: &DaemonSettings) -> Result<DaemonAddress, BackendError> {
        if daemon.systemd_socket {
            return Ok(External::new(daemon, ExternalManager::SystemdSocket).address());
        }

        // Whatever runs it, the daemon publishes its socket in the pidfile.
        Subproc::new(daemon).locate()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rstest::rstest;

    use super::*;
    use crate::service::{ReadyError, ReplaceError, ServiceFindError};

    fn other_program() -> BackendError {
        BackendError::new(ExternalRestartError::OtherProgram {
            program: PathBuf::from("/opt/homebrew/opt/atuin/bin/atuin"),
            ours: PathBuf::from("/opt/preview/atuin"),
            manager: ExternalManager::LaunchdJob {
                label: "sh.brew.atuin".to_owned(),
            },
        })
    }

    fn replace(err: ReplaceError) -> ReadyError {
        ReadyError::Find(ServiceFindError::Replace(err))
    }

    /// Only a daemon that another program runs is a refusal that the user can act on.
    #[rstest]
    #[case::stopping(replace(ReplaceError::Stop(StopError::Backend(other_program()))), true)]
    #[case::spawning(replace(ReplaceError::Spawn(SpawnError::Backend(other_program()))), true)]
    #[case::missing_manager(
        replace(ReplaceError::Stop(StopError::Backend(BackendError::new(
            ExternalRestartError::Missing { manager: "launchd" }
        )))),
        false
    )]
    #[case::not_running(ReadyError::NotRunning, false)]
    fn finds_the_refusal_of_another_program(#[case] err: ReadyError, #[case] refused: bool) {
        assert_eq!(err.other_program().is_some(), refused);
    }
}
