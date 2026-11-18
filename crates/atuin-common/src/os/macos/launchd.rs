//! Random `launchd` utilities.
#![allow(unsafe_code, reason = "launchd's API is C.")]

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::os::fd::{FromRawFd, OwnedFd};
use std::ptr::NonNull;

use rustix::io::Errno;

// From libSystem: see `launch_activate_socket(3)` and `free(3)`.
unsafe extern "C" {
    fn launch_activate_socket(name: *const c_char, fds: *mut *mut c_int, cnt: *mut usize) -> c_int;
    fn free(ptr: *mut c_void);
}

/// A socket at a path, a `SockPathName`, that launchd listens on for the calling job.
#[derive(Debug, derive_more::Into)]
pub struct CheckedInSocket(OwnedFd);

impl CheckedInSocket {
    /// launchd's own code, past the errnos, for a socket its sandbox kept it from placing:
    /// `launchctl error 159`.
    const SANDBOX_RESTRICTION: c_int = 159;

    /// Check in for the socket `name` in the job's plist, which only the job launchd started can.
    pub fn activate(name: &CStr) -> Result<Self, ActivateError> {
        let mut fds = std::ptr::null_mut();
        let mut count = 0;
        // SAFETY: `name` is NUL-terminated, and both out-pointers point to live locals.
        let err = unsafe { launch_activate_socket(name.as_ptr(), &raw mut fds, &raw mut count) };
        if err == Self::SANDBOX_RESTRICTION {
            return Err(ActivateError::Sandboxed(name.to_owned()));
        }
        if err != 0 {
            return Err(match Errno::from_raw_os_error(err) {
                Errno::NOENT => ActivateError::NotFound(name.to_owned()),
                Errno::SRCH => ActivateError::NotManaged,
                Errno::ALREADY => ActivateError::AlreadyActivated(name.to_owned()),
                errno => ActivateError::Other(errno.into()),
            });
        }

        // launchd binds a socket at a path the once, and one at a host and port once for each
        // address, of which the rest close here.
        let fd = NonNull::new(fds)
            .map(|ptr| Descriptors { ptr, count }.into_owned())
            .unwrap_or_default()
            .into_iter()
            .next();
        fd.map(Self).ok_or_else(|| ActivateError::NotFound(name.to_owned()))
    }
}

/// Errors from [`CheckedInSocket::activate`].
#[derive(Debug, thiserror::Error)]
pub enum ActivateError {
    /// Also returned until launchd has placed the socket, which it can't while the socket's
    /// directory doesn't exist.
    #[error("launchd holds no socket {0:?} for this job")]
    NotFound(CString),
    #[error("this process is not a job launchd started")]
    NotManaged,
    /// launchd's sandbox kept it from placing the socket, as it does in a directory of the user's
    /// `$TMPDIR` that it would have to create.
    #[error("launchd's sandbox kept it from placing socket {0:?}")]
    Sandboxed(CString),
    #[error("this job has already checked in for socket {0:?}")]
    AlreadyActivated(CString),
    #[error("could not check in with launchd: {0}")]
    Other(std::io::Error),
}

/// The array of descriptors `launch_activate_socket` allocates, which it frees as it drops.
struct Descriptors {
    ptr: NonNull<c_int>,
    count: usize,
}

impl Descriptors {
    /// Take ownership of each descriptor.
    fn into_owned(self) -> Vec<OwnedFd> {
        // SAFETY: launchd stored `count` descriptors at `ptr`.
        let raw = unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.count) };
        raw.iter()
            .map(|&fd| {
                // SAFETY: launchd hands each descriptor over to its caller, and nothing else owns
                // it.
                unsafe { OwnedFd::from_raw_fd(fd) }
            })
            .collect()
    }
}

impl Drop for Descriptors {
    fn drop(&mut self) {
        // SAFETY: launchd allocates the array for its caller to free, and it isn't read after this.
        unsafe { free(self.ptr.as_ptr().cast()) };
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// A test runs outside any launchd job, or in one with no such socket.
    #[rstest]
    fn a_process_launchd_holds_no_socket_for_is_refused() {
        let err = CheckedInSocket::activate(c"atuin-test").unwrap_err();
        assert!(matches!(err, ActivateError::NotManaged | ActivateError::NotFound(_)), "{err:?}");
    }
}
