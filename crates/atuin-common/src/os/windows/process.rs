#![allow(unsafe_code, reason = "win32 API calls all require unsafe.")]

use std::ops::ControlFlow;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr;
use std::time::Duration;

use windows_sys::Win32::Foundation::WAIT_TIMEOUT;
use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DETACHED_PROCESS, OpenProcess, PROCESS_INFORMATION, STARTUPINFOW,
    TerminateProcess, WaitForSingleObject,
};

use super::{fallible_do, get_last_error};

/// RAII-safe operations on a windows process handle.
pub struct Handle {
    inner: OwnedHandle,
    pid: u32,
}

impl Handle {
    /// Open the given pid with the specified desired access mask.
    ///
    /// See [`OpenProcess`].
    pub fn open(pid: u32, access: u32) -> Result<Self, std::io::Error> {
        let raw = unsafe { OpenProcess(access, 0, pid) };
        if raw.is_null() {
            return Err(get_last_error());
        }
        // SAFETY: OpenProcess returned a valid handle, which the caller owns.
        let inner = unsafe { OwnedHandle::from_raw_handle(raw) };
        Ok(Self { inner, pid })
    }

    /// Spawn `program` without a console, inheriting none of this process's handles.
    ///
    /// [`std::process::Command`] always passes every inheritable handle on, so a long-lived child
    /// holds open whatever pipe a shell is reading our output from. `raw_args` is appended to the
    /// command line verbatim, like [`std::os::windows::process::CommandExt::raw_arg`].
    pub fn spawn_detached(program: &Path, raw_args: &str) -> Result<Self, std::io::Error> {
        let application: Vec<u16> = program.as_os_str().encode_wide().chain([0]).collect();
        // A Windows path cannot contain `"`, so quoting argv[0] needs no escaping.
        let mut command_line: Vec<u16> = "\""
            .encode_utf16()
            .chain(program.as_os_str().encode_wide())
            .chain(format!("\" {raw_args}").encode_utf16())
            .chain([0])
            .collect();
        let startup_info = STARTUPINFOW {
            cb: u32::try_from(size_of::<STARTUPINFOW>()).expect("STARTUPINFOW is ~100 bytes"),
            ..Default::default()
        };
        let mut info = PROCESS_INFORMATION::default();

        // SAFETY: both strings are NUL-terminated, `command_line` is writable as CreateProcessW
        // requires, and every pointer outlives the call.
        fallible_do(|| unsafe {
            CreateProcessW(
                application.as_ptr(),
                command_line.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                DETACHED_PROCESS,
                ptr::null(),
                ptr::null(),
                &raw const startup_info,
                &raw mut info,
            )
        })?;

        // SAFETY: CreateProcessW succeeded, so `hProcess` is a valid handle the caller owns.
        let inner = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
        // SAFETY: as above, for `hThread`, which we never use.
        drop(unsafe { OwnedHandle::from_raw_handle(info.hThread) });
        Ok(Self {
            inner,
            pid: info.dwProcessId,
        })
    }

    /// Kill the process via the [`TerminateProcess`] call. Requires [`PROCESS_TERMINATE`] access.
    pub fn terminate(&self) -> Result<(), std::io::Error> {
        fallible_do(|| unsafe { TerminateProcess(self.inner.as_raw_handle(), 1) })
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
        let status = unsafe { WaitForSingleObject(self.inner.as_raw_handle(), 0) };
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
}
