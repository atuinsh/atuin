//! Utilities for operating on processes.

use std::time::Duration;

use nix::errno::Errno;
use nix::sys::signal;
use nix::sys::signal::Signal as NixSignal;
use nix::unistd::Pid;

/// Gracefully terminate the process via `SIGTERM`.
///
/// If the process does not gracefully terminate, it is forcefully killed via `SIGKILL`.
pub async fn force_terminate(pid: Pid, timeout: Duration) -> Result<(), std::io::Error> {
    match signal::kill(pid, NixSignal::SIGTERM) {
        Ok(()) => {}
        Err(Errno::ESRCH) => return Ok(()),
        Err(errno) => return Err(errno.into()),
    }

    tokio::time::sleep(timeout).await;
    if !is_alive(pid) {
        return Ok(());
    }

    match signal::kill(pid, NixSignal::SIGKILL) {
        Ok(()) => {}
        Err(Errno::ESRCH) => return Ok(()),
        Err(errno) => return Err(errno.into()),
    };

    Ok(())
}

/// Check whether the given pid is alive.
#[must_use]
pub fn is_alive(pid: Pid) -> bool {
    !matches!(nix::sys::signal::kill(pid, None::<NixSignal>), Err(Errno::ESRCH))
}
