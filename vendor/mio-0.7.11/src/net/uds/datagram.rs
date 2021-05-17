use crate::io_source::IoSource;
use crate::{event, sys, Interest, Registry, Token};

use std::net::Shutdown;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::net;
use std::path::Path;
use std::{fmt, io};

/// A Unix datagram socket.
pub struct UnixDatagram {
    inner: IoSource<net::UnixDatagram>,
}

impl UnixDatagram {
    /// Creates a Unix datagram socket bound to the given path.
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<UnixDatagram> {
        sys::uds::datagram::bind(path.as_ref()).map(UnixDatagram::from_std)
    }

    /// Creates a new `UnixDatagram` from a standard `net::UnixDatagram`.
    ///
    /// This function is intended to be used to wrap a Unix datagram from the
    /// standard library in the Mio equivalent. The conversion assumes nothing
    /// about the underlying datagram; ; it is left up to the user to set it
    /// in non-blocking mode.
    pub fn from_std(socket: net::UnixDatagram) -> UnixDatagram {
        UnixDatagram {
            inner: IoSource::new(socket),
        }
    }

    /// Connects the socket to the specified address.
    pub fn connect<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        self.inner.connect(path)
    }

    /// Creates a Unix Datagram socket which is not bound to any address.
    pub fn unbound() -> io::Result<UnixDatagram> {
        sys::uds::datagram::unbound().map(UnixDatagram::from_std)
    }

    /// Create an unnamed pair of connected sockets.
    pub fn pair() -> io::Result<(UnixDatagram, UnixDatagram)> {
        sys::uds::datagram::pair().map(|(socket1, socket2)| {
            (
                UnixDatagram::from_std(socket1),
                UnixDatagram::from_std(socket2),
            )
        })
    }

    /// Returns the address of this socket.
    pub fn local_addr(&self) -> io::Result<sys::SocketAddr> {
        sys::uds::datagram::local_addr(&self.inner)
    }

    /// Returns the address of this socket's peer.
    ///
    /// The `connect` method will connect the socket to a peer.
    pub fn peer_addr(&self) -> io::Result<sys::SocketAddr> {
        sys::uds::datagram::peer_addr(&self.inner)
    }

    /// Receives data from the socket.
    ///
    /// On success, returns the number of bytes read and the address from
    /// whence the data came.
    pub fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, sys::SocketAddr)> {
        self.inner
            .do_io(|inner| sys::uds::datagram::recv_from(inner, buf))
    }

    /// Receives data from the socket.
    ///
    /// On success, returns the number of bytes read.
    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.do_io(|inner| inner.recv(buf))
    }

    /// Sends data on the socket to the specified address.
    ///
    /// On success, returns the number of bytes written.
    pub fn send_to<P: AsRef<Path>>(&self, buf: &[u8], path: P) -> io::Result<usize> {
        self.inner.do_io(|inner| inner.send_to(buf, path))
    }

    /// Sends data on the socket to the socket's peer.
    ///
    /// The peer address may be set by the `connect` method, and this method
    /// will return an error if the socket has not already been connected.
    ///
    /// On success, returns the number of bytes written.
    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        self.inner.do_io(|inner| inner.send(buf))
    }

    /// Returns the value of the `SO_ERROR` option.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.take_error()
    }

    /// Shut down the read, write, or both halves of this connection.
    ///
    /// This function will cause all pending and future I/O calls on the
    /// specified portions to immediately return with an appropriate value
    /// (see the documentation of `Shutdown`).
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.inner.shutdown(how)
    }
}

impl event::Source for UnixDatagram {
    fn register(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        self.inner.register(registry, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        self.inner.reregister(registry, token, interests)
    }

    fn deregister(&mut self, registry: &Registry) -> io::Result<()> {
        self.inner.deregister(registry)
    }
}

impl fmt::Debug for UnixDatagram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

impl IntoRawFd for UnixDatagram {
    fn into_raw_fd(self) -> RawFd {
        self.inner.into_inner().into_raw_fd()
    }
}

impl AsRawFd for UnixDatagram {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl FromRawFd for UnixDatagram {
    /// Converts a `RawFd` to a `UnixDatagram`.
    ///
    /// # Notes
    ///
    /// The caller is responsible for ensuring that the socket is in
    /// non-blocking mode.
    unsafe fn from_raw_fd(fd: RawFd) -> UnixDatagram {
        UnixDatagram::from_std(FromRawFd::from_raw_fd(fd))
    }
}
