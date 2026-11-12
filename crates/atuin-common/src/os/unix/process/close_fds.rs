//! Implementation of [`super::CommandExt::close_fds`].

use std::os::fd::RawFd;
use std::os::unix::process::CommandExt as _;
use std::process::Command;

/// The lowest file descriptor that [`close_fds`] possibly closes in the child process. Excludes
/// standard streams (0, 1, 2).
const FIRST_NON_STD_FD: RawFd = 3;

/// The highest file descriptor [`close_fds`] touches when it isn't possible to query the list of
/// open file descriptors.
const FALLBACK_FD_END: RawFd = 256;

/// Implementation of [`super::CommandExt::close_fds`].
#[allow(unsafe_code, reason = "must call `cloexec_fds` and `pre_exec`")]
pub fn close_fds(command: &mut Command) -> &mut Command {
    // List must be obtained outside the `mark_fds` closure to ensure it remains async-signal-safe.
    let open_fds = list_open_fds_from(FIRST_NON_STD_FD);

    // This closure must be async-signal-safe. It may own values whose destructors aren't, but those
    // destructors must be run only once the closure gets dropped, not during an invocation.
    let mark_fds = move || {
        match &open_fds {
            // SAFETY: this runs between `fork` and `exec`.
            Ok(fds) => unsafe { cloexec_fds(fds.iter().copied()) },
            // SAFETY: this runs between `fork` and `exec`.
            Err(()) => unsafe { cloexec_fds(FIRST_NON_STD_FD..FALLBACK_FD_END) },
        }
        Ok(())
    };

    // SAFETY: `mark_fds` is async-signal-safe and doesn't touch any memory except what was
    // captured.
    unsafe { command.pre_exec(mark_fds) }
}

/// List all open file descriptors in the current process, starting with `first`.
fn list_open_fds_from(first: RawFd) -> Result<Vec<RawFd>, ()> {
    let open_fds = if cfg!(target_os = "linux") {
        "/proc/self/fd"
    } else if cfg!(target_os = "macos") {
        "/dev/fd"
    } else {
        return Err(());
    };

    // The directory's own descriptor is listed too, but it's closed again by the time anything
    // marks it, which then fails with `EBADF`, or marks a close-on-exec descriptor of std's that
    // reused its number.
    let fds = std::fs::read_dir(open_fds)
        .map_err(|_| ())?
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
        .filter(|&fd| fd >= first)
        .collect();
    Ok(fds)
}

/// Mark a list of file descriptors as close-on-exec.
///
/// This function is async-signal-safe as long as [`F::into_iter()`](IntoIterator::into_iter),
/// [`F::IntoIter::next`](Iterator::next), and [`F::IntoIter::drop`](Drop::drop) are all
/// async-signal-safe.
///
/// # Safety
///
/// May be called only between `fork` and `exec` in a child process (i.e., in a `pre_exec` hook).
#[allow(unsafe_code, reason = "must call raw libc function")]
unsafe fn cloexec_fds<F>(fds: F)
where
    F: IntoIterator<Item = RawFd>,
{
    for fd in fds {
        // SAFETY: Raw libc call: a file descriptor is a valid first argument, `F_SETFD` is a valid
        // operation for the second argument, and `FD_CLOEXEC` is a valid third argument as expected
        // by `F_SETFD`.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
}

#[allow(unsafe_code, reason = "test-only code; must call various unsafe APIs")]
#[cfg(test)]
mod tests {
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
    use std::process::Command;

    use rstest::rstest;

    use super::super::CommandExt as _;
    use super::*;

    /// Serializes descriptor-limit updates and inheritable-descriptor checks across test threads.
    static INHERITABLE_FD: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    /// A descriptor on /dev/null above the standard streams, open without close-on-exec as a shell
    /// may leave one. Closed on drop.
    struct InheritableFd {
        fd: OwnedFd,
        _exclusive: parking_lot::MutexGuard<'static, ()>,
    }

    impl InheritableFd {
        fn open(min_fd: RawFd) -> Self {
            let exclusive = INHERITABLE_FD.lock();
            raise_fd_limit_past(min_fd);
            let devnull = std::fs::File::open("/dev/null").unwrap();
            let mut fd = dup_from(&devnull, min_fd);
            if min_fd < FALLBACK_FD_END && fd.as_raw_fd() >= FALLBACK_FD_END {
                drop(fd);
                fd = dup_from(&devnull, FIRST_NON_STD_FD);
                assert!(
                    fd.as_raw_fd() < FALLBACK_FD_END,
                    "no free descriptor below {FALLBACK_FD_END}"
                );
            }
            Self {
                fd,
                _exclusive: exclusive,
            }
        }

        /// Whether a child spawned with `setup` finds this descriptor open.
        fn inherited_with(&self, setup: Setup) -> bool {
            let mut test = Command::new("test");
            test.arg("-e").arg(format!("/dev/fd/{}", self.fd.as_raw_fd()));
            setup.apply(&mut test);
            test.status().unwrap().success()
        }
    }

    /// Duplicate `file` onto an unused descriptor at or above `min_fd` without close-on-exec.
    fn dup_from(file: &std::fs::File, min_fd: RawFd) -> OwnedFd {
        // SAFETY: `file` is an open descriptor. `F_DUPFD` allocates a fresh descriptor at or above
        // `min_fd` and leaves close-on-exec cleared.
        let raw = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD, min_fd) };
        assert!(raw >= min_fd, "fcntl(F_DUPFD, {min_fd}): {}", std::io::Error::last_os_error());
        // SAFETY: `fcntl(F_DUPFD)` returned a new open descriptor that nothing else owns.
        unsafe { OwnedFd::from_raw_fd(raw) }
    }

    /// Raise the soft limit on open descriptors so `F_DUPFD` has room at or above `min_fd`.
    /// macOS starts with a soft limit of 256.
    fn raise_fd_limit_past(min_fd: RawFd) {
        use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};

        let limit = getrlimit(Resource::Nofile);
        let mut needed = u64::try_from(min_fd).unwrap() + 64;
        if let Some(max) = limit.maximum {
            needed = needed.min(max);
        }
        if limit.current.is_some_and(|current| current < needed) {
            let raised = Rlimit {
                current: Some(needed),
                maximum: limit.maximum,
            };
            setrlimit(Resource::Nofile, raised)
                .unwrap_or_else(|e| panic!("raising the limit past descriptor {min_fd}: {e}"));
        }
    }

    /// What a test does about descriptors before spawning its child.
    #[derive(Clone, Copy, Debug)]
    enum Setup {
        Nothing,
        CloseFds,
        /// What [`close_fds`] does when it can't list descriptors, which on Linux and macOS it
        /// always can.
        Fallback,
    }

    impl Setup {
        fn apply(self, cmd: &mut Command) {
            match self {
                Self::Nothing => {}
                Self::CloseFds => {
                    cmd.close_fds();
                }
                Self::Fallback => {
                    let mark_fds = || {
                        // SAFETY: this runs between `fork` and `exec`.
                        unsafe { cloexec_fds(FIRST_NON_STD_FD..FALLBACK_FD_END) };
                        Ok(())
                    };
                    // SAFETY: `mark_fds` only makes async-signal-safe system calls.
                    unsafe { cmd.pre_exec(mark_fds) };
                }
            }
        }
    }

    #[rstest]
    #[case::by_default(Setup::Nothing, 1000, true)]
    #[case::not_with_close_fds(Setup::CloseFds, 1000, false)]
    #[case::not_bashs_saved_stdout_with_the_fallback(Setup::Fallback, 254, false)]
    #[case::one_past_the_fallbacks_end(Setup::Fallback, FALLBACK_FD_END, true)]
    fn the_child_inherits_a_descriptor(
        #[case] setup: Setup,
        #[case] min_fd: RawFd,
        #[case] inherited: bool,
    ) {
        assert_eq!(InheritableFd::open(min_fd).inherited_with(setup), inherited);
    }

    /// Std reports a failed `exec` through a pipe in the child, which `close_fds` must leave open
    /// until the `exec`; closed early, the failure would read as a spawn that worked.
    #[rstest]
    fn a_failed_exec_is_still_reported() {
        let err = Command::new("/nonexistent/atuin").close_fds().spawn().unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
