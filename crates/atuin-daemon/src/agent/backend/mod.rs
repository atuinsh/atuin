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
use crate::agent::{LifecycleTimeout, PidfileWaitError};
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
    /// How often to check whether a just-launched agent answers yet.
    const READY_POLL: Duration = Duration::from_millis(40);

    /// The agent `daemon` configures, if one is running, whatever its build and whatever started
    /// it.
    pub(super) async fn find(daemon: &DaemonSettings) -> Result<Option<Self>, FindError> {
        Self::at(DaemonAddress::of(daemon)?).await
    }

    /// The agent answering at `address`, if any, whatever its build.
    async fn at(address: DaemonAddress) -> Result<Option<Self>, FindError> {
        // Failing to connect means nothing listens there.
        let Ok(mut client) = HistoryClient::new(address.clone()).await else {
            return Ok(None);
        };

        match client.check_daemon_version().await {
            Ok(build) => Ok(Some(Self {
                address,
                build: build.map_err(OtherBuild::from),
            })),
            Err(status) if status.code() == Code::Unavailable => Ok(None),
            Err(status) if status.code() == Code::Unimplemented => Err(FindError::Legacy),
            Err(status) => Err(FindError::Status(Box::new(status))),
        }
    }

    /// Wait for a just-launched agent to answer at `address`.
    pub(super) async fn answering(
        address: &DaemonAddress,
        timeout: LifecycleTimeout,
    ) -> Result<Self, SpawnError> {
        let timeout = timeout.get();
        let poll = Backoff::Constant(Self::READY_POLL).retry(
            || async move {
                match Self::at(address.clone()).await {
                    Ok(Some(handle)) => ControlFlow::Break(Ok(handle)),
                    Ok(None) => ControlFlow::Continue(()),
                    Err(err) => ControlFlow::Break(Err(err.into())),
                }
            },
            timeout,
        );

        // `retry` doesn't bound its first poll, which waits on the agent to boot when launchd holds
        // its socket: the connection is accepted at once.
        match tokio::time::timeout(timeout, poll).await {
            Ok(Ok(answered)) => answered,
            Ok(Err(())) | Err(_) => Err(SpawnError::NotReady(timeout)),
        }
    }

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

/// How an agent is started and stopped, and where it listens.
///
/// Waiting for one to exit or answer is the same whatever runs it, and is left to the caller.
#[enum_dispatch]
#[allow(async_fn_in_trait, reason = "only used within our code and we don't need it to be Send")]
pub trait Backend {
    /// Where the agents this backend launches listen.
    fn address(&self) -> DaemonAddress;

    /// Start an agent, without waiting for it to answer.
    async fn launch(&self) -> Result<(), SpawnError>;

    /// Ask `agent` to stop, without waiting for it to exit.
    async fn stop(&self, agent: &AgentHandle) -> Result<(), StopError>;
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
