//! Linux descriptor accounting.

use std::io;

pub(super) use super::unix::{limit, raise_limit};

/// Count open descriptors from the size of `/proc/self/fd`, listing it where the size is unset.
pub(super) fn count_open() -> io::Result<usize> {
    // Linux 6.2 made the directory's size the open count; older kernels report 0.
    let size = std::fs::metadata("/proc/self/fd")?.len();
    if size == 0 {
        return super::unix::count_open();
    }
    usize::try_from(size).map_err(io::Error::other)
}
