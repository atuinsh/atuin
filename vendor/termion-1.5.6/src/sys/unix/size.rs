use std::{io, mem};

use super::cvt;
use super::libc::{c_ushort, ioctl, STDOUT_FILENO, TIOCGWINSZ};

#[repr(C)]
struct TermSize {
    row: c_ushort,
    col: c_ushort,
    x: c_ushort,
    y: c_ushort,
}
/// Get the size of the terminal.
pub fn terminal_size() -> io::Result<(u16, u16)> {
    unsafe {
        let mut size: TermSize = mem::zeroed();
        cvt(ioctl(STDOUT_FILENO, TIOCGWINSZ.into(), &mut size as *mut _))?;
        Ok((size.col as u16, size.row as u16))
    }
}

/// Get the size of the terminal, in pixels
pub fn terminal_size_pixels() -> io::Result<(u16, u16)> {
    unsafe {
        let mut size: TermSize = mem::zeroed();
        cvt(ioctl(STDOUT_FILENO, TIOCGWINSZ.into(), &mut size as *mut _))?;
        Ok((size.x as u16, size.y as u16))
    }
}
