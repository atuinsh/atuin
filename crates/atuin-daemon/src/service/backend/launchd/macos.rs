use tokio::process::Command;

use crate::service::backend::ExternalRestartError;

/// launchd, macOS's service manager.
pub enum Launchd {}

impl Launchd {
    /// The label of the launchd job that started this process, if one did.
    pub(in crate::service) fn external_job_label() -> Option<String> {
        // launchd names the job only to the process it started, and anything that process forks
        // sees `0`.
        let label = atuin_common::env::var_os("XPC_SERVICE_NAME")?.into_string().ok()?;
        (label != "0").then_some(label)
    }

    /// Have launchd restart this user's job `label`: stop it, which launchd does with `SIGTERM`,
    /// and start it again.
    pub(in crate::service) async fn restart_job(label: &str) -> Result<(), ExternalRestartError> {
        let target = format!("gui/{}/{label}", atuin_common::os::unix::uid());
        let command = format!("launchctl kickstart -k {target}");
        match Command::new("launchctl").args(["kickstart", "-k", &target]).status().await {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(ExternalRestartError::Failed { command, status }),
            Err(source) => Err(ExternalRestartError::Spawn { command, source }),
        }
    }
}
