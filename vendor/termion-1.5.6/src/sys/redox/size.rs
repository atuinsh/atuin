use std::io;

use super::{cvt, redox_termios, syscall};

/// Get the size of the terminal.
pub fn terminal_size() -> io::Result<(u16, u16)> {
    let mut winsize = redox_termios::Winsize::default();

    let fd = cvt(syscall::dup(1, b"winsize"))?;
    let res = cvt(syscall::read(fd, &mut winsize));
    let _ = syscall::close(fd);

    if res? == winsize.len() {
        Ok((winsize.ws_col, winsize.ws_row))
    } else {
        Err(io::Error::new(io::ErrorKind::Other, "Unable to get the terminal size."))
    }
}
