#![allow(unsafe_code, reason = "win32 API calls all require unsafe.")]

use std::ops::ControlFlow;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::time::Duration;

use windows_sys::Win32::Foundation::{HANDLE, WAIT_TIMEOUT};
use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent};
use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, WaitForSingleObject};

use super::{fallible_do, get_last_error};

/// RAII-safe operations on windows [`HANDLE`] types.
pub struct Handle {
    // Unlike a raw `HANDLE`, it can cross an `.await` in a future that has to be `Send`.
    inner: OwnedHandle,
    pid: u32,
}

impl Handle {
    /// Open the given pid with the specified desired access mask.
    ///
    /// See [`OpenProcess`].
    pub fn open(pid: u32, access: u32) -> Result<Self, std::io::Error> {
        let inner = unsafe { OpenProcess(access, 0, pid) };
        if inner.is_null() {
            return Err(get_last_error());
        }
        // SAFETY: `OpenProcess` returned a valid handle, which nothing else owns.
        let inner = unsafe { OwnedHandle::from_raw_handle(inner) };
        Ok(Self { inner, pid })
    }

    /// Kill the process via the [`TerminateProcess`] call. Requires [`PROCESS_TERMINATE`] access.
    pub fn terminate(&self) -> Result<(), std::io::Error> {
        fallible_do(|| unsafe { TerminateProcess(self.raw(), 1) })
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
        fallible_do(|| unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, self.pid) })
    }

    /// Check whether the process is alive right now. Requires [`PROCESS_SYNCHRONIZE`] access.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        let status = unsafe { WaitForSingleObject(self.raw(), 0) };
        status == WAIT_TIMEOUT
    }

    /// Try to terminate the process.
    ///
    /// This calls [`send_ctrl_break`] and, after the specified duration, checks whether the process
    /// has gracefully exited. If it hasn't it gets killed via [`Self::terminate`].
    pub async fn force_stop(&self, timeout: Duration) -> Result<(), std::io::Error> {
        let _ = self.send_ctrl_break();

        let exited = crate::os::process::EXIT_BACKOFF
            .retry_sync(
                || {
                    if self.is_alive() {
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

        self.terminate()
    }

    fn raw(&self) -> HANDLE {
        self.inner.as_raw_handle()
    }
}
