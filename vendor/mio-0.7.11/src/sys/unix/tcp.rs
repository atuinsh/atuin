use std::convert::TryInto;
use std::io;
use std::mem;
use std::mem::{size_of, MaybeUninit};
use std::net::{self, SocketAddr};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::time::Duration;

use crate::sys::unix::net::{new_socket, socket_addr, to_socket_addr};
use crate::net::TcpKeepalive;

#[cfg(any(target_os = "openbsd", target_os = "netbsd", target_os = "haiku"))]
use libc::SO_KEEPALIVE as KEEPALIVE_TIME;
#[cfg(any(target_os = "macos", target_os = "ios"))]
use libc::TCP_KEEPALIVE as KEEPALIVE_TIME;
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "haiku"
)))]
use libc::TCP_KEEPIDLE as KEEPALIVE_TIME;
pub type TcpSocket = libc::c_int;

pub(crate) fn new_v4_socket() -> io::Result<TcpSocket> {
    new_socket(libc::AF_INET, libc::SOCK_STREAM)
}

pub(crate) fn new_v6_socket() -> io::Result<TcpSocket> {
    new_socket(libc::AF_INET6, libc::SOCK_STREAM)
}

pub(crate) fn bind(socket: TcpSocket, addr: SocketAddr) -> io::Result<()> {
    let (raw_addr, raw_addr_length) = socket_addr(&addr);
    syscall!(bind(socket, raw_addr.as_ptr(), raw_addr_length))?;
    Ok(())
}

pub(crate) fn connect(socket: TcpSocket, addr: SocketAddr) -> io::Result<net::TcpStream> {
    let (raw_addr, raw_addr_length) = socket_addr(&addr);

    match syscall!(connect(socket, raw_addr.as_ptr(), raw_addr_length)) {
        Err(err) if err.raw_os_error() != Some(libc::EINPROGRESS) => {
            Err(err)
        }
        _ => {
            Ok(unsafe { net::TcpStream::from_raw_fd(socket) })
        }
    }
}

pub(crate) fn listen(socket: TcpSocket, backlog: u32) -> io::Result<net::TcpListener> {
    let backlog = backlog.try_into().unwrap_or(i32::max_value());
    syscall!(listen(socket, backlog))?;
    Ok(unsafe { net::TcpListener::from_raw_fd(socket) })
}

pub(crate) fn close(socket: TcpSocket) {
    let _ = unsafe { net::TcpStream::from_raw_fd(socket) };
}

pub(crate) fn set_reuseaddr(socket: TcpSocket, reuseaddr: bool) -> io::Result<()> {
    let val: libc::c_int = if reuseaddr { 1 } else { 0 };
    syscall!(setsockopt(
        socket,
        libc::SOL_SOCKET,
        libc::SO_REUSEADDR,
        &val as *const libc::c_int as *const libc::c_void,
        size_of::<libc::c_int>() as libc::socklen_t,
    ))
    .map(|_| ())
}

pub(crate) fn get_reuseaddr(socket: TcpSocket) -> io::Result<bool> {
    let mut optval: libc::c_int = 0;
    let mut optlen = mem::size_of::<libc::c_int>() as libc::socklen_t;

    syscall!(getsockopt(
        socket,
        libc::SOL_SOCKET,
        libc::SO_REUSEADDR,
        &mut optval as *mut _ as *mut _,
        &mut optlen,
    ))?;

    Ok(optval != 0)
}

#[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
pub(crate) fn set_reuseport(socket: TcpSocket, reuseport: bool) -> io::Result<()> {
    let val: libc::c_int = if reuseport { 1 } else { 0 };

    syscall!(setsockopt(
        socket,
        libc::SOL_SOCKET,
        libc::SO_REUSEPORT,
        &val as *const libc::c_int as *const libc::c_void,
        size_of::<libc::c_int>() as libc::socklen_t,
    ))
    .map(|_| ())
}

#[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
pub(crate) fn get_reuseport(socket: TcpSocket) -> io::Result<bool> {
    let mut optval: libc::c_int = 0;
    let mut optlen = mem::size_of::<libc::c_int>() as libc::socklen_t;

    syscall!(getsockopt(
        socket,
        libc::SOL_SOCKET,
        libc::SO_REUSEPORT,
        &mut optval as *mut _ as *mut _,
        &mut optlen,
    ))?;

    Ok(optval != 0)
}

pub(crate) fn get_localaddr(socket: TcpSocket) -> io::Result<SocketAddr> {
    let mut addr: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut length = size_of::<libc::sockaddr_storage>() as libc::socklen_t;

    syscall!(getsockname(
        socket,
        &mut addr as *mut _ as *mut _,
        &mut length
    ))?;

    unsafe { to_socket_addr(&addr) }
}

pub(crate) fn set_linger(socket: TcpSocket, dur: Option<Duration>) -> io::Result<()> {
    let val: libc::linger = libc::linger {
        l_onoff: if dur.is_some() { 1 } else { 0 },
        l_linger: dur
            .map(|dur| dur.as_secs() as libc::c_int)
            .unwrap_or_default(),
    };
    syscall!(setsockopt(
        socket,
        libc::SOL_SOCKET,
        #[cfg(target_vendor = "apple")]
        libc::SO_LINGER_SEC,
        #[cfg(not(target_vendor = "apple"))]
        libc::SO_LINGER,
        &val as *const libc::linger as *const libc::c_void,
        size_of::<libc::linger>() as libc::socklen_t,
    ))
    .map(|_| ())
}

pub(crate) fn get_linger(socket: TcpSocket) -> io::Result<Option<Duration>> {
    let mut val: libc::linger =  unsafe { std::mem::zeroed() };
    let mut len = mem::size_of::<libc::linger>() as libc::socklen_t;

    syscall!(getsockopt(
        socket,
        libc::SOL_SOCKET,
        #[cfg(target_vendor = "apple")]
        libc::SO_LINGER_SEC,
        #[cfg(not(target_vendor = "apple"))]
        libc::SO_LINGER,
        &mut val as *mut _ as *mut _,
        &mut len,
    ))?;

    if val.l_onoff == 0 {
        Ok(None)
    } else {
        Ok(Some(Duration::from_secs(val.l_linger as u64)))
    }
}

pub(crate) fn set_recv_buffer_size(socket: TcpSocket, size: u32) -> io::Result<()> {
    let size = size.try_into().ok().unwrap_or_else(i32::max_value);
    syscall!(setsockopt(
        socket,
        libc::SOL_SOCKET,
        libc::SO_RCVBUF,
        &size as *const _ as *const libc::c_void,
        size_of::<libc::c_int>() as libc::socklen_t
    ))
    .map(|_| ())
}

pub(crate) fn get_recv_buffer_size(socket: TcpSocket) -> io::Result<u32> {
    let mut optval: libc::c_int = 0;
    let mut optlen = size_of::<libc::c_int>() as libc::socklen_t;
    syscall!(getsockopt(
        socket,
        libc::SOL_SOCKET,
        libc::SO_RCVBUF,
        &mut optval as *mut _ as *mut _,
        &mut optlen,
    ))?;

    Ok(optval as u32)
}

pub(crate) fn set_send_buffer_size(socket: TcpSocket, size: u32) -> io::Result<()> {
    let size = size.try_into().ok().unwrap_or_else(i32::max_value);
    syscall!(setsockopt(
        socket,
        libc::SOL_SOCKET,
        libc::SO_SNDBUF,
        &size as *const _ as *const libc::c_void,
        size_of::<libc::c_int>() as libc::socklen_t
    ))
    .map(|_| ())
}

pub(crate) fn get_send_buffer_size(socket: TcpSocket) -> io::Result<u32> {
    let mut optval: libc::c_int = 0;
    let mut optlen = size_of::<libc::c_int>() as libc::socklen_t;

    syscall!(getsockopt(
        socket,
        libc::SOL_SOCKET,
        libc::SO_SNDBUF,
        &mut optval as *mut _ as *mut _,
        &mut optlen,
    ))?;

    Ok(optval as u32)
}

pub(crate) fn set_keepalive(socket: TcpSocket, keepalive: bool) -> io::Result<()> {
    let val: libc::c_int = if keepalive { 1 } else { 0 };
    syscall!(setsockopt(
        socket,
        libc::SOL_SOCKET,
        libc::SO_KEEPALIVE,
        &val as *const _ as *const libc::c_void,
        size_of::<libc::c_int>() as libc::socklen_t
    ))
    .map(|_| ())
}

pub(crate) fn get_keepalive(socket: TcpSocket) -> io::Result<bool> {
    let mut optval: libc::c_int = 0;
    let mut optlen = mem::size_of::<libc::c_int>() as libc::socklen_t;

    syscall!(getsockopt(
        socket,
        libc::SOL_SOCKET,
        libc::SO_KEEPALIVE,
        &mut optval as *mut _ as *mut _,
        &mut optlen,
    ))?;

    Ok(optval != 0)
}

pub(crate) fn set_keepalive_params(socket: TcpSocket, keepalive: TcpKeepalive) -> io::Result<()> {
    if let Some(dur) = keepalive.time {
        set_keepalive_time(socket, dur)?;
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
    ))]
    {
        if let Some(dur) = keepalive.interval {
            set_keepalive_interval(socket, dur)?;
        }
    
        if let Some(retries) = keepalive.retries {
            set_keepalive_retries(socket, retries)?;
        }
    }


    Ok(())
}

fn set_keepalive_time(socket: TcpSocket, time: Duration) -> io::Result<()> {
    let time_secs = time
        .as_secs()
        .try_into()
        .ok()
        .unwrap_or_else(i32::max_value);
    syscall!(setsockopt(
        socket,
        libc::IPPROTO_TCP,
        KEEPALIVE_TIME,
        &(time_secs as libc::c_int) as *const _ as *const libc::c_void,
        size_of::<libc::c_int>() as libc::socklen_t
    ))
    .map(|_| ())
}

pub(crate) fn get_keepalive_time(socket: TcpSocket) -> io::Result<Option<Duration>> {
    if !get_keepalive(socket)? {
        return Ok(None);
    }

    let mut optval: libc::c_int = 0;
    let mut optlen = mem::size_of::<libc::c_int>() as libc::socklen_t;
    syscall!(getsockopt(
        socket,
        libc::IPPROTO_TCP,
        KEEPALIVE_TIME,
        &mut optval as *mut _ as *mut _,
        &mut optlen,
    ))?;

    Ok(Some(Duration::from_secs(optval as u64)))
}

/// Linux, FreeBSD, and NetBSD support setting the keepalive interval via
/// `TCP_KEEPINTVL`.
/// See:
/// - https://man7.org/linux/man-pages/man7/tcp.7.html
/// - https://www.freebsd.org/cgi/man.cgi?query=tcp#end
/// - http://man.netbsd.org/tcp.4#DESCRIPTION
///
/// OpenBSD does not:
/// https://man.openbsd.org/tcp
#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
))]
fn set_keepalive_interval(socket: TcpSocket, interval: Duration) -> io::Result<()> {
    let interval_secs = interval
        .as_secs()
        .try_into()
        .ok()
        .unwrap_or_else(i32::max_value);
    syscall!(setsockopt(
        socket,
        libc::IPPROTO_TCP,
        libc::TCP_KEEPINTVL,
        &(interval_secs as libc::c_int) as *const _ as *const libc::c_void,
        size_of::<libc::c_int>() as libc::socklen_t
    ))
    .map(|_| ())
}

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
))]
pub(crate) fn get_keepalive_interval(socket: TcpSocket) -> io::Result<Option<Duration>> {
    if !get_keepalive(socket)? {
        return Ok(None);
    }

    let mut optval: libc::c_int = 0;
    let mut optlen = mem::size_of::<libc::c_int>() as libc::socklen_t;
    syscall!(getsockopt(
        socket,
        libc::IPPROTO_TCP,
        libc::TCP_KEEPINTVL,
        &mut optval as *mut _ as *mut _,
        &mut optlen,
    ))?;

    Ok(Some(Duration::from_secs(optval as u64)))
}

/// Linux, macOS/iOS, FreeBSD, and NetBSD support setting the number of TCP
/// keepalive retries via `TCP_KEEPCNT`.
/// See:
/// - https://man7.org/linux/man-pages/man7/tcp.7.html
/// - https://www.freebsd.org/cgi/man.cgi?query=tcp#end
/// - http://man.netbsd.org/tcp.4#DESCRIPTION
///
/// OpenBSD does not:
/// https://man.openbsd.org/tcp
#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
))]
fn set_keepalive_retries(socket: TcpSocket, retries: u32) -> io::Result<()> {
    let retries = retries.try_into().ok().unwrap_or_else(i32::max_value);
    syscall!(setsockopt(
        socket,
        libc::IPPROTO_TCP,
        libc::TCP_KEEPCNT,
        &(retries as libc::c_int) as *const _ as *const libc::c_void,
        size_of::<libc::c_int>() as libc::socklen_t
    ))
    .map(|_| ())
}

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
))]
pub(crate) fn get_keepalive_retries(socket: TcpSocket) -> io::Result<Option<u32>> {
    if !get_keepalive(socket)? {
        return Ok(None);
    }

    let mut optval: libc::c_int = 0;
    let mut optlen = mem::size_of::<libc::c_int>() as libc::socklen_t;
    syscall!(getsockopt(
        socket,
        libc::IPPROTO_TCP,
        libc::TCP_KEEPCNT,
        &mut optval as *mut _ as *mut _,
        &mut optlen,
    ))?;

    Ok(Some(optval as u32))
}

pub fn accept(listener: &net::TcpListener) -> io::Result<(net::TcpStream, SocketAddr)> {
    let mut addr: MaybeUninit<libc::sockaddr_storage> = MaybeUninit::uninit();
    let mut length = size_of::<libc::sockaddr_storage>() as libc::socklen_t;

    // On platforms that support it we can use `accept4(2)` to set `NONBLOCK`
    // and `CLOEXEC` in the call to accept the connection.
    #[cfg(any(
        // Android x86's seccomp profile forbids calls to `accept4(2)`
        // See https://github.com/tokio-rs/mio/issues/1445 for details
        all(
            not(target_arch="x86"),
            target_os = "android"
        ),
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "illumos",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    let stream = {
        syscall!(accept4(
            listener.as_raw_fd(),
            addr.as_mut_ptr() as *mut _,
            &mut length,
            libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
        ))
        .map(|socket| unsafe { net::TcpStream::from_raw_fd(socket) })
    }?;

    // But not all platforms have the `accept4(2)` call. Luckily BSD (derived)
    // OSes inherit the non-blocking flag from the listener, so we just have to
    // set `CLOEXEC`.
    #[cfg(any(
        all(
            target_arch = "x86",
            target_os = "android"
        ),
        target_os = "ios", 
        target_os = "macos", 
        target_os = "solaris"
    ))]
    let stream = {
        syscall!(accept(
            listener.as_raw_fd(),
            addr.as_mut_ptr() as *mut _,
            &mut length
        ))
        .map(|socket| unsafe { net::TcpStream::from_raw_fd(socket) })
        .and_then(|s| {
            syscall!(fcntl(s.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC))?;
    
            // See https://github.com/tokio-rs/mio/issues/1450
            #[cfg(all(target_arch = "x86",target_os = "android"))]
            syscall!(fcntl(s.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK))?;
            
            Ok(s)
        })
    }?;

    // This is safe because `accept` calls above ensures the address
    // initialised.
    unsafe { to_socket_addr(addr.as_ptr()) }.map(|addr| (stream, addr))
}
