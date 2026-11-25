//! Watching other processes.

#![allow(unsafe_code, reason = "kqueue is a C API")]

use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
use std::ptr;

use rustix::process::Pid;

/// Error returned by [`on_exit`].
#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("the process exited before the watch began")]
    AlreadyExited,
    #[error("can't watch the process: {0}")]
    Unwatchable(std::io::Error),
}

/// Runs `action` on a new thread once the process `pid` exits.
///
/// # Errors
///
/// Fails when the process exited before the watch began, or when the system can't watch the
/// process. `action` never runs after a failure.
pub fn on_exit(pid: Pid, action: impl FnOnce() + Send + 'static) -> Result<(), WatchError> {
    // SAFETY: the function has no preconditions.
    let queue = unsafe { libc::kqueue() };
    if queue < 0 {
        return Err(WatchError::Unwatchable(std::io::Error::last_os_error()));
    }
    // SAFETY: `kqueue` returned a new descriptor that nothing else owns.
    let queue = unsafe { OwnedFd::from_raw_fd(queue) };
    let watch = libc::kevent {
        ident: pid.as_raw_nonzero().get().cast_unsigned() as usize,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: ptr::null_mut(),
    };
    // SAFETY: `watch` outlives the call, and the call writes no events.
    let added = unsafe {
        libc::kevent(queue.as_raw_fd(), &raw const watch, 1, ptr::null_mut(), 0, ptr::null())
    };
    if added < 0 {
        let error = std::io::Error::last_os_error();
        return Err(if error.raw_os_error() == Some(libc::ESRCH) {
            WatchError::AlreadyExited
        } else {
            WatchError::Unwatchable(error)
        });
    }
    std::thread::Builder::new()
        .spawn(move || {
            if wait_for_event(&queue).is_ok() {
                action();
            }
        })
        .map_err(WatchError::Unwatchable)?;
    Ok(())
}

/// Waits for one event on `queue`.
fn wait_for_event(queue: &OwnedFd) -> std::io::Result<()> {
    // SAFETY: the all-zero pattern is a valid event, which the call overwrites.
    let mut event: libc::kevent = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: `event` outlives the call, which writes at most one event.
        let got = unsafe {
            libc::kevent(queue.as_raw_fd(), ptr::null(), 0, &raw mut event, 1, ptr::null())
        };
        if got > 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::sync::mpsc;
    use std::time::Duration;

    use rstest::rstest;

    use super::*;
    use crate::os::unix::process::pid_from_u32;

    #[rstest]
    fn the_action_runs_when_the_process_exits() {
        let mut child = Command::new("sleep").arg("0.2").spawn().expect("sleep starts");
        let (sender, exited) = mpsc::channel();

        on_exit(pid_from_u32(child.id()).unwrap(), move || {
            sender.send(()).expect("the test waits");
        })
        .expect("the child still runs");

        child.wait().expect("sleep exits");
        exited.recv_timeout(Duration::from_secs(5)).expect("the action runs");
    }

    #[rstest]
    fn a_process_that_exited_cannot_be_watched() {
        let mut child = Command::new("true").spawn().expect("true starts");
        let pid = pid_from_u32(child.id()).unwrap();
        child.wait().expect("true exits");

        assert!(matches!(on_exit(pid, || {}), Err(WatchError::AlreadyExited)));
    }
}
