use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Kill,
}

#[derive(Debug, thiserror::Error)]
pub enum KillError {
    #[error("no process found with pid {pid}")]
    NoSuchProcess {
        pid: u32,
    },
    #[error("not permitted to signal process with pid {pid}")]
    PermissionDenied {
        pid: u32,
    },
    #[error("failed to signal process with pid {pid}")]
    Os {
        pid: u32,
        #[source]
        source: std::io::Error,
    },
}

pub fn kill(pid: u32, signal: Signal) -> Result<(), KillError> {
    #[cfg(unix)]
    {
        crate::os::unix::process::kill(pid, signal.into())
    }
    #[cfg(windows)]
    {
        crate::os::windows::process::kill(pid, signal.into())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = signal;
        Err(KillError::Os {
            pid,
            source: std::io::Error::from(std::io::ErrorKind::Unsupported),
        })
    }
}

pub fn terminate(pid: u32, timeout: Duration) -> Result<(), KillError> {
    #[cfg(unix)]
    {
        crate::os::unix::process::terminate(pid, timeout)
    }
    #[cfg(windows)]
    {
        crate::os::windows::process::terminate(pid, timeout)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = timeout;
        Err(KillError::Os {
            pid,
            source: std::io::Error::from(std::io::ErrorKind::Unsupported),
        })
    }
}
