use super::Subproc;

/// Errors from [`Subproc::daemonize`], which has nothing to fail at on Windows.
#[derive(Debug, thiserror::Error)]
pub enum DaemonizeError {}

impl Subproc {
    /// Detach this process, as the service `launch` started: on Windows, it already outlives the
    /// client that spawned it.
    #[must_use]
    pub const fn daemonize() -> Result<(), DaemonizeError> {
        Ok(())
    }
}
