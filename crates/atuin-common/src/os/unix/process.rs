//! Utilities for operating on processes.

use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::Signal as NixSignal;
use nix::unistd::Pid;

use crate::os::process::{KillError, Signal as CommonSignal};

const TERMINATE_POLL_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Hangup,
    Interrupt,
    Quit,
    Kill,
    Terminate,
    User1,
    User2,
}

impl Signal {
    fn to_nix(self) -> NixSignal {
        match self {
            Self::Hangup => NixSignal::SIGHUP,
            Self::Interrupt => NixSignal::SIGINT,
            Self::Quit => NixSignal::SIGQUIT,
            Self::Kill => NixSignal::SIGKILL,
            Self::Terminate => NixSignal::SIGTERM,
            Self::User1 => NixSignal::SIGUSR1,
            Self::User2 => NixSignal::SIGUSR2,
        }
    }
}

impl From<CommonSignal> for Signal {
    fn from(signal: CommonSignal) -> Self {
        match signal {
            CommonSignal::Kill => Self::Kill,
        }
    }
}

pub fn kill(pid: u32, signal: Signal) -> Result<(), KillError> {
    let raw_pid = i32::try_from(pid).map_err(|_| KillError::NoSuchProcess { pid })?;

    match nix::sys::signal::kill(Pid::from_raw(raw_pid), signal.to_nix()) {
        Ok(()) => Ok(()),
        Err(Errno::ESRCH) => Err(KillError::NoSuchProcess { pid }),
        Err(Errno::EPERM) => Err(KillError::PermissionDenied { pid }),
        Err(errno) => Err(KillError::Os {
            pid,
            source: errno.into(),
        }),
    }
}

pub fn terminate(pid: u32, timeout: Duration) -> Result<(), KillError> {
    match kill(pid, Signal::Terminate) {
        Ok(()) => {}
        Err(KillError::NoSuchProcess { .. }) => return Ok(()),
        Err(err) => return Err(err),
    }

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !is_alive(pid) {
            return Ok(());
        }
        std::thread::sleep(TERMINATE_POLL_INTERVAL);
    }

    if !is_alive(pid) {
        return Ok(());
    }

    match kill(pid, Signal::Kill) {
        Ok(()) | Err(KillError::NoSuchProcess { .. }) => Ok(()),
        Err(err) => Err(err),
    }
}

fn is_alive(pid: u32) -> bool {
    let Ok(raw_pid) = i32::try_from(pid) else {
        return false;
    };

    !matches!(nix::sys::signal::kill(Pid::from_raw(raw_pid), None::<NixSignal>), Err(Errno::ESRCH))
}
