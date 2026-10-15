//! The backend used to manage the daemon agent.
#[cfg(target_os = "macos")]
mod launchd;
mod subproc;

#[cfg(target_os = "macos")]
use std::io;
use std::ops::ControlFlow;
use std::time::Duration;

use atuin_client::settings::{Daemon as DaemonSettings, Settings};
use atuin_common::futures::Backoff;
use enum_dispatch::enum_dispatch;
#[cfg(target_os = "macos")]
pub use launchd::Launchd;
pub use subproc::Subproc;
use tonic::Code;

use crate::HistoryClient;
use crate::agent::{LifecycleTimeout, Pidfile, PidfileWaitError};
use crate::client::{AddressError, DaemonAddress};
use crate::grpc::versioning::OtherBuild;

/// The arguments that run the `atuin` executable as the agent.
const AGENT_ARGS: [&str; 2] = ["daemon", "start"];

/// Represents a handle to an actively running agent.
#[derive(Debug)]
pub struct AgentHandle {
    address: DaemonAddress,
    /// How the agent's build compared with this client's when it was found.
    build: Result<(), OtherBuild>,
}

impl AgentHandle {
    pub const fn address(&self) -> &DaemonAddress {
        &self.address
    }

    /// Check that the agent is this client's build.
    pub fn ensure_our_build(&self) -> Result<(), OtherBuild> {
        self.build.clone()
    }

    /// Ask the agent to shut down, without waiting for it to.
    ///
    /// Best effort: an agent that is already gone, or doesn't take the request, is the business
    /// of whoever then waits for it to exit.
    async fn request_shutdown(&self) {
        let Ok(mut client) = HistoryClient::new(self.address.clone()).await else {
            return;
        };
        let _ = client.shutdown().await;
    }
}

/// Follows the agent through the socket it listens on and the pidfile it holds, whichever backend
/// runs it.
struct AgentTracker {
    daemon: DaemonSettings,
    pidfile: Pidfile,
    timeout: LifecycleTimeout,
}

impl AgentTracker {
    /// How often to check whether a just-started agent answers yet.
    const READY_POLL: Duration = Duration::from_millis(40);

    fn new(settings: &Settings) -> Self {
        Self {
            daemon: settings.daemon.clone(),
            pidfile: Pidfile::new(&settings.daemon),
            timeout: LifecycleTimeout::from(settings),
        }
    }

    /// The agent answering at its address, if any, whatever its build.
    async fn find(&self) -> Result<Option<AgentHandle>, FindError> {
        let address = DaemonAddress::of(&self.daemon)?;
        // Failing to connect means nothing listens there.
        let Ok(mut client) = HistoryClient::new(address.clone()).await else {
            return Ok(None);
        };

        match client.check_daemon_version().await {
            Ok(build) => Ok(Some(AgentHandle {
                address,
                build: build.map_err(OtherBuild::from),
            })),
            Err(status) if status.code() == Code::Unavailable => Ok(None),
            Err(status) if status.code() == Code::Unimplemented => Err(FindError::Legacy),
            Err(status) => Err(FindError::Status(Box::new(status))),
        }
    }

    /// Wait for a just-started agent to answer.
    async fn answering(&self) -> Result<AgentHandle, SpawnError> {
        let timeout = self.timeout.get();
        Backoff::Constant(Self::READY_POLL)
            .retry(
                || async move {
                    match self.find().await {
                        Ok(Some(handle)) => ControlFlow::Break(Ok(handle)),
                        Ok(None) => ControlFlow::Continue(()),
                        Err(err) => ControlFlow::Break(Err(err.into())),
                    }
                },
                timeout,
            )
            .await
            .unwrap_or_else(|()| Err(SpawnError::NotReady(timeout)))
    }

    /// Wait for the agent to exit, which it has once it releases the pidfile.
    async fn exited(&self) -> Result<(), PidfileWaitError> {
        self.pidfile.released(self.timeout).await
    }
}

/// Errors from spawning an agent.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error(transparent)]
    Pidfile(#[from] PidfileWaitError),
    #[error("could not locate the atuin executable: {0}")]
    Exe(std::io::Error),
    #[error("failed to launch the daemon: {0}")]
    Launch(std::io::Error),
    #[error("timed out waiting for daemon startup after {}ms", .0.as_millis())]
    NotReady(Duration),
    #[error(transparent)]
    Find(#[from] FindError),
}

/// Errors from finding an agent.
#[derive(Debug, thiserror::Error)]
pub enum FindError {
    #[error(transparent)]
    Address(#[from] AddressError),
    #[error("legacy daemon detected; restart daemon manually")]
    Legacy,
    #[error("could not ask the daemon for its status: {0}")]
    Status(Box<tonic::Status>),
}

/// Errors from stopping an agent.
#[derive(Debug, thiserror::Error)]
pub enum StopError {
    #[error(transparent)]
    Pidfile(#[from] PidfileWaitError),
    #[cfg(target_os = "macos")]
    #[error("failed to remove the daemon's launchd agent: {0}")]
    Launchd(io::Error),
}

#[enum_dispatch]
#[allow(async_fn_in_trait, reason = "only used within our code and we don't need it to be Send")]
pub trait Backend {
    /// Spawn an agent.
    async fn spawn(&self) -> Result<AgentHandle, SpawnError>;

    /// Try to find an active agent.
    ///
    /// This needs to guarantee that the returned agent is running and reachable, but does not need
    /// to guarantee that the agent is up-to-date.
    async fn find(&self) -> Result<Option<AgentHandle>, FindError>;

    /// Try to forcefully stop the given agent.
    async fn force_stop(&self, handle: AgentHandle) -> Result<(), StopError>;
}

#[enum_dispatch(Backend)]
pub enum AnyBackend {
    #[cfg(target_os = "macos")]
    Launchd(Launchd),
    Subproc(Subproc),
}

impl AnyBackend {
    /// The backend that manages the agent `settings` describe.
    pub fn new(settings: &Settings) -> Self {
        // Without `autostart`, something else manages the daemon (`brew services`, home-manager),
        // and an agent of our own beside theirs would fight theirs for the pidfile at every login.
        #[cfg(target_os = "macos")]
        if settings.daemon.autostart && Launchd::is_available() {
            return Launchd::new(settings).into();
        }

        Subproc::new(settings).into()
    }
}
