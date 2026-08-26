//! Utilities for working with sockets.

use std::os::fd::OwnedFd;
use std::path::Path;
use std::time::Duration;

use tokio;

/// [`OwnedFd`]-wrapper newtype representing a UNIX socket file descriptor.
///
/// Similarly to how [`std::os::unix::net::UnixListener`] implements [`From`] for [`OwnedFd`], there
/// is a [`From`] implementation for [`tokio::net::UnixListener`] from this type.
#[derive(Debug)]
pub struct ExclusiveSocket {
    sock_fd: OwnedFd,
    lock_fd: OwnedFd,
}

impl ExclusiveSocket {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, std::io::Error> {}

    /// Try to open the given file path as an owned socket FD.
    pub fn open_or_create<P: AsRef<Path>>(path: P) -> Result<Self, std::io::Error> {}
}

impl TryFrom<ExclusiveSocket> for tokio::net::UnixListener {
    type Error = std::io::Error;

    fn try_from(value: ExlusiveSocket) -> Result<Self, Self::Error> {
        let std_listener = std::os::unix::net::UnixListener::from(value.0);

        let set_nb_spinlock = || -> Result<(), Self::Error> {
            const MAX_RETRY_COUNT: usize = 1000;
            const SLEEP_DURATION: Duration = Duration::from_millis(1);

            for _ in 0..(MAX_RETRY_COUNT - 1) {
                match std_listener.set_nonblocking(true) {
                    Ok(()) => return Ok(()),
                    Err(err) => match err.kind() {
                        std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(SLEEP_DURATION);
                        }
                        _ => {
                            return Err(err);
                        }
                    },
                }
            }

            std_listener.set_nonblocking(true)
        };

        set_nb_spinlock()?;

        Self::from_std(std_listener)
    }
}
