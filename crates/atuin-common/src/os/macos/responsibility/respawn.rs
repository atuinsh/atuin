//! Running the rest of a program in a child that is responsible for itself.

use std::ffi::{OsStr, c_int};
use std::sync::atomic::{AtomicI32, Ordering};

use rustix::process::{Pid, getppid};

use super::{DisclaimedCommand, SpawnDisclaimedError};
use crate::os::macos::process::{WatchError, on_exit};

/// The child that the signal handler passes signals on to.
static CHILD: AtomicI32 = AtomicI32::new(0);

/// Error returned by [`respawn_disclaimed`].
#[derive(Debug, thiserror::Error)]
pub enum RespawnError {
    #[error("can't find this program's executable: {0}")]
    CurrentExe(std::io::Error),
    #[error("can't spawn this program as its own responsible process: {0}")]
    Spawn(SpawnDisclaimedError),
    #[error(
        "can't watch the parent process, so this process will keep running if the parent is \
         killed: {0}"
    )]
    ParentUnwatched(std::io::Error),
}

/// Runs the rest of the program in a child process that is responsible for itself.
///
/// The function returns in the child. The parent waits for the child, passes SIGHUP, SIGINT,
/// SIGQUIT, and SIGTERM on to the child, and exits with the child's status. The child exits once
/// the parent exits, so a caller that kills the parent leaves nothing behind.
///
/// `marker` names the environment variable that tells the child that it is the child. Call this
/// function first in `main`, before the program reads its input or starts a thread.
///
/// # Errors
///
/// Fails in the parent when the parent can't spawn the child, and the parent then keeps the
/// responsible process that it has. Fails in the child when the child can't watch the parent,
/// and the child then keeps running when the parent exits. The program can go on after either
/// failure.
pub fn respawn_disclaimed(marker: impl AsRef<OsStr>) -> Result<(), RespawnError> {
    let marker = marker.as_ref();
    let parent = getppid();
    let marked_parent = crate::env::var_os(marker)
        .and_then(|marker| marker.to_str()?.parse().ok())
        .and_then(Pid::from_raw);
    if let Some(marked_parent) = marked_parent {
        if parent == Some(marked_parent) {
            return exit_with(marked_parent);
        }
        // launchd adopted this process because the parent exited before this check.
        if parent.is_some_and(Pid::is_init) {
            exit_now();
        }
        // Any other parent means that the marker was meant for an ancestor of this process.
    }

    let exe = crate::os::process::current_exe().map_err(RespawnError::CurrentExe)?;
    let child = DisclaimedCommand::new(exe)
        .args(std::env::args_os().skip(1))
        .env(marker, std::process::id().to_string())
        .spawn()
        .map_err(RespawnError::Spawn)?;
    std::process::exit(wait_forwarding_signals(child));
}

fn exit_now() -> ! {
    // SAFETY: `_exit` ends the process without running any code of this process.
    unsafe { libc::_exit(1) }
}

/// Exits this process as soon as the process `parent` exits.
fn exit_with(parent: Pid) -> Result<(), RespawnError> {
    let watched = match on_exit(parent, || exit_now()) {
        Ok(()) => Ok(()),
        Err(WatchError::AlreadyExited) => exit_now(),
        Err(WatchError::Unwatchable(error)) => Err(RespawnError::ParentUnwatched(error)),
    };
    // A parent that exited before the watch began hands this process to launchd, and its process
    // id may already belong to another process that the watch would then follow.
    if getppid() != Some(parent) {
        exit_now();
    }
    watched
}

extern "C" fn forward(signal: c_int) {
    let child = CHILD.load(Ordering::Relaxed);
    if child > 0 {
        // SAFETY: `kill` is async-signal-safe.
        unsafe { libc::kill(child, signal) };
    }
}

/// Passes signals that end a process on to `child`, waits for `child`, and returns the status to
/// exit with.
///
/// A child that a signal ends gives the status that a shell reports, 128 plus the signal's number.
fn wait_forwarding_signals(child: Pid) -> i32 {
    let child = child.as_raw_nonzero().get();
    CHILD.store(child, Ordering::Relaxed);
    #[expect(clippy::fn_to_numeric_cast_any, reason = "`signal` takes the handler as an address")]
    let handler = forward as extern "C" fn(c_int) as libc::sighandler_t;
    for signal in [libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM] {
        // SAFETY: `forward` only calls async-signal-safe functions.
        unsafe { libc::signal(signal, handler) };
    }
    let mut status = 0;
    loop {
        // SAFETY: `status` outlives the wait.
        let waited = unsafe { libc::waitpid(child, &raw mut status, 0) };
        if waited != -1 {
            break;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return 1;
        }
    }
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        1
    }
}
