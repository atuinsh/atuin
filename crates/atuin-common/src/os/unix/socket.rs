use std::path::Path;

use rustix::io::Errno;
use rustix::{fs, net};

/// Check whether a Unix socket is in use.
///
/// Unfortunately, on macOS and BSD, this can incorrectly return `false` if the socket is in use but
/// has a full backlog, as these systems return `ECONNREFUSED` in this case, which is
/// indistinguishable from a stale socket. Linux does not have this problem as it returns `EAGAIN`
/// instead. For Atuin's purposes (checking whether a socket is being actively used by an Atuin
/// daemon), this case is unlikely to happen in practice.
///
/// If `path` exists but is not a socket (e.g., is a regular file), this function will return either
/// `Ok(false)` or `Err(_)`. If you need to know whether `Ok(false)` was returned because a
/// non-socket file already exists at `path`, use [`std::fs::symlink_metadata`],
/// [`std::fs::Metadata::file_type`], and [`std::os::unix::fs::FileTypeExt::is_socket`].
///
/// # Errors
///
/// Returns an error if `connect()` returns anything other than 0 (success), `EAGAIN`, or
/// `ECONNREFUSED`.
pub fn socket_in_use(path: &Path) -> std::io::Result<bool> {
    let fd = net::socket(net::AddressFamily::UNIX, net::SocketType::STREAM, None)?;
    fs::fcntl_setfl(&fd, fs::fcntl_getfl(&fd)? | fs::OFlags::NONBLOCK)?;

    match net::connect(&fd, &net::SocketAddrUnix::new(path)?) {
        Ok(()) | Err(Errno::AGAIN) => Ok(true),
        Err(Errno::CONNREFUSED) => Ok(false),
        Err(e) => Err(e.into()),
    }
}
