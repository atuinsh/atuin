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
#[cfg(any(target_os = "linux", target_os = "android"))]
use crate::path::AbsPathBuf;

/// How often to check whether a process has exited while waiting out the graceful-shutdown period.
pub(crate) const EXIT_BACKOFF: Backoff = Backoff::Exponential {
    initial: Duration::from_millis(10),
    max: Duration::from_millis(200),
    factor: NonZeroU32::new(2).unwrap(),
};

/// The path to the running executable.
///
/// # Panics
///
/// If the OS can't say. On Linux, that takes both `/proc` being unmounted and the working directory
/// being gone, since the kernel still passes the path it ran the executable from.
#[must_use]
pub fn current_exe() -> PathBuf {
    let exe = std::env::current_exe();
    // Without `/proc`, std can't read `/proc/self/exe`. `AT_EXECFN` is the path the executable was
    // run from, relative if that was.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let exe = exe.or_else(|_| {
        use std::os::unix::ffi::OsStrExt as _;

        let execfn = std::ffi::OsStr::from_bytes(rustix::param::linux_execfn().to_bytes());
        AbsPathBuf::from_current_dir(execfn).map(PathBuf::from)
    });
    exe.expect("the OS knows the running executable's path")
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
