//! Utilities for operating on processes.

use std::ops::ControlFlow;
use std::time::Duration;

use rustix::io::Errno;
use rustix::process::{self, Pid, Signal};

/// Gracefully terminate the process via `SIGTERM`.
///
/// If the process does not gracefully terminate, it is forcefully killed via `SIGKILL`.
pub async fn force_terminate(pid: Pid, timeout: Duration) -> Result<(), std::io::Error> {
    match process::kill_process(pid, Signal::TERM) {
        Ok(()) => {}
        Err(Errno::SRCH) => return Ok(()),
        Err(errno) => return Err(errno.into()),
    }

    let exited = crate::os::process::EXIT_BACKOFF
        .retry_blocking(
            || {
                if is_alive(pid) {
                    ControlFlow::Continue(())
                } else {
                    ControlFlow::Break(())
                }
            },
            timeout,
        )
        .await;

    if exited.is_ok() {
        return Ok(());
    }

    match process::kill_process(pid, Signal::KILL) {
        Ok(()) => {}
        Err(Errno::SRCH) => return Ok(()),
        Err(errno) => return Err(errno.into()),
    }

    Ok(())
}

/// Check whether the given pid is alive.
#[must_use]
pub fn is_alive(pid: Pid) -> bool {
    !matches!(process::test_kill_process(pid), Err(Errno::SRCH))
}
