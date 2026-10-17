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

/// A socket launchd listens on for the calling job, whose descriptors close as they drop.
#[derive(Debug)]
pub struct CheckedInSocket {
    /// One for each address launchd bound, so one for a `SockPathName`. Never empty.
    fds: Vec<OwnedFd>,
}

impl CheckedInSocket {
    /// Check in for the socket `name` in the job's plist, which only the job launchd started can.
    pub fn activate(name: &CStr) -> Result<Self, ActivateError> {
        let mut fds = std::ptr::null_mut();
        let mut count = 0;
        // SAFETY: `name` is NUL-terminated, and both out-pointers point to live locals.
        let err = unsafe { launch_activate_socket(name.as_ptr(), &raw mut fds, &raw mut count) };
        if err != 0 {
            return Err(match Errno::from_raw_os_error(err) {
                Errno::NOENT => ActivateError::NotFound(name.to_owned()),
                Errno::SRCH => ActivateError::NotManaged,
                Errno::ALREADY => ActivateError::AlreadyActivated(name.to_owned()),
                errno => ActivateError::Other(errno.into()),
            });
        }

        let fds = NonNull::new(fds)
            .map(|ptr| Descriptors { ptr, count }.into_owned())
            .unwrap_or_default();
        if fds.is_empty() {
            return Err(ActivateError::NotFound(name.to_owned()));
        }
        Ok(Self { fds })
    }
}

impl IntoIterator for CheckedInSocket {
    type IntoIter = std::vec::IntoIter<OwnedFd>;
    type Item = OwnedFd;

    fn into_iter(self) -> Self::IntoIter {
        self.fds.into_iter()
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
