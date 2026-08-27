use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, GetLastError,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

use crate::os::process::{KillError, Signal as CommonSignal};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Kill,
}

impl From<CommonSignal> for Signal {
    fn from(signal: CommonSignal) -> Self {
        match signal {
            CommonSignal::Kill => Self::Kill,
        }
    }
}

pub fn kill(pid: u32, signal: Signal) -> Result<(), KillError> {
    match signal {
        Signal::Kill => terminate_process(pid),
    }
}

pub fn terminate(pid: u32, timeout: Duration) -> Result<(), KillError> {
    let _ = timeout;

    match terminate_process(pid) {
        Ok(()) | Err(KillError::NoSuchProcess { .. }) => Ok(()),
        Err(err) => Err(err),
    }
}

fn terminate_process(pid: u32) -> Result<(), KillError> {
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            return Err(match GetLastError() {
                ERROR_ACCESS_DENIED => KillError::PermissionDenied { pid },
                ERROR_INVALID_PARAMETER => KillError::NoSuchProcess { pid },
                code => KillError::Os {
                    pid,
                    source: std::io::Error::from_raw_os_error(code as i32),
                },
            });
        }

        let terminated = TerminateProcess(handle, 1);
        let code = if terminated == 0 {
            GetLastError()
        } else {
            0
        };
        CloseHandle(handle);

        if terminated == 0 {
            return Err(KillError::Os {
                pid,
                source: std::io::Error::from_raw_os_error(code as i32),
            });
        }
    }

    Ok(())
}
