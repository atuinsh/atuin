//! TTY-related utilities.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use rustix::fs::{Dev, Mode, OFlags};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

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
        [stdin(), stdout(), stderr()].into_iter().find_map(Self::from_fd)
    }

    /// Whether this terminal is a Linux virtual console (`/dev/tty1`, `/dev/tty2`, etc).
    ///
    /// Always returns false on other platforms.
    #[must_use]
    pub fn is_virtual_console(self) -> bool {
        // From <https://github.com/torvalds/linux/blob/v7.3/Documentation/admin-guide/devices.txt>:
        // virtual consoles are char major 4, minor 0 to 63.
        const TTY_MAJOR: u32 = 4;
        const MAX_VT_MINOR: u32 = 63;

        cfg!(target_os = "linux")
            && rustix::fs::major(self.rdev) == TTY_MAJOR
            && rustix::fs::minor(self.rdev) <= MAX_VT_MINOR
    }
}

/// A terminal descriptor in non-blocking mode.
#[derive(Debug)]
pub struct AsyncTty(AsyncFd<File>);

impl AsyncTty {
    /// Register `fd` with the reactor, switching it to non-blocking mode.
    ///
    /// **Be warned**: the mode belongs to the open file description, so every descriptor sharing it
    /// turns non-blocking too. See [`Self::reopen`] for a descriptor someone else may share.
    pub fn new(fd: OwnedFd) -> io::Result<Self> {
        rustix::io::ioctl_fionbio(&fd, true)?;
        let file = File::from(fd);
        // SAFETY: `file` owns the descriptor, and `AsyncTty` never hands out a mutable reference
        // to it, so it stays open and unchanged for as long as the `AsyncFd` lives.
        #[allow(unsafe_code, reason = "registering an owned descriptor with the reactor")]
        let inner = unsafe { AsyncFd::register(file) }?;
        Ok(Self(inner))
    }

    /// Open the terminal behind `fd` afresh, so that making it non-blocking leaves `fd` alone.
    pub fn reopen(fd: BorrowedFd<'_>, access: Access) -> io::Result<Self> {
        let access = match access {
            Access::Read => OFlags::RDONLY,
            Access::Write => OFlags::WRONLY,
        };
        let path = rustix::termios::ttyname(fd, Vec::new())?;
        let fd = rustix::fs::open(
            path.as_c_str(),
            access | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        Self::new(fd)
    }
}

impl AsFd for AsyncTty {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.get_ref().as_fd()
    }
}

/// Which way a reopened terminal is used.
#[derive(Clone, Copy, Debug)]
pub enum Access {
    Read,
    Write,
}

impl AsyncRead for AsyncTty {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut guard = ready!(self.0.poll_read_ready(cx))?;
            let unfilled = buf.initialize_unfilled();
            let len = unfilled.len();
            match guard.try_io(|fd| fd.get_ref().read(unfilled)) {
                Ok(Ok(read)) => {
                    if read > 0 && read < len {
                        guard.clear_ready();
                    }
                    buf.advance(read);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_would_block) => {}
            }
        }
    }
}

impl AsyncWrite for AsyncTty {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.0.poll_write_ready(cx))?;
            match guard.try_io(|fd| fd.get_ref().write(buf)) {
                Ok(Ok(written)) => {
                    if written > 0 && written < buf.len() {
                        guard.clear_ready();
                    }
                    return Poll::Ready(Ok(written));
                }
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_would_block) => {}
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs::File;
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::time::Duration;

    use rstest::{fixture, rstest};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(5);

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
        #[allow(
            clippy::unnecessary_cast,
            clippy::cast_sign_loss,
            reason = "Dev is not u64 on every platform"
        )]
        {
            assert_eq!(id.dev as u64, meta.dev());
            assert_eq!(id.rdev as u64, meta.rdev());
        }
    }

    #[cfg(target_os = "linux")]
    #[rstest]
    #[case::tty0(4, 0, true)]
    #[case::tty1(4, 1, true)]
    #[case::tty63(4, 63, true)]
    #[case::ttys0(4, 64, false)]
    #[case::console(5, 1, false)]
    #[case::dev_tty(5, 0, false)]
    #[case::ptmx(5, 2, false)]
    #[case::pts0(136, 0, false)]
    fn virtual_consoles_are_recognised_by_device_number(
        #[case] major: u32,
        #[case] minor: u32,
        #[case] expected: bool,
    ) {
        let id = TtyId {
            dev: 0,
            rdev: rustix::fs::makedev(major, minor),
        };

        assert_eq!(id.is_virtual_console(), expected);
    }

    #[rstest]
    fn a_pty_is_not_a_virtual_console(pty: (File, File, PathBuf)) {
        let (_master, slave, _path) = pty;

        assert!(!TtyId::from_fd(&slave).unwrap().is_virtual_console());
    }

    /// Serialises the tests that swap this process's stdin.
    static STDIN_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    /// Replace stdin with `fd` for as long as the returned guard lives.
    fn with_stdin(fd: std::os::fd::BorrowedFd<'_>) -> impl Drop {
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
    #[tokio::test]
    async fn reopening_leaves_the_original_descriptor_blocking(
        pty: (File, File, PathBuf),
        #[values(Access::Read, Access::Write)] access: Access,
    ) {
        // The reason to reopen: everything else sharing the inherited description, this
        // process's stderr among it, must not start seeing `EAGAIN`.
        let (_master, slave, _path) = pty;

        let reopened = AsyncTty::reopen(slave.as_fd(), access).unwrap();

        assert!(rustix::fs::fcntl_getfl(&reopened).unwrap().contains(OFlags::NONBLOCK));
        assert!(!rustix::fs::fcntl_getfl(&slave).unwrap().contains(OFlags::NONBLOCK));
    }

    #[rstest]
    #[tokio::test]
    async fn a_read_waits_on_the_reactor_for_input(pty: (File, File, PathBuf)) {
        let (mut master, slave, _path) = pty;
        // Nobody reads the master, and on macOS closing a terminal with its echo still queued
        // waits over half a second for it to drain.
        let mut termios = rustix::termios::tcgetattr(&slave).unwrap();
        termios.local_modes.remove(rustix::termios::LocalModes::ECHO);
        rustix::termios::tcsetattr(&slave, rustix::termios::OptionalActions::Now, &termios)
            .unwrap();
        let mut tty = AsyncTty::reopen(slave.as_fd(), Access::Read).unwrap();
        let read = tokio::spawn(async move {
            let mut buf = [0u8; 64];
            let n = tty.read(&mut buf).await.unwrap();
            buf[..n].to_vec()
        });
        // Let the read find nothing and park before there is anything to read.
        tokio::task::yield_now().await;

        master.write_all(b"typed\n").unwrap();

        let got = tokio::time::timeout(TIMEOUT, read).await.unwrap().unwrap();
        assert_eq!(got, b"typed\n");
    }

    #[rstest]
    #[tokio::test]
    async fn a_write_reaches_the_other_end(pty: (File, File, PathBuf)) {
        let (mut master, slave, _path) = pty;
        let mut tty = AsyncTty::reopen(slave.as_fd(), Access::Write).unwrap();

        tty.write_all(b"output").await.unwrap();

        let mut got = [0u8; 6];
        master.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"output");
    }

    #[rstest]
    fn only_a_terminal_can_be_reopened() {
        // Callers fall back to blocking I/O on this error, so a pipe has to produce one.
        let (reader, _writer) = std::io::pipe().unwrap();

        assert!(AsyncTty::reopen(reader.as_fd(), Access::Read).is_err());
    }
}
