//! Process file-descriptor accounting: how many are open, and how many may be.
//!
//! Counting is cheap wherever it is supported: one `stat` of `/proc/self/fd` on Linux 6.2+, whose
//! size the kernel reports as the open count, one `proc_pidinfo` call on macOS, and a listing of
//! `/proc/self/fd` on older Linux and other Unixes. Windows has neither a count nor a
//! per-process limit worth enforcing.
#![allow(
    clippy::disallowed_methods,
    reason = "the pool's own descriptor count; leasing from itself would be circular"
)]

use std::io;

#[cfg(any(target_os = "linux", target_os = "android"))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(all(unix, not(target_os = "macos")))]
mod unix;

#[cfg(any(target_os = "linux", target_os = "android"))]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android", target_os = "macos"))))]
use unix as platform;
#[cfg(windows)]
use windows as platform;

/// How many descriptors the process may hold open at once.
///
/// Orders by that count, [`FdLimit::Unbounded`] last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FdLimit {
    /// Opening a descriptor past this many fails with `EMFILE`.
    Bounded(usize),
    /// No per-process limit worth enforcing.
    Unbounded,
}

impl FdLimit {
    /// From an rlimit value, where `None` is `RLIM_INFINITY`.
    #[cfg(unix)]
    fn from_rlimit(raw: Option<u64>) -> Self {
        raw.and_then(|n| usize::try_from(n).ok()).map_or(Self::Unbounded, Self::Bounded)
    }
}

/// Count the descriptors this process holds open.
///
/// # Errors
///
/// [`io::ErrorKind::Unsupported`] where the platform offers no count, or the failure of the query.
pub fn count_open() -> io::Result<usize> {
    platform::count_open()
}

/// The descriptor limit in effect for this process.
#[must_use]
pub fn limit() -> FdLimit {
    platform::limit()
}

/// Raise the soft descriptor limit as far as the process may, returning the limit now in effect.
///
/// Child processes inherit the raised limit.
///
/// # Errors
///
/// The failure of `setrlimit`.
pub fn raise_limit() -> io::Result<FdLimit> {
    platform::raise_limit()
}

#[cfg(windows)]
mod windows {
    use std::io;

    use super::FdLimit;

    pub(super) fn count_open() -> io::Result<usize> {
        Err(io::ErrorKind::Unsupported.into())
    }

    pub(super) const fn limit() -> FdLimit {
        FdLimit::Unbounded
    }

    #[allow(clippy::unnecessary_wraps, reason = "mirrors the fallible Unix signature")]
    pub(super) const fn raise_limit() -> io::Result<FdLimit> {
        Ok(FdLimit::Unbounded)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    // An exact delta needs nothing else in the process opening descriptors meanwhile, which
    // nextest's process-per-test gives.
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    #[rstest]
    fn count_open_follows_opens_and_closes() {
        let dir = tempfile::tempdir().unwrap();
        let before = count_open().unwrap();
        let files: Vec<_> = (0..16)
            .map(|i| std::fs::File::create(dir.path().join(i.to_string())).unwrap())
            .collect();
        assert_eq!(count_open().unwrap(), before + 16);
        drop(files);
        assert_eq!(count_open().unwrap(), before);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[rstest]
    fn the_procfs_size_agrees_with_the_listing() {
        assert_eq!(linux::count_open().unwrap(), unix::count_open().unwrap());
    }

    #[rstest]
    fn raising_the_limit_never_lowers_it_and_is_idempotent() {
        let before = limit();
        let raised = raise_limit().unwrap();
        assert!(raised >= before, "raised to {raised:?} from {before:?}");
        assert_eq!(limit(), raised);
        assert_eq!(raise_limit().unwrap(), raised);
    }
}
