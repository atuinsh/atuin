use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, GetLastError,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

use crate::os::process::{KillError, Signal as CommonSignal};

/// RAII-safe operations on windows [`HANDLE`] types.
struct Handle {
    inner: HANDLE,
}

impl Handle {
    /// Open the given pid with the specified desired access mask.
    ///
    /// See [`OpenProcess`].
    pub fn open(pid: i32, access: DWORD) -> Result<Self, std::io::Error> {
        Ok(Self {
            inner: fallible_do(|| unsafe { OpenProcess(access, 0, pid) })?,
        })
    }

    /// Kill the process via the [`TerminateProcess`] call. Requires [`PROCESS_TERMINATE`] access.
    pub fn terminate(&self) -> Result<(), std::io::Error> {
        fallible_do(|| unsafe { TerminateProcess(self.inner, 1) })
    }

    /// Send a CTRL-Break code to a particular process.
    ///
    /// This is the best signal we have in-place of `SIGTERM` on windows, so this is what we get.
    ///
    /// Ctrl-Break is a signal that processes cannot ignore. The target process must have done two
    /// things to receive this signal:
    ///
    ///   - They must have a registered [`SetConsoleCtrlHandler`].
    ///   - They must have spawned with [`CREATE_NEW_PROCESS_GROUP`].
    pub fn send_ctrl_break(&self) -> Result<(), std::io::Error> {
        fallible_do(|| unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT) })
    }

    /// Check whether the process is alive right now. Requires [`PROCESS_SYNCHRONIZE`] access.
    pub fn is_alive(&self) -> bool {
        if unsafe { WaitForSingleObject(self.inner, 0) } == WAIT_TIMEOUT {
            false
        } else {
            true
        }
    }

    /// Try to terminate the process.
    ///
    /// This calls [`send_ctrl_break`] and, after the specified duration, checks whether the process
    /// has gracefully exited. If it hasn't it gets killed via [`Self::terminate`].
    pub async fn force_stop(&self, timeout: Duration) -> Result<(), std::io::Error> {
        let ctrlb = self.send_ctrl_break();

        let err = match ctrlb {
            Ok(()) => return Ok(()),
            Err(err) => err,
        };

        tokio::time::sleep(timeout).await;
        if !self.is_alive() {
            return Ok(());
        }

        self.terminate()?;

        Ok(())
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.inner);
        }
    }
}

/// Given a process ID, kill it. Equivalent to `SIGKILL` -- very not graceful.
pub fn kill(pid: i32) -> Result<(), std::io::Error> {
    Handle::open(pid, PROCESS_TERMINATE)?.terminate()
}

/// See [`Handle::force_stop`].
pub async fn force_stop(pid: i32, timeout: Duration) -> Result<(), std::io::Error> {
    Handle::open(pid, PROCESS_SYNCHRONIZE | PROCESS_TERMINATE)?.force_stop().await?
}
