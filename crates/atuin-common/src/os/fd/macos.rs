//! macOS descriptor accounting, through libproc and sysctl.
#![allow(unsafe_code, reason = "libproc and sysctl have no safe wrapper")]

use std::ffi::c_int;
use std::io;
use std::mem::MaybeUninit;

use rustix::process::{Resource, Rlimit, getpid, getrlimit, setrlimit};

use super::FdLimit;

const ENTRY_BYTES: usize = size_of::<libc::proc_fdinfo>();

/// Room for descriptors opened between sizing the buffer and filling it.
const SLACK_ENTRIES: usize = 32;

/// Count open descriptors with `proc_pidinfo(PROC_PIDLISTFDS)`.
pub(super) fn count_open() -> io::Result<usize> {
    let pid = getpid().as_raw_nonzero().get();
    loop {
        let table = list_fds(pid, &mut [])?;
        // libproc reports failure as 0, and a live process never has an empty table.
        if table == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buf = Vec::<libc::proc_fdinfo>::with_capacity(table / ENTRY_BYTES + SLACK_ENTRIES);
        let written = list_fds(pid, buf.spare_capacity_mut())?;
        // A full buffer may have been cut short; size it again for the table that outgrew it.
        if written < buf.capacity() * ENTRY_BYTES {
            return Ok(written / ENTRY_BYTES);
        }
    }
}

/// The soft `RLIMIT_NOFILE`, capped at `kern.maxfilesperproc`.
pub(super) fn limit() -> FdLimit {
    let soft = FdLimit::from_rlimit(getrlimit(Resource::Nofile).current);
    // The kernel applies the cap even to a soft limit reported as unlimited.
    max_files_per_proc().map_or(soft, |max| soft.min(FdLimit::from_rlimit(Some(max))))
}

/// Raise the soft `RLIMIT_NOFILE` to the hard one, capped at `kern.maxfilesperproc`.
pub(super) fn raise_limit() -> io::Result<FdLimit> {
    let Rlimit { current, maximum } = getrlimit(Resource::Nofile);
    // Older macOS rejects a soft limit past `kern.maxfilesperproc`, unlimited included.
    let max = max_files_per_proc()?;
    let target = Some(maximum.map_or(max, |hard| hard.min(max)));
    if FdLimit::from_rlimit(target) > FdLimit::from_rlimit(current) {
        setrlimit(Resource::Nofile, Rlimit {
            current: target,
            maximum,
        })?;
    }
    Ok(limit())
}

/// `proc_pidinfo(PROC_PIDLISTFDS)` into `buf`, returning the bytes written; an empty `buf` returns
/// the bytes the whole descriptor table needs instead.
fn list_fds(pid: c_int, buf: &mut [MaybeUninit<libc::proc_fdinfo>]) -> io::Result<usize> {
    let len = c_int::try_from(size_of_val(buf)).map_err(io::Error::other)?;
    let ptr = if buf.is_empty() {
        std::ptr::null_mut::<std::ffi::c_void>()
    } else {
        buf.as_mut_ptr().cast()
    };
    // SAFETY: `ptr` is NULL with a zero `len`, or points at `len` writable bytes.
    let bytes = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, ptr, len) };
    usize::try_from(bytes).map_err(|_| io::Error::last_os_error())
}

/// `kern.maxfilesperproc`: the ceiling on any process's descriptors.
fn max_files_per_proc() -> io::Result<u64> {
    let mut value: c_int = 0;
    let mut len = size_of::<c_int>();
    // SAFETY: the name is NUL-terminated, and `value`/`len` describe one writable `c_int`.
    let rc = unsafe {
        libc::sysctlbyname(
            c"kern.maxfilesperproc".as_ptr(),
            (&raw mut value).cast(),
            &raw mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    u64::try_from(value).map_err(io::Error::other)
}
