use std::fmt;
use std::io;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::time::Sleep;

use crate::common::{task, Future, Pin, Poll};

#[allow(unreachable_pub)] // https://github.com/rust-lang/rust/issues/57411
pub use self::addr_stream::AddrStream;
use super::accept::Accept;

/// A stream of connections from binding to an address.
#[must_use = "streams do nothing unless polled"]
pub struct AddrIncoming {
    addr: SocketAddr,
    listener: TcpListener,
    sleep_on_errors: bool,
    tcp_keepalive_timeout: Option<Duration>,
    tcp_nodelay: bool,
    timeout: Option<Pin<Box<Sleep>>>,
}

impl AddrIncoming {
    pub(super) fn new(addr: &SocketAddr) -> crate::Result<Self> {
        let std_listener = StdTcpListener::bind(addr).map_err(crate::Error::new_listen)?;

        AddrIncoming::from_std(std_listener)
    }

    pub(super) fn from_std(std_listener: StdTcpListener) -> crate::Result<Self> {
        // TcpListener::from_std doesn't set O_NONBLOCK
        std_listener
            .set_nonblocking(true)
            .map_err(crate::Error::new_listen)?;
        let listener = TcpListener::from_std(std_listener).map_err(crate::Error::new_listen)?;
        AddrIncoming::from_listener(listener)
    }

    /// Creates a new `AddrIncoming` binding to provided socket address.
    pub fn bind(addr: &SocketAddr) -> crate::Result<Self> {
        AddrIncoming::new(addr)
    }

    /// Creates a new `AddrIncoming` from an existing `tokio::net::TcpListener`.
    pub fn from_listener(listener: TcpListener) -> crate::Result<Self> {
        let addr = listener.local_addr().map_err(crate::Error::new_listen)?;
        Ok(AddrIncoming {
            listener,
            addr,
            sleep_on_errors: true,
            tcp_keepalive_timeout: None,
            tcp_nodelay: false,
            timeout: None,
        })
    }

    /// Get the local address bound to this listener.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Set whether TCP keepalive messages are enabled on accepted connections.
    ///
    /// If `None` is specified, keepalive is disabled, otherwise the duration
    /// specified will be the time to remain idle before sending TCP keepalive
    /// probes.
    pub fn set_keepalive(&mut self, keepalive: Option<Duration>) -> &mut Self {
        self.tcp_keepalive_timeout = keepalive;
        self
    }

    /// Set the value of `TCP_NODELAY` option for accepted connections.
    pub fn set_nodelay(&mut self, enabled: bool) -> &mut Self {
        self.tcp_nodelay = enabled;
        self
    }

    /// Set whether to sleep on accept errors.
    ///
    /// A possible scenario is that the process has hit the max open files
    /// allowed, and so trying to accept a new connection will fail with
    /// `EMFILE`. In some cases, it's preferable to just wait for some time, if
    /// the application will likely close some files (or connections), and try
    /// to accept the connection again. If this option is `true`, the error
    /// will be logged at the `error` level, since it is still a big deal,
    /// and then the listener will sleep for 1 second.
    ///
    /// In other cases, hitting the max open files should be treat similarly
    /// to being out-of-memory, and simply error (and shutdown). Setting
    /// this option to `false` will allow that.
    ///
    /// Default is `true`.
    pub fn set_sleep_on_errors(&mut self, val: bool) {
        self.sleep_on_errors = val;
    }

    fn poll_next_(&mut self, cx: &mut task::Context<'_>) -> Poll<io::Result<AddrStream>> {
        // Check if a previous timeout is active that was set by IO errors.
        if let Some(ref mut to) = self.timeout {
            ready!(Pin::new(to).poll(cx));
        }
        self.timeout = None;

        loop {
            match ready!(self.listener.poll_accept(cx)) {
                Ok((socket, addr)) => {
                    if let Some(dur) = self.tcp_keepalive_timeout {
                        let socket = socket2::SockRef::from(&socket);
                        let conf = socket2::TcpKeepalive::new().with_time(dur);
                        if let Err(e) = socket.set_tcp_keepalive(&conf) {
                            trace!("error trying to set TCP keepalive: {}", e);
                        }
                    }
                    if let Err(e) = socket.set_nodelay(self.tcp_nodelay) {
                        trace!("error trying to set TCP nodelay: {}", e);
                    }
                    return Poll::Ready(Ok(AddrStream::new(socket, addr)));
                }
                Err(e) => {
                    // Connection errors can be ignored directly, continue by
                    // accepting the next request.
                    if is_connection_error(&e) {
                        debug!("accepted connection already errored: {}", e);
                        continue;
                    }

                    if self.sleep_on_errors {
                        error!("accept error: {}", e);

                        // Sleep 1s.
                        let mut timeout = Box::pin(tokio::time::sleep(Duration::from_secs(1)));

                        match timeout.as_mut().poll(cx) {
                            Poll::Ready(()) => {
                                // Wow, it's been a second already? Ok then...
                                continue;
                            }
                            Poll::Pending => {
                                self.timeout = Some(timeout);
                                return Poll::Pending;
                            }
                        }
                    } else {
                        return Poll::Ready(Err(e));
                    }
                }
            }
        }
    }
}

impl Accept for AddrIncoming {
    type Conn = AddrStream;
    type Error = io::Error;

    fn poll_accept(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> Poll<Option<Result<Self::Conn, Self::Error>>> {
        let result = ready!(self.poll_next_(cx));
        Poll::Ready(Some(result))
    }
}

/// This function defines errors that are per-connection. Which basically
/// means that if we get this error from `accept()` system call it means
/// next connection might be ready to be accepted.
///
/// All other errors will incur a timeout before next `accept()` is performed.
/// The timeout is useful to handle resource exhaustion errors like ENFILE
/// and EMFILE. Otherwise, could enter into tight loop.
fn is_connection_error(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::ConnectionRefused
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::ConnectionReset)
}

impl fmt::Debug for AddrIncoming {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AddrIncoming")
            .field("addr", &self.addr)
            .field("sleep_on_errors", &self.sleep_on_errors)
            .field("tcp_keepalive_timeout", &self.tcp_keepalive_timeout)
            .field("tcp_nodelay", &self.tcp_nodelay)
            .finish()
    }
}

mod addr_stream {
    use std::io;
    use std::net::SocketAddr;
    #[cfg(unix)]
    use std::os::unix::io::{AsRawFd, RawFd};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::net::TcpStream;

    use crate::common::{task, Pin, Poll};

    /// A transport returned yieled by `AddrIncoming`.
    #[pin_project::pin_project]
    #[derive(Debug)]
    pub struct AddrStream {
        #[pin]
        inner: TcpStream,
        pub(super) remote_addr: SocketAddr,
    }

    impl AddrStream {
        pub(super) fn new(tcp: TcpStream, addr: SocketAddr) -> AddrStream {
            AddrStream {
                inner: tcp,
                remote_addr: addr,
            }
        }

        /// Returns the remote (peer) address of this connection.
        #[inline]
        pub fn remote_addr(&self) -> SocketAddr {
            self.remote_addr
        }

        /// Consumes the AddrStream and returns the underlying IO object
        #[inline]
        pub fn into_inner(self) -> TcpStream {
            self.inner
        }

        /// Attempt to receive data on the socket, without removing that data
        /// from the queue, registering the current task for wakeup if data is
        /// not yet available.
        pub fn poll_peek(
            &mut self,
            cx: &mut task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<usize>> {
            self.inner.poll_peek(cx, buf)
        }
    }

    impl AsyncRead for AddrStream {
        #[inline]
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            self.project().inner.poll_read(cx, buf)
        }
    }

    impl AsyncWrite for AddrStream {
        #[inline]
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.project().inner.poll_write(cx, buf)
        }

        #[inline]
        fn poll_write_vectored(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            bufs: &[io::IoSlice<'_>],
        ) -> Poll<io::Result<usize>> {
            self.project().inner.poll_write_vectored(cx, bufs)
        }

        #[inline]
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
            // TCP flush is a noop
            Poll::Ready(Ok(()))
        }

        #[inline]
        fn poll_shutdown(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
            self.project().inner.poll_shutdown(cx)
        }

        #[inline]
        fn is_write_vectored(&self) -> bool {
            // Note that since `self.inner` is a `TcpStream`, this could
            // *probably* be hard-coded to return `true`...but it seems more
            // correct to ask it anyway (maybe we're on some platform without
            // scatter-gather IO?)
            self.inner.is_write_vectored()
        }
    }

    #[cfg(unix)]
    impl AsRawFd for AddrStream {
        fn as_raw_fd(&self) -> RawFd {
            self.inner.as_raw_fd()
        }
    }
}
