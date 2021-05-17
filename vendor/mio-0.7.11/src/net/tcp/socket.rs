use std::io;
use std::mem;
use std::net::SocketAddr;
#[cfg(unix)]
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
#[cfg(windows)]
use std::os::windows::io::{AsRawSocket, FromRawSocket, IntoRawSocket, RawSocket};
use std::time::Duration;

use crate::net::{TcpListener, TcpStream};
use crate::sys;

/// A non-blocking TCP socket used to configure a stream or listener.
///
/// The `TcpSocket` type wraps the operating-system's socket handle. This type
/// is used to configure the socket before establishing a connection or start
/// listening for inbound connections.
///
/// The socket will be closed when the value is dropped.
#[derive(Debug)]
pub struct TcpSocket {
    sys: sys::tcp::TcpSocket,
}

/// Configures a socket's TCP keepalive parameters.
#[derive(Debug, Default, Clone)]
pub struct TcpKeepalive {
    pub(crate) time: Option<Duration>,
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "windows",
    ))]
    pub(crate) interval: Option<Duration>,
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
    ))]
    pub(crate) retries: Option<u32>,
}

impl TcpSocket {
    /// Create a new IPv4 TCP socket.
    ///
    /// This calls `socket(2)`.
    pub fn new_v4() -> io::Result<TcpSocket> {
        sys::tcp::new_v4_socket().map(|sys| TcpSocket { sys })
    }

    /// Create a new IPv6 TCP socket.
    ///
    /// This calls `socket(2)`.
    pub fn new_v6() -> io::Result<TcpSocket> {
        sys::tcp::new_v6_socket().map(|sys| TcpSocket { sys })
    }

    pub(crate) fn new_for_addr(addr: SocketAddr) -> io::Result<TcpSocket> {
        if addr.is_ipv4() {
            TcpSocket::new_v4()
        } else {
            TcpSocket::new_v6()
        }
    }

    /// Bind `addr` to the TCP socket.
    pub fn bind(&self, addr: SocketAddr) -> io::Result<()> {
        sys::tcp::bind(self.sys, addr)
    }

    /// Connect the socket to `addr`.
    ///
    /// This consumes the socket and performs the connect operation. Once the
    /// connection completes, the socket is now a non-blocking `TcpStream` and
    /// can be used as such.
    pub fn connect(self, addr: SocketAddr) -> io::Result<TcpStream> {
        let stream = sys::tcp::connect(self.sys, addr)?;

        // Don't close the socket
        mem::forget(self);
        Ok(TcpStream::from_std(stream))
    }

    /// Listen for inbound connections, converting the socket to a
    /// `TcpListener`.
    pub fn listen(self, backlog: u32) -> io::Result<TcpListener> {
        let listener = sys::tcp::listen(self.sys, backlog)?;

        // Don't close the socket
        mem::forget(self);
        Ok(TcpListener::from_std(listener))
    }

    /// Sets the value of `SO_REUSEADDR` on this socket.
    pub fn set_reuseaddr(&self, reuseaddr: bool) -> io::Result<()> {
        sys::tcp::set_reuseaddr(self.sys, reuseaddr)
    }

    /// Get the value of `SO_REUSEADDR` set on this socket.
    pub fn get_reuseaddr(&self) -> io::Result<bool> {
        sys::tcp::get_reuseaddr(self.sys)
    }

    /// Sets the value of `SO_REUSEPORT` on this socket.
    /// Only supported available in unix
    #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
    pub fn set_reuseport(&self, reuseport: bool) -> io::Result<()> {
        sys::tcp::set_reuseport(self.sys, reuseport)
    }

    /// Get the value of `SO_REUSEPORT` set on this socket.
    /// Only supported available in unix
    #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
    pub fn get_reuseport(&self) -> io::Result<bool> {
        sys::tcp::get_reuseport(self.sys)
    }

    /// Sets the value of `SO_LINGER` on this socket.
    pub fn set_linger(&self, dur: Option<Duration>) -> io::Result<()> {
        sys::tcp::set_linger(self.sys, dur)
    }

    /// Gets the value of `SO_LINGER` on this socket
    pub fn get_linger(&self) -> io::Result<Option<Duration>> {
        sys::tcp::get_linger(self.sys)
    }

    /// Sets the value of `SO_RCVBUF` on this socket.
    pub fn set_recv_buffer_size(&self, size: u32) -> io::Result<()> {
        sys::tcp::set_recv_buffer_size(self.sys, size)
    }

    /// Get the value of `SO_RCVBUF` set on this socket.
    ///
    /// Note that if [`set_recv_buffer_size`] has been called on this socket
    /// previously, the value returned by this function may not be the same as
    /// the argument provided to `set_recv_buffer_size`. This is for the
    /// following reasons:
    ///
    /// * Most operating systems have minimum and maximum allowed sizes for the
    ///   receive buffer, and will clamp the provided value if it is below the
    ///   minimum or above the maximum. The minimum and maximum buffer sizes are
    ///   OS-dependent.
    /// * Linux will double the buffer size to account for internal bookkeeping
    ///   data, and returns the doubled value from `getsockopt(2)`. As per `man
    ///   7 socket`:
    ///   > Sets or gets the maximum socket receive buffer in bytes. The
    ///   > kernel doubles this value (to allow space for bookkeeping
    ///   > overhead) when it is set using `setsockopt(2)`, and this doubled
    ///   > value is returned by `getsockopt(2)`.
    ///
    /// [`set_recv_buffer_size`]: #method.set_recv_buffer_size
    pub fn get_recv_buffer_size(&self) -> io::Result<u32> {
        sys::tcp::get_recv_buffer_size(self.sys)
    }

    /// Sets the value of `SO_SNDBUF` on this socket.
    pub fn set_send_buffer_size(&self, size: u32) -> io::Result<()> {
        sys::tcp::set_send_buffer_size(self.sys, size)
    }

    /// Get the value of `SO_SNDBUF` set on this socket.
    ///
    /// Note that if [`set_send_buffer_size`] has been called on this socket
    /// previously, the value returned by this function may not be the same as
    /// the argument provided to `set_send_buffer_size`. This is for the
    /// following reasons:
    ///
    /// * Most operating systems have minimum and maximum allowed sizes for the
    ///   receive buffer, and will clamp the provided value if it is below the
    ///   minimum or above the maximum. The minimum and maximum buffer sizes are
    ///   OS-dependent.
    /// * Linux will double the buffer size to account for internal bookkeeping
    ///   data, and returns the doubled value from `getsockopt(2)`. As per `man
    ///   7 socket`:
    ///   > Sets or gets the maximum socket send buffer in bytes. The
    ///   > kernel doubles this value (to allow space for bookkeeping
    ///   > overhead) when it is set using `setsockopt(2)`, and this doubled
    ///   > value is returned by `getsockopt(2)`.
    ///
    /// [`set_send_buffer_size`]: #method.set_send_buffer_size
    pub fn get_send_buffer_size(&self) -> io::Result<u32> {
        sys::tcp::get_send_buffer_size(self.sys)
    }

    /// Sets whether keepalive messages are enabled to be sent on this socket.
    ///
    /// This will set the `SO_KEEPALIVE` option on this socket.
    pub fn set_keepalive(&self, keepalive: bool) -> io::Result<()> {
        sys::tcp::set_keepalive(self.sys, keepalive)
    }

    /// Returns whether or not TCP keepalive probes will be sent by this socket.
    pub fn get_keepalive(&self) -> io::Result<bool> {
        sys::tcp::get_keepalive(self.sys)
    }

    /// Sets parameters configuring TCP keepalive probes for this socket.
    ///
    /// The supported parameters depend on the operating system, and are
    /// configured using the [`TcpKeepalive`] struct. At a minimum, all systems
    /// support configuring the [keepalive time]: the time after which the OS
    /// will start sending keepalive messages on an idle connection.
    ///
    /// # Notes
    ///
    /// * This will enable TCP keepalive on this socket, if it is not already
    ///   enabled.
    /// * On some platforms, such as Windows, any keepalive parameters *not*
    ///   configured by the `TcpKeepalive` struct passed to this function may be
    ///   overwritten with their default values. Therefore, this function should
    ///   either only be called once per socket, or the same parameters should
    ///   be passed every time it is called.
    ///
    /// # Examples
    #[cfg_attr(feature = "os-poll", doc = "```")]
    #[cfg_attr(not(feature = "os-poll"), doc = "```ignore")]
    /// use mio::net::{TcpSocket, TcpKeepalive};
    /// use std::time::Duration;
    ///
    /// # fn main() -> Result<(), std::io::Error> {
    /// let socket = TcpSocket::new_v6()?;
    /// let keepalive = TcpKeepalive::default()
    ///     .with_time(Duration::from_secs(4));
    ///     // Depending on the target operating system, we may also be able to
    ///     // configure the keepalive probe interval and/or the number of retries
    ///     // here as well.
    ///
    /// socket.set_keepalive_params(keepalive)?;
    /// # Ok(()) }
    /// ```
    ///
    /// [`TcpKeepalive`]: ../struct.TcpKeepalive.html
    /// [keepalive time]: ../struct.TcpKeepalive.html#method.with_time
    pub fn set_keepalive_params(&self, keepalive: TcpKeepalive) -> io::Result<()> {
        self.set_keepalive(true)?;
        sys::tcp::set_keepalive_params(self.sys, keepalive)
    }

    /// Returns the amount of time after which TCP keepalive probes will be sent
    /// on idle connections.
    ///
    /// If `None`, then keepalive messages are disabled.
    ///
    /// This returns the value of `SO_KEEPALIVE` + `IPPROTO_TCP` on OpenBSD,
    /// NetBSD, and Haiku, `TCP_KEEPALIVE` on macOS and iOS, and `TCP_KEEPIDLE`
    /// on all other Unix operating systems. On Windows, it is not possible to
    /// access the value of TCP keepalive parameters after they have been set.
    ///
    /// Some platforms specify this value in seconds, so sub-second
    /// specifications may be omitted.
    #[cfg_attr(docsrs, doc(cfg(not(target_os = "windows"))))]
    #[cfg(not(target_os = "windows"))]
    pub fn get_keepalive_time(&self) -> io::Result<Option<Duration>> {
        sys::tcp::get_keepalive_time(self.sys)
    }

    /// Returns the time interval between TCP keepalive probes, if TCP keepalive is
    /// enabled on this socket.
    ///
    /// If `None`, then keepalive messages are disabled.
    ///
    /// This returns the value of `TCP_KEEPINTVL` on supported Unix operating
    /// systems. On Windows, it is not possible to access the value of TCP
    /// keepalive parameters after they have been set..
    ///
    /// Some platforms specify this value in seconds, so sub-second
    /// specifications may be omitted.
    #[cfg_attr(
        docsrs,
        doc(cfg(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "netbsd",
        )))
    )]
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
    ))]
    pub fn get_keepalive_interval(&self) -> io::Result<Option<Duration>> {
        sys::tcp::get_keepalive_interval(self.sys)
    }

    /// Returns the maximum number of TCP keepalive probes that will be sent before
    /// dropping a connection, if TCP keepalive is enabled on this socket.
    ///
    /// If `None`, then keepalive messages are disabled.
    ///
    /// This returns the value of `TCP_KEEPCNT` on Unix operating systems that
    /// support this option. On Windows, it is not possible to access the value
    /// of TCP keepalive parameters after they have been set.
    #[cfg_attr(
        docsrs,
        doc(cfg(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "netbsd",
        )))
    )]
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
    ))]
    pub fn get_keepalive_retries(&self) -> io::Result<Option<u32>> {
        sys::tcp::get_keepalive_retries(self.sys)
    }

    /// Returns the local address of this socket
    ///
    /// Will return `Err` result in windows if called before calling `bind`
    pub fn get_localaddr(&self) -> io::Result<SocketAddr> {
        sys::tcp::get_localaddr(self.sys)
    }
}

impl Drop for TcpSocket {
    fn drop(&mut self) {
        sys::tcp::close(self.sys);
    }
}

#[cfg(unix)]
impl IntoRawFd for TcpSocket {
    fn into_raw_fd(self) -> RawFd {
        let ret = self.sys;
        // Avoid closing the socket
        mem::forget(self);
        ret
    }
}

#[cfg(unix)]
impl AsRawFd for TcpSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.sys
    }
}

#[cfg(unix)]
impl FromRawFd for TcpSocket {
    /// Converts a `RawFd` to a `TcpSocket`.
    ///
    /// # Notes
    ///
    /// The caller is responsible for ensuring that the socket is in
    /// non-blocking mode.
    unsafe fn from_raw_fd(fd: RawFd) -> TcpSocket {
        TcpSocket { sys: fd }
    }
}

#[cfg(windows)]
impl IntoRawSocket for TcpSocket {
    fn into_raw_socket(self) -> RawSocket {
        // The winapi crate defines `SOCKET` as `usize`. The Rust std
        // conditionally defines `RawSocket` as a fixed size unsigned integer
        // matching the pointer width. These end up being the same type but we
        // must cast between them.
        let ret = self.sys as RawSocket;

        // Avoid closing the socket
        mem::forget(self);

        ret
    }
}

#[cfg(windows)]
impl AsRawSocket for TcpSocket {
    fn as_raw_socket(&self) -> RawSocket {
        self.sys as RawSocket
    }
}

#[cfg(windows)]
impl FromRawSocket for TcpSocket {
    /// Converts a `RawSocket` to a `TcpSocket`.
    ///
    /// # Notes
    ///
    /// The caller is responsible for ensuring that the socket is in
    /// non-blocking mode.
    unsafe fn from_raw_socket(socket: RawSocket) -> TcpSocket {
        TcpSocket {
            sys: socket as sys::tcp::TcpSocket,
        }
    }
}

impl TcpKeepalive {
    // Sets the amount of time after which TCP keepalive probes will be sent
    /// on idle connections.
    ///
    /// This will set the value of `SO_KEEPALIVE` + `IPPROTO_TCP` on OpenBSD,
    /// NetBSD, and Haiku, `TCP_KEEPALIVE` on macOS and iOS, and `TCP_KEEPIDLE`
    /// on all other Unix operating systems. On Windows, this sets the value of
    /// the `tcp_keepalive` struct's `keepalivetime` field.
    ///
    /// Some platforms specify this value in seconds, so sub-second
    /// specifications may be omitted.
    pub fn with_time(self, time: Duration) -> Self {
        Self {
            time: Some(time),
            ..self
        }
    }

    /// Sets the time interval between TCP keepalive probes.
    /// This sets the value of `TCP_KEEPINTVL` on supported Unix operating
    /// systems. On Windows, this sets the value of the `tcp_keepalive` struct's
    /// `keepaliveinterval` field.
    ///
    /// Some platforms specify this value in seconds, so sub-second
    /// specifications may be omitted.
    #[cfg_attr(
        docsrs,
        doc(cfg(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "windows"
        )))
    )]
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "windows"
    ))]
    pub fn with_interval(self, interval: Duration) -> Self {
        Self {
            interval: Some(interval),
            ..self
        }
    }

    /// Sets the maximum number of TCP keepalive probes that will be sent before
    /// dropping a connection, if TCP keepalive is enabled on this socket.
    ///
    /// This will set the value of `TCP_KEEPCNT` on Unix operating systems that
    /// support this option.
    #[cfg_attr(
        docsrs,
        doc(cfg(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "netbsd",
        )))
    )]
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
    ))]
    pub fn with_retries(self, retries: u32) -> Self {
        Self {
            retries: Some(retries),
            ..self
        }
    }

    /// Returns a new, empty set of TCP keepalive parameters.
    pub fn new() -> Self {
        Self::default()
    }
}
