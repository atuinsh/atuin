//! Descriptor accounting for the Unixes other than macOS: `RLIMIT_NOFILE` and procfs.

use std::io;

use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};

use super::FdLimit;

/// The soft `RLIMIT_NOFILE`.
pub(super) fn limit() -> FdLimit {
    FdLimit::from_rlimit(getrlimit(Resource::Nofile).current)
}

/// Raise the soft `RLIMIT_NOFILE` to the hard one.
pub(super) fn raise_limit() -> io::Result<FdLimit> {
    let Rlimit { current, maximum } = getrlimit(Resource::Nofile);
    if current != maximum {
        setrlimit(Resource::Nofile, Rlimit {
            current: maximum,
            maximum,
        })?;
    }
    Ok(FdLimit::from_rlimit(maximum))
}

/// Count the entries of `/proc/self/fd`, less the one listing it.
///
/// # Errors
///
/// [`io::ErrorKind::Unsupported`] where there is no procfs.
pub(super) fn count_open() -> io::Result<usize> {
    let listing = std::fs::read_dir("/proc/self/fd").map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => io::ErrorKind::Unsupported.into(),
        _ => err,
    })?;
    // The listing's own descriptor is one of its entries.
    Ok(listing.count().saturating_sub(1))
}
