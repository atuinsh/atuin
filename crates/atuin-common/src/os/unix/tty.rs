//! TTY-related utilities.

use std::os::fd::{AsFd, BorrowedFd};

use rustix::fs::Dev;

/// A unique identifier for a terminal device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TtyId {
    /// Which filesystem the terminal lives on.
    ///
    /// Corresponds to `st_dev` in `struct stat`.
    pub dev: Dev,

    /// The device ID of the terminal.
    ///
    /// Corresponds to `st_rdev` in `struct stat`.
    pub rdev: Dev,
}

impl TtyId {
    /// Get the terminal ID of a file descriptor.
    ///
    /// Returns [`None`] if the file is not a terminal.
    #[must_use]
    pub fn from_fd(fd: impl AsFd) -> Option<Self> {
        let fd = fd.as_fd();
        if !rustix::termios::isatty(fd) {
            return None;
        }
        let stat = rustix::fs::fstat(fd).ok()?;
        Some(Self {
            dev: stat.st_dev,
            rdev: stat.st_rdev,
        })
    }

    /// Get the ID of the terminal that this process is attached to.
    ///
    /// Returns [`None`] if none of this process's standard file descriptors are attached to a
    /// terminal.
    #[must_use]
    pub fn current() -> Option<Self> {
        use rustix::stdio::{stderr, stdin, stdout};
        first_tty_id([stdin(), stdout(), stderr()])
    }
}

/// Get the ID of the first FD in `fds` that is a terminal.
fn first_tty_id<'a>(fds: impl IntoIterator<Item = BorrowedFd<'a>>) -> Option<TtyId> {
    fds.into_iter().find_map(TtyId::from_fd)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs::File;
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;

    use rstest::{fixture, rstest};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

    use super::*;

    /// Open a fresh pty pair, returning the master, the slave, and the slave's path.
    #[fixture]
    fn pty() -> (File, File, PathBuf) {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        let name = ptsname(&master, Vec::new()).unwrap();
        let path = PathBuf::from(OsStr::from_bytes(name.as_bytes()));
        let slave = File::options().read(true).write(true).open(&path).unwrap();
        (File::from(master), slave, path)
    }

    #[rstest]
    fn from_fd_of_a_pty_matches_a_stat_of_its_path(pty: (File, File, PathBuf)) {
        use std::os::unix::fs::MetadataExt;

        let (_master, slave, path) = pty;
        let meta = std::fs::metadata(&path).unwrap();

        let id = TtyId::from_fd(&slave).expect("a pty slave is a terminal");

        // `Dev` is already u64 on Linux but is narrower on macOS, so the conversion is not
        // redundant everywhere it compiles.
        #[allow(clippy::unnecessary_cast, reason = "Dev is not u64 on every platform")]
        {
            assert_eq!(id.dev as u64, meta.dev());
            assert_eq!(id.rdev as u64, meta.rdev());
        }
    }

    /// Serialises the tests that swap this process's stdin.
    static STDIN_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    /// Replace stdin with `fd` for as long as the returned guard lives.
    fn with_stdin(fd: BorrowedFd<'_>) -> impl Drop {
        struct Restore {
            saved: std::os::fd::OwnedFd,
            _lock: parking_lot::MutexGuard<'static, ()>,
        }
        impl Drop for Restore {
            fn drop(&mut self) {
                rustix::stdio::dup2_stdin(&self.saved).unwrap();
            }
        }
        let lock = STDIN_LOCK.lock();
        let saved = rustix::io::dup(std::io::stdin()).unwrap();
        rustix::stdio::dup2_stdin(fd).unwrap();
        Restore { saved, _lock: lock }
    }

    #[rstest]
    fn current_finds_the_terminal_on_stdin(pty: (File, File, PathBuf)) {
        let (_master, slave, _path) = pty;
        let expected = TtyId::from_fd(&slave).unwrap();

        let found = {
            let _stdin = with_stdin(slave.as_fd());
            TtyId::current()
        };

        assert_eq!(found, Some(expected));
    }

    #[rstest]
    fn dev_null_is_not_a_terminal() {
        // /dev/null is a character device, so a check based on the file type alone would
        // wrongly accept it.
        let devnull = File::open("/dev/null").unwrap();
        assert_eq!(TtyId::from_fd(&devnull), None);
    }

    #[rstest]
    fn first_tty_id_skips_non_terminals(pty: (File, File, PathBuf)) {
        // Mirrors `atuin search -i </dev/null >/dev/null`, where only stderr is left on the tty.
        let (_master, slave, _path) = pty;
        let devnull = File::open("/dev/null").unwrap();
        let expected = TtyId::from_fd(&slave).unwrap();

        let found = first_tty_id([devnull.as_fd(), devnull.as_fd(), slave.as_fd()]);

        assert_eq!(found, Some(expected));
    }

    #[rstest]
    fn first_tty_id_returns_the_earliest_terminal(
        #[from(pty)] first: (File, File, PathBuf),
        #[from(pty)] second: (File, File, PathBuf),
    ) {
        let (_master_a, slave_a, _path_a) = first;
        let (_master_b, slave_b, _path_b) = second;
        let expected = TtyId::from_fd(&slave_a).unwrap();
        assert_ne!(TtyId::from_fd(&slave_b), Some(expected), "the two ptys must differ");

        let found = first_tty_id([slave_a.as_fd(), slave_b.as_fd()]);

        assert_eq!(found, Some(expected));
    }

    #[rstest]
    fn first_tty_id_is_none_when_nothing_is_a_terminal() {
        // The case we deliberately do not handle: with every standard fd redirected there is no
        // way to tell which terminal we belong to, so the pty proxy is not used.
        let devnull = File::open("/dev/null").unwrap();
        let regular = tempfile::NamedTempFile::new().unwrap();

        let found = first_tty_id([devnull.as_fd(), regular.as_file().as_fd(), devnull.as_fd()]);

        assert_eq!(found, None);
    }
}
