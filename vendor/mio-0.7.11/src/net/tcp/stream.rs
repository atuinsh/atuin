use std::fmt;
use std::io::{self, IoSlice, IoSliceMut, Read, Write};
use std::net::{self, Shutdown, SocketAddr};
#[cfg(unix)]
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
#[cfg(windows)]
use std::os::windows::io::{AsRawSocket, FromRawSocket, IntoRawSocket, RawSocket};

use crate::io_source::IoSource;
use crate::net::TcpSocket;
use crate::{event, Interest, Registry, Token};

/// A non-blocking TCP stream between a local socket and a remote socket.
///
/// The socket will be closed when the value is dropped.
///
/// # Examples
///
#[cfg_attr(feature = "os-poll", doc = "```")]
#[cfg_attr(not(feature = "os-poll"), doc = "```ignore")]
/// # use std::net::{TcpListener, SocketAddr};
/// # use std::error::Error;
/// #
/// # fn main() -> Result<(), Box<dyn Error>> {
/// let address: SocketAddr = "127.0.0.1:0".parse()?;
/// let listener = TcpListener::bind(address)?;
/// use mio::{Events, Interest, Poll, Token};
/// use mio::net::TcpStream;
/// use std::time::Duration;
///
/// let mut stream = TcpStream::connect(listener.local_addr()?)?;
///
/// let mut poll = Poll::new()?;
/// let mut events = Events::with_capacity(128);
///
/// // Register the socket with `Poll`
/// poll.registry().register(&mut stream, Token(0), Interest::WRITABLE)?;
///
/// poll.poll(&mut events, Some(Duration::from_millis(100)))?;
///
/// // The socket might be ready at this point
/// #     Ok(())
/// # }
/// ```
pub struct TcpStream {
    inner: IoSource<net::TcpStream>,
}

impl TcpStream {
    /// Create a new TCP stream and issue a non-blocking connect to the
    /// specified address.
    pub fn connect(addr: SocketAddr) -> io::Result<TcpStream> {
        let socket = TcpSocket::new_for_addr(addr)?;
        socket.connect(addr)
    }

    /// Creates a new `TcpStream` from a standard `net::TcpStream`.
    ///
    /// This function is intended to be used to wrap a TCP stream from the
    /// standard library in the Mio equivalent. The conversion assumes nothing
    /// about the underlying stream; it is left up to the user to set it in
    /// non-blocking mode.
    ///
    /// # Note
    ///
    /// The TCP stream here will not have `connect` called on it, so it
    /// should already be connected via some other means (be it manually, or
    /// the standard library).
    pub fn from_std(stream: net::TcpStream) -> TcpStream {
        TcpStream {
            inner: IoSource::new(stream),
        }
    }

    /// Returns the socket address of the remote peer of this TCP connection.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.peer_addr()
    }

    /// Returns the socket address of the local half of this TCP connection.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    /// Shuts down the read, write, or both halves of this connection.
    ///
    /// This function will cause all pending and future I/O on the specified
    /// portions to return immediately with an appropriate value (see the
    /// documentation of `Shutdown`).
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.inner.shutdown(how)
    }

    /// Sets the value of the `TCP_NODELAY` option on this socket.
    ///
    /// If set, this option disables the Nagle algorithm. This means that
    /// segments are always sent as soon as possible, even if there is only a
    /// small amount of data. When not set, data is buffered until there is a
    /// sufficient amount to send out, thereby avoiding the frequent sending of
    /// small packets.
    ///
    /// # Notes
    ///
    /// On Windows make sure the stream is connected before calling this method,
    /// by receiving an (writable) event. Trying to set `nodelay` on an
    /// unconnected `TcpStream` is undefined behavior.
    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.inner.set_nodelay(nodelay)
    }

    /// Gets the value of the `TCP_NODELAY` option on this socket.
    ///
    /// For more information about this option, see [`set_nodelay`][link].
    ///
    /// [link]: #method.set_nodelay
    ///
    /// # Notes
    ///
    /// On Windows make sure the stream is connected before calling this method,
    /// by receiving an (writable) event. Trying to get `nodelay` on an
    /// unconnected `TcpStream` is undefined behavior.
    pub fn nodelay(&self) -> io::Result<bool> {
        self.inner.nodelay()
    }

    /// Sets the value for the `IP_TTL` option on this socket.
    ///
    /// This value sets the time-to-live field that is used in every packet sent
    /// from this socket.
    ///
    /// # Notes
    ///
    /// On Windows make sure the stream is connected before calling this method,
    /// by receiving an (writable) event. Trying to set `ttl` on an
    /// unconnected `TcpStream` is undefined behavior.
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.inner.set_ttl(ttl)
    }

    /// Gets the value of the `IP_TTL` option for this socket.
    ///
    /// For more information about this option, see [`set_ttl`][link].
    ///
    /// # Notes
    ///
    /// On Windows make sure the stream is connected before calling this method,
    /// by receiving an (writable) event. Trying to get `ttl` on an
    /// unconnected `TcpStream` is undefined behavior.
    ///
    /// [link]: #method.set_ttl
    pub fn ttl(&self) -> io::Result<u32> {
        self.inner.ttl()
    }

    /// Get the value of the `SO_ERROR` option on this socket.
    ///
    /// This will retrieve the stored error in the underlying socket, clearing
    /// the field in the process. This can be useful for checking errors between
    /// calls.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.inner.take_error()
    }

    /// Receives data on the socket from the remote address to which it is
    /// connected, without removing that data from the queue. On success,
    /// returns the number of bytes peeked.
    ///
    /// Successive calls return the same data. This is accomplished by passing
    /// `MSG_PEEK` as a flag to the underlying recv system call.
    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.peek(buf)
    }
}

impl Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.do_io(|inner| (&*inner).read(buf))
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.inner.do_io(|inner| (&*inner).read_vectored(bufs))
    }
}

impl<'a> Read for &'a TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.do_io(|inner| (&*inner).read(buf))
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.inner.do_io(|inner| (&*inner).read_vectored(bufs))
    }
}

impl Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.do_io(|inner| (&*inner).write(buf))
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.inner.do_io(|inner| (&*inner).write_vectored(bufs))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.do_io(|inner| (&*inner).flush())
    }
}

impl<'a> Write for &'a TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.do_io(|inner| (&*inner).write(buf))
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.inner.do_io(|inner| (&*inner).write_vectored(bufs))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.do_io(|inner| (&*inner).flush())
    }
}

impl event::Source for TcpStream {
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

impl fmt::Debug for TcpStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

#[cfg(unix)]
impl IntoRawFd for TcpStream {
    fn into_raw_fd(self) -> RawFd {
        self.inner.into_inner().into_raw_fd()
    }
}

#[cfg(unix)]
impl AsRawFd for TcpStream {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

#[cfg(unix)]
impl FromRawFd for TcpStream {
    /// Converts a `RawFd` to a `TcpStream`.
    ///
    /// # Notes
    ///
    /// The caller is responsible for ensuring that the socket is in
    /// non-blocking mode.
    unsafe fn from_raw_fd(fd: RawFd) -> TcpStream {
        TcpStream::from_std(FromRawFd::from_raw_fd(fd))
    }
}

#[cfg(windows)]
impl IntoRawSocket for TcpStream {
    fn into_raw_socket(self) -> RawSocket {
        self.inner.into_inner().into_raw_socket()
    }
}

#[cfg(windows)]
impl AsRawSocket for TcpStream {
    fn as_raw_socket(&self) -> RawSocket {
        self.inner.as_raw_socket()
    }
}

#[cfg(windows)]
impl FromRawSocket for TcpStream {
    /// Converts a `RawSocket` to a `TcpStream`.
    ///
    /// # Notes
    ///
    /// The caller is responsible for ensuring that the socket is in
    /// non-blocking mode.
    unsafe fn from_raw_socket(socket: RawSocket) -> TcpStream {
        TcpStream::from_std(FromRawSocket::from_raw_socket(socket))
    }
}
