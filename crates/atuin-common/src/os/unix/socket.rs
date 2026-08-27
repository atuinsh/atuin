use std::io::ErrorKind;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::time::Duration;

use crate::os::file::PidFileLock;

#[derive(Debug)]
pub struct ExclusiveSocket(PidFileLock<OwnedFd>);

impl ExclusiveSocket {
    pub fn open_or_create<P: AsRef<Path>, T>(
        path: P,
        lock: &PidFileLock<T>,
    ) -> Result<Self, std::io::Error> {
        let path = path.as_ref();

        let listener = match UnixListener::bind(path) {
            Ok(listener) => listener,
            Err(err) if err.kind() == ErrorKind::AddrInUse => {
                std::fs::remove_file(path)?;
                UnixListener::bind(path)?
            }
            Err(err) => return Err(err),
        };

        Ok(Self(lock.with_payload(OwnedFd::from(listener))))
    }

    fn set_nonblocking(listener: &UnixListener) -> Result<(), std::io::Error> {
        const MAX_RETRIES: usize = 1000;
        const SLEEP: Duration = Duration::from_millis(1);

        for _ in 0..MAX_RETRIES - 1 {
            match listener.set_nonblocking(true) {
                Ok(()) => return Ok(()),
                Err(err) if err.kind() == ErrorKind::WouldBlock => std::thread::sleep(SLEEP),
                Err(err) => return Err(err),
            }
        }

        listener.set_nonblocking(true)
    }
}

impl TryFrom<ExclusiveSocket> for PidFileLock<tokio::net::UnixListener> {
    type Error = std::io::Error;

    fn try_from(value: ExclusiveSocket) -> Result<Self, Self::Error> {
        value.0.try_map(|fd| {
            let listener = UnixListener::from(fd);
            ExclusiveSocket::set_nonblocking(&listener)?;
            tokio::net::UnixListener::from_std(listener)
        })
    }
}
