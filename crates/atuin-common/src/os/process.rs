use std::time::Duration;

#[cfg(windows)]
use windows_sys::Win32::System::Threading::{PROCESS_SYNCHRONIZE, PROCESS_TERMINATE};

#[cfg(unix)]
use crate::os::unix;
#[cfg(windows)]
use crate::os::windows;

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
