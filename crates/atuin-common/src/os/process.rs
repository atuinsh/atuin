use std::num::NonZeroU32;
use std::path::PathBuf;
use std::time::Duration;

#[cfg(windows)]
use windows_sys::Win32::System::Threading::{PROCESS_SYNCHRONIZE, PROCESS_TERMINATE};

use crate::futures::Backoff;
#[cfg(unix)]
use crate::os::unix;
#[cfg(windows)]
use crate::os::windows;
use crate::path::AbsPathBuf;

/// How often to check whether a process has exited while waiting out the graceful-shutdown period.
pub(crate) const EXIT_BACKOFF: Backoff = Backoff::Exponential {
    initial: Duration::from_millis(10),
    max: Duration::from_millis(200),
    factor: NonZeroU32::new(2).unwrap(),
};

/// The absolute path to the running executable.
///
/// On Linux, this works without `/proc`.
///
/// # Errors
///
/// If the OS can't say where the running executable is.
pub fn current_exe() -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe();
    // Without `/proc`, std can't read `/proc/self/exe`. `AT_EXECFN` is the path the executable was
    // run from.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let exe = exe.or_else(|_| {
        use std::os::unix::ffi::OsStrExt as _;

        let execfn = std::ffi::OsStr::from_bytes(rustix::param::linux_execfn().to_bytes());
        std::io::Result::Ok(PathBuf::from(execfn))
    });
    let exe = exe?;
    // `AT_EXECFN` is relative if the executable was run by a relative path. A caller can hand the
    // path to a process with another working directory, so the path is made absolute here.
    if exe.is_absolute() {
        return Ok(exe);
    }
    AbsPathBuf::from_current_dir(exe).map(PathBuf::from)
}

/// Gracefully (and then forcefully) terminate the process.
///
/// TODO(taylordotfish): On Unix, if the process doesn't exist, this function returns [`Ok`], but on
/// Windows, it returns an error. This should be unified.
pub async fn force_terminate(pid: u32, timeout: Duration) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        let Some(pid) = unix::process::pid_from_u32(pid) else {
            // Return `Ok` if the PID doesn't fit in a pid_t, the same behavior as if the process
            // doesn't exist (`unix::process::force_terminate` handles `ESRCH` by returning `Ok`).
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
