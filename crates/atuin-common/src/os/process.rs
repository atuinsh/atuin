use std::time::Duration;

#[cfg(unix)]
use crate::os::unix;
#[cfg(unix)]
use nix;

#[cfg(windows)]
use crate::os::windows;

/// Gracefully (and then forcefully) terminate the process.
pub async fn force_terminate(pid: i32, timeout: Duration) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    unix::process::force_terminate(nix::unistd::Pid::from_raw(pid), timeout).await

    #[cfg(windows)]
    windows::process::force_stop(pid, timeout).await
}
