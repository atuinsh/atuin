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
///
/// `expected_start_time` is the start_time the caller expects the process at `pid` to have (e.g. the
/// value persisted alongside the pid). It guards against pid reuse: if the live process's identity
/// does not match, it is left untouched. Pass `None` when the expected identity is unknown (for
/// example an old-format pidfile), in which case liveness is best-effort.
pub async fn force_terminate(
    pid: u32,
    expected_start_time: Option<u64>,
    timeout: Duration,
) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        let Some(pid) = rustix::process::Pid::from_raw(pid.cast_signed()) else {
            return Ok(());
        };
        unix::process::force_terminate(pid, expected_start_time, timeout).await
    }

    #[cfg(windows)]
    {
        // On Windows the process is identified by an OpenProcess handle rather than a start_time,
        // so there is no separate identity comparison to perform here.
        let _ = expected_start_time;
        windows::process::Handle::open(pid, PROCESS_TERMINATE | PROCESS_SYNCHRONIZE)?
            .force_stop(timeout)
            .await
    }
}

/// Get the start time (in seconds) of the current process, if it can be determined.
///
/// This is persisted alongside the daemon pid so that a later force-cleanup can tell whether the
/// recorded pid still refers to the same process or has been reused by an unrelated one.
#[must_use]
pub fn current_process_start_time() -> Option<u64> {
    let pid = sysinfo::Pid::from_u32(std::process::id());
    let mut system = sysinfo::System::new();
    if system.refresh_process(pid) {
        system.process(pid).map(sysinfo::Process::start_time)
    } else {
        None
    }
}
