use std::{fs, io};
use std::os::unix::io::AsRawFd;

use super::libc;


/// Is this stream a TTY?
pub fn is_tty<T: AsRawFd>(stream: &T) -> bool {
    unsafe { libc::isatty(stream.as_raw_fd()) == 1 }
}

/// Get the TTY device.
///
/// This allows for getting stdio representing _only_ the TTY, and not other streams.
pub fn get_tty() -> io::Result<fs::File> {
    fs::OpenOptions::new().read(true).write(true).open("/dev/tty")
}
