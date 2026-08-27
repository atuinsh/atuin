use std::num::NonZeroU32;
use std::time::Duration;

#[cfg(windows)]
use windows_sys::Win32::System::Threading::{PROCESS_SYNCHRONIZE, PROCESS_TERMINATE};

use crate::futures::Backoff;
#[cfg(unix)]
use crate::os::unix;
#[cfg(windows)]
use crate::os::windows;

/// How often to check whether a process has exited while waiting out the graceful-shutdown period.
pub(crate) const EXIT_BACKOFF: Backoff = Backoff::Exponential {
    initial: Duration::from_millis(10),
    max: Duration::from_millis(200),
    factor: NonZeroU32::new(2).unwrap(),
};

/// Gracefully (and then forcefully) terminate the process.
pub async fn force_terminate(pid: u32, timeout: Duration) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        let Some(pid) = rustix::process::Pid::from_raw(pid.cast_signed()) else {
            return Ok(());
        };
        unix::process::force_terminate(pid, timeout).await
    }

    #[cfg(windows)]
    {
        windows::process::Handle::open(pid, PROCESS_TERMINATE | PROCESS_SYNCHRONIZE)?
            .force_stop(timeout)
            .await
    }
}
