use std::process::{Command, Stdio};

use atuin_client::settings::Settings;

use crate::agent::backend::{AGENT_ARGS, AgentHandle, Backend, SpawnError, StopError};
use crate::client::DaemonAddress;

/// Represents a backend that manages atuin-daemon as a background task by forking a user's regular
/// `atuin` process.
///
/// The agent is `<this executable> daemon start --daemonize`, so only the `atuin` binary can spawn
/// one. It binds its socket itself.
pub struct Subproc {
    address: DaemonAddress,
}

impl Subproc {
    pub fn new(settings: &Settings) -> Self {
        Self {
            address: DaemonAddress::configured(&settings.daemon),
        }
    }
}

impl Backend for Subproc {
    fn address(&self) -> DaemonAddress {
        self.address.clone()
    }

    async fn launch(&self) -> Result<(), SpawnError> {
        let exe = std::env::current_exe().map_err(SpawnError::Exe)?;

        let mut agent = Command::new(exe);
        agent.args(AGENT_ARGS).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());

        // The agent detaches itself, since it must fork before it starts its async runtime.
        #[cfg(unix)]
        agent.arg("--daemonize");

        agent.spawn().map_err(SpawnError::Launch)?;
        Ok(())
    }

    async fn stop(&self, agent: &AgentHandle) -> Result<(), StopError> {
        agent.request_shutdown().await;
        Ok(())
    }
}
