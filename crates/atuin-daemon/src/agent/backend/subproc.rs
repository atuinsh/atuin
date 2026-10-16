use std::process::{Command, Stdio};

use atuin_client::settings::Settings;

use crate::agent::backend::{
    AGENT_ARGS, AgentHandle, AgentTracker, Backend, FindError, SpawnError, StopError,
};

/// Represents a backend that manages atuin-daemon as a background task by forking a user's regular
/// `atuin` process.
///
/// The agent is `<this executable> daemon start --daemonize`, so only the `atuin` binary can spawn
/// one.
pub struct Subproc {
    tracker: AgentTracker,
}

impl Subproc {
    pub fn new(settings: &Settings) -> Self {
        Self {
            tracker: AgentTracker::new(settings),
        }
    }

    /// Start the agent process in the background, without waiting for it to answer.
    pub(super) fn launch() -> Result<(), SpawnError> {
        let exe = std::env::current_exe().map_err(SpawnError::Exe)?;

        let mut agent = Command::new(exe);
        agent.args(AGENT_ARGS).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());

        // The agent detaches itself, since it must fork before it starts its async runtime.
        #[cfg(unix)]
        agent.arg("--daemonize");

        agent.spawn().map_err(SpawnError::Launch)?;
        Ok(())
    }
}

impl Backend for Subproc {
    async fn spawn(&self) -> Result<AgentHandle, SpawnError> {
        // An agent that was just asked to shut down holds the pidfile until it exits, and one
        // launched before then would find the pidfile taken and exit itself.
        self.tracker.exited().await?;
        Self::launch()?;
        self.tracker.answering().await
    }

    async fn find(&self) -> Result<Option<AgentHandle>, FindError> {
        self.tracker.find().await
    }

    async fn force_stop(&self, handle: AgentHandle) -> Result<(), StopError> {
        handle.request_shutdown().await;
        self.tracker.exited().await?;
        Ok(())
    }
}
